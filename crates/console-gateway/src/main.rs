use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;

use console_gateway::config::{self, Config};
use console_gateway::oidc::OidcClient;
use console_gateway::rate_limit::RateLimiter;
use console_gateway::registry::ToolsState;
use console_gateway::routes::AppState;
use console_gateway::session::{Clock, SessionStore, SystemClock};
use console_gateway::{app, audit::AuditSink, connect_pool, migrate, sweeper};
use tracing_subscriber::EnvFilter;

/// `/login` attempts per client IP per minute.
const LOGIN_PER_MINUTE: u32 = 30;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let res = match std::env::args().nth(1).as_deref() {
        None | Some("serve") => run().await,
        // One-shot job / init container: the only process that holds the
        // migrator credential (SEC-006).
        Some("migrate") => match config::migrator_url_from_env() {
            Ok(url) => migrate(&url).await.map(|()| {
                tracing::info!("database migrations applied");
            }),
            Err(e) => Err(e.to_string()),
        },
        Some(_) => Err("usage: console-gateway [serve|migrate]".into()),
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            tracing::error!(error = %msg, "console-gateway failed to start");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let cfg = Config::from_env().map_err(|e| e.to_string())?;
    // M4: the registry and assertion keys fail closed before anything binds.
    let mut warnings = cfg.warnings.clone();
    let tools = ToolsState::from_config(&cfg, &mut warnings)?;
    warn_if_group_readable(&mut warnings, "CONSOLE_ASSERTION_KEY_FILE");
    warn_if_group_readable(&mut warnings, "CONSOLE_TOOL_SESSION_HMAC_KEY_FILE");
    for w in &warnings {
        tracing::warn!(warning = %w, "configuration");
    }
    tracing::info!(
        tools = ?tools.iter().map(|t| t.cfg.id.as_str()).collect::<Vec<_>>(),
        kid = tools.keys().map(|k| k.kid()),
        "tool registry loaded"
    );
    let index = std::fs::read_to_string(format!("{}/index.html", cfg.dist))
        .map_err(|e| format!("CONSOLE_DIST/index.html: {e}"))?;

    if let Some(url) = &cfg.migrator_database_url {
        migrate(url).await?;
    }
    let pool = connect_pool(&cfg.database_url, cfg.db_max_connections)
        .await
        .map_err(|e| format!("CONSOLE_DATABASE_URL: connect failed: {e}"))?;
    let store = SessionStore::new(pool);
    if !store.schema_ready().await {
        return Err(
            "database schema not migrated (or CONSOLE_DATABASE_URL lacks console_app rights); \
             run `console-gateway migrate`"
                .into(),
        );
    }
    let audit_pool = connect_pool(&cfg.audit_database_url, cfg.audit_db_max_connections)
        .await
        .map_err(|e| format!("CONSOLE_AUDIT_DATABASE_URL: connect failed: {e}"))?;

    let audit = AuditSink::new(audit_pool);
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    audit_registry(&audit, &tools, clock.now()).await?;
    let oidc = OidcClient::discover(&cfg, clock.clone())
        .await
        .map_err(|e| e.to_string())?;

    let listen = cfg.listen;
    let state = Arc::new(AppState {
        cfg: Arc::new(cfg),
        store,
        audit,
        oidc: Arc::new(oidc),
        clock,
        login_limiter: RateLimiter::per_minute(LOGIN_PER_MINUTE),
        tools: Arc::new(tools),
    });
    let sweeper = sweeper::spawn(state.clone());
    let state_audit = state.audit.clone();

    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| format!("CONSOLE_LISTEN: {e}"))?;
    tracing::info!(addr = %listen, dist = %state.cfg.dist, "console-gateway listening");
    let res = axum::serve(
        listener,
        app(state, &index).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await
    .map_err(|e| e.to_string());
    sweeper.abort();
    // Best-effort audit rows still queued (PERF-002); bounded wait.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), state_audit.flush()).await;
    res
}

/// Start-up audit of the registry (CM-3 change record: what this gateway
/// serves) and of every `insecure_http_upstream` deviation (NIST-003).
/// Fails closed: a gateway that cannot record its deviations does not start.
// COMPLIANCE MAPPING:
// - NIST 800-53: CM-3 (Configuration Change Control), AU-2 — registry load audited with the tool ids
// - NIST 800-53: CM-6 (Configuration Settings), SC-8 — insecure_http_upstream deviation audited with its reason
async fn audit_registry(
    audit: &AuditSink,
    tools: &ToolsState,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    use console_gateway::audit::{AuditEvent, EventType, Outcome};
    let fail = |e: sqlx::Error| format!("audit write failed at start-up: {e}");
    let mut ev = AuditEvent::new(now, EventType::RegistryLoaded, Outcome::Success);
    ev.reason = Some("startup");
    ev.tool_id = Some(
        tools
            .iter()
            .map(|t| t.cfg.id.as_str())
            .collect::<Vec<_>>()
            .join(","),
    );
    audit.record(&ev).await.map_err(fail)?;
    for t in tools.iter() {
        let Some(reason) = &t.cfg.insecure_upstream_reason else {
            continue;
        };
        tracing::warn!(
            target: "audit",
            tool_id = %t.cfg.id,
            upstream = %t.cfg.upstream_origin(),
            insecure_http_upstream_reason = %reason,
            "plain-http upstream allowed by registry (CM-6 deviation)"
        );
        let mut ev = AuditEvent::new(now, EventType::RegistryInsecureUpstream, Outcome::Success);
        ev.reason = Some("insecure_http_upstream");
        ev.tool_id = Some(t.cfg.id.clone());
        audit.record(&ev).await.map_err(fail)?;
    }
    Ok(())
}

/// Key files should be readable by the gateway user only (mode 0600/0400).
fn warn_if_group_readable(warnings: &mut Vec<String>, key: &'static str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Some(path) = std::env::var(key).ok().filter(|p| !p.trim().is_empty()) else {
            return;
        };
        if let Ok(meta) = std::fs::metadata(&path)
            && meta.permissions().mode() & 0o077 != 0
        {
            warnings.push(format!(
                "{key}: file is readable by group/others; use mode 0600"
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = (warnings, key);
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { () = ctrl_c => {}, () = term => {} }
}
