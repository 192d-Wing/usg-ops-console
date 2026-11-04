use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;

use console_gateway::config::{self, Config};
use console_gateway::oidc::OidcClient;
use console_gateway::rate_limit::RateLimiter;
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
    for w in &cfg.warnings {
        tracing::warn!(warning = %w, "configuration");
    }
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

    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let oidc = OidcClient::discover(&cfg, clock.clone())
        .await
        .map_err(|e| e.to_string())?;

    let listen = cfg.listen;
    let state = Arc::new(AppState {
        cfg: Arc::new(cfg),
        store,
        audit: AuditSink::new(audit_pool),
        oidc: Arc::new(oidc),
        clock,
        login_limiter: RateLimiter::per_minute(LOGIN_PER_MINUTE),
    });
    let sweeper = sweeper::spawn(state.clone());

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
    res
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
