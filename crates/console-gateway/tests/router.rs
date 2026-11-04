//! Router-level checks that need no database: every non-public route is
//! gated, public routes are reachable, hardening headers are present.

mod common;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use common::*;
use console_gateway::audit::AuditSink;
use console_gateway::oidc::{OidcClient, http_client};
use console_gateway::rate_limit::RateLimiter;
use console_gateway::routes::AppState;
use console_gateway::session::SessionStore;
use console_gateway::{app, headers};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

const ISSUER: &str = "http://localhost:8180/realms/icam";

/// Pools are lazy and point nowhere: any test that reaches the DB fails.
fn router() -> Router {
    let cfg = config(ISSUER, &[]);
    let lazy = || {
        PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(200))
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .unwrap()
    };
    let clock = ManualClock::at(t0());
    let oidc = OidcClient::from_metadata(
        metadata(ISSUER),
        &cfg,
        http_client().unwrap(),
        clock.clone(),
    )
    .unwrap();
    let state = Arc::new(AppState {
        cfg: Arc::new(cfg),
        store: SessionStore::new(lazy()),
        audit: AuditSink::new(lazy()),
        oidc: Arc::new(oidc),
        clock,
        login_limiter: RateLimiter::per_minute(30),
    });
    app(state, "")
}

async fn send(method: Method, path: &str, nav: bool) -> axum::response::Response {
    let mut b = Request::builder().method(method).uri(path);
    if nav {
        b = b.header("sec-fetch-mode", "navigate");
    }
    router()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn healthz_ok_with_security_headers() {
    let res = send(Method::GET, "/healthz", false).await;
    assert_eq!(res.status(), StatusCode::OK);
    let csp = res.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(
        csp.contains("form-action 'self' http://localhost:8180"),
        "SEC-004: issuer origin allowed for the logout form redirect: {csp}"
    );
    assert!(!csp.contains("unsafe-inline"));
    assert_eq!(res.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
}

/// Route enumeration: adding a route without allow-listing it here and in
/// `routes::PUBLIC_PATHS` makes it gated; this test keeps it that way.
#[tokio::test]
async fn every_non_public_route_requires_a_session() {
    let protected: &[(Method, &str)] = &[
        (Method::GET, "/"),
        (Method::GET, "/index.html"),
        (Method::GET, "/console-shell_bg.wasm"),
        (Method::GET, "/assets/shell.css"),
        (Method::GET, "/t/dcim/"),
        (Method::GET, "/api/session"),
        (Method::POST, "/api/session/keepalive"),
        (Method::POST, "/logout"),
        (Method::GET, "/logout"),
        (Method::DELETE, "/api/session"),
        (Method::GET, "/healthz/x"),
        (Method::GET, "/api/nope"),
        (Method::POST, "/api"),
        (Method::GET, "/auth.css/x"),
    ];
    for (m, p) in protected {
        let res = send(m.clone(), p, false).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{m} {p}");
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store", "{m} {p}");
        let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"], "unauthenticated");
        assert_eq!(v["reason"], "no_session");
        assert_eq!(v["login_url"], "/login");
    }
    let res = send(Method::GET, "/?site=x", true).await;
    assert_eq!(res.status(), StatusCode::FOUND);
    assert_eq!(
        res.headers()[header::LOCATION],
        "/login?return_to=%2F%3Fsite%3Dx"
    );
}

#[tokio::test]
async fn malformed_session_cookie_is_not_adopted() {
    let res = router()
        .oneshot(
            Request::get("/api/session")
                .header(header::COOKIE, "__Host-console_sid=attacker-chosen")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn public_pages_need_no_session() {
    let res = send(Method::GET, "/logged-out", true).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 8192).await.unwrap();
    assert!(
        std::str::from_utf8(&body)
            .unwrap()
            .contains("href=\"/login\"")
    );
    // /callback without the auth cookie fails before any DB access.
    // (It audits, which fails against the dead pool, but still answers 400.)
    let res = send(Method::GET, "/callback?code=x&state=y", true).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(
        res.headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .any(|c| c.to_str().unwrap().starts_with("__Host-console_auth=;"))
    );
}

#[tokio::test]
async fn auth_css_is_public_and_same_origin() {
    let res = send(Method::GET, "/auth.css", false).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/css")
    );
    let body = axum::body::to_bytes(res.into_body(), 8192).await.unwrap();
    assert!(std::str::from_utf8(&body).unwrap().contains("color-scheme"));
}

#[tokio::test]
async fn identity_headers_are_stripped() {
    use axum::routing::get;
    let echo = Router::new()
        .route(
            "/echo",
            get(|h: axum::http::HeaderMap| async move {
                headers::IDENTITY_HEADERS
                    .iter()
                    .filter(|n| h.contains_key(**n))
                    .count()
                    .to_string()
            }),
        )
        .layer(axum::middleware::from_fn(headers::strip_identity_headers));
    let mut req = Request::get("/echo");
    for n in headers::IDENTITY_HEADERS {
        req = req.header(n, "admin");
    }
    let res = echo
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = axum::body::to_bytes(res.into_body(), 64).await.unwrap();
    assert_eq!(&body[..], b"0");
}
