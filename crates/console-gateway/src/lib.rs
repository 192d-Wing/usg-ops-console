//! console-gateway: serves the shell behind Keycloak OIDC sessions.
//!
//! Module map: [`config`] (env, fail closed), [`oidc`] (relying party),
//! [`session`] (ids, cookies, expiry, store), [`csrf`], [`audit`],
//! [`client_ip`], [`return_to`], [`routes`] (HTTP surface), [`sweeper`],
//! [`headers`] (CSP and hardening), [`rate_limit`].

pub mod audit;
pub mod client_ip;
pub mod config;
pub mod csrf;
pub mod headers;
pub mod oidc;
pub mod rate_limit;
pub mod return_to;
pub mod routes;
pub mod session;
pub mod sweeper;

use std::time::Duration;

use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{any, get, post};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{ConnectOptions, PgPool};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::config::Secret;
use crate::headers::{content_security_policy, header_layer, strip_identity_headers};
use crate::routes::Shared;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const FALLBACK_CSP: &str = "default-src 'self'; frame-ancestors 'none'; base-uri 'none'";

/// Builds the full router. Everything except `/healthz`, `/login`, the
/// callback path, `/logged-out` and `/auth.css` sits behind `require_session`.
pub fn app(state: Shared, index_html: &str) -> Router {
    let dist = state.cfg.dist.clone();
    let spa = ServeDir::new(&dist).fallback(ServeFile::new(format!("{dist}/index.html")));
    let issuer_origin = state.cfg.issuer_origin();
    let csp = HeaderValue::from_str(&content_security_policy(
        index_html,
        issuer_origin.as_deref(),
    ))
    .unwrap_or_else(|_| HeaderValue::from_static(FALLBACK_CSP));

    let protected = Router::new()
        .route("/api/session", get(routes::api_session))
        .route("/api/session/keepalive", post(routes::keepalive))
        // INT-003: unknown API paths are an honest JSON 404, never the SPA.
        .route("/api", any(routes::api_not_found))
        .route("/api/", any(routes::api_not_found))
        .route("/api/{*rest}", any(routes::api_not_found))
        .route("/logout", post(routes::logout))
        .fallback_service(spa)
        .layer(from_fn_with_state(state.clone(), routes::require_session));
    let public = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/login", get(routes::login))
        .route(&state.cfg.oidc_redirect_path, get(routes::callback))
        .route("/logged-out", get(routes::logged_out))
        .route("/auth.css", get(routes::auth_css));

    public
        .merge(protected)
        .with_state(state)
        // NIST 800-53: SC-8 (HSTS), SC-18 (Mobile Code: CSP), plus nosniff/no-referrer hardening.
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            csp,
        ))
        .layer(header_layer(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(header_layer(header::REFERRER_POLICY, "no-referrer"))
        .layer(header_layer(
            header::STRICT_TRANSPORT_SECURITY,
            "max-age=31536000; includeSubDomains",
        ))
        .layer(from_fn(strip_identity_headers))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        // SEC-008: spans carry method + path only; never the query string
        // (the callback's `code`/`state`, `return_to`).
        .layer(TraceLayer::new_for_http().make_span_with(request_span))
}

/// Request span without the query string or headers.
// COMPLIANCE MAPPING:
// - NIST 800-53: SI-11 (Error Handling), AU-9 — no authorization codes or state in logs
pub fn request_span(req: &Request) -> tracing::Span {
    tracing::info_span!(
        "request",
        method = %req.method(),
        path = %req.uri().path(),
    )
}

/// `SET ROLE` applied to every migrator connection so all objects are owned
/// by the NOLOGIN owner role (SEC-006).
pub const OWNER_ROLE: &str = "console_owner";

/// Pool for `console-gateway migrate`: one connection, `SET ROLE console_owner`.
// COMPLIANCE MAPPING:
// - NIST 800-53: AU-9 (Protection of Audit Information), AC-6 — schema owned by a NOLOGIN role; runtime role DML only
pub async fn connect_migrator(url: &Secret) -> Result<PgPool, sqlx::Error> {
    let opts: PgConnectOptions = url.expose().parse()?;
    let opts = opts.disable_statement_logging();
    PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(30))
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::Executor::execute(&mut *conn, "SET ROLE console_owner").await?;
                Ok(())
            })
        })
        .connect_with(opts)
        .await
}

/// Runs the embedded migrations as the owner role.
pub async fn migrate(url: &Secret) -> Result<(), String> {
    let pool = connect_migrator(url)
        .await
        .map_err(|e| format!("CONSOLE_MIGRATOR_DATABASE_URL: connect failed: {e}"))?;
    MIGRATOR
        .run(&pool)
        .await
        .map_err(|e| format!("database migrations failed: {e}"))?;
    pool.close().await;
    Ok(())
}

// COMPLIANCE MAPPING:
// - NIST 800-53: SC-5 (Denial of Service Protection) — bounded pool, acquire and statement timeouts
/// Pool with bounded acquire/statement timeouts. Statement logging is off
/// so bound values (ids, tokens) never reach logs.
pub async fn connect_pool(url: &Secret, max: u32) -> Result<PgPool, sqlx::Error> {
    let opts: PgConnectOptions = url.expose().parse()?;
    let opts = opts
        .options([
            ("statement_timeout", "5000"),
            ("idle_in_transaction_session_timeout", "10000"),
        ])
        .disable_statement_logging();
    PgPoolOptions::new()
        .max_connections(max)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(opts)
        .await
}
