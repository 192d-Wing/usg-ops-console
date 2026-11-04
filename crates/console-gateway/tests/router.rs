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
    router_with(false)
}

fn router_with(tools: bool) -> Router {
    let cfg = config(ISSUER, &[]);
    let tools = if tools {
        tools_state(&cfg, &registry_toml(9))
    } else {
        console_gateway::registry::ToolsState::empty()
    };
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
        tools: Arc::new(tools),
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

async fn body_string(res: axum::response::Response) -> String {
    let body = axum::body::to_bytes(res.into_body(), 16 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&body).into_owned()
}

fn on_host(method: Method, host: &str, path: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, host)
}

/// Tool content is never served on the console origin, session or not.
#[tokio::test]
async fn console_origin_404s_tool_paths() {
    for p in [
        "/t",
        "/t/",
        "/t/hello/",
        "/t/hello/x?y=1",
        "/_console/launch?code=x",
        "/_console/context",
    ] {
        let res = router_with(true)
            .oneshot(Request::get(p).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{p}");
        assert_eq!(body_string(res).await, r#"{"error":"not_found"}"#);
    }
}

#[tokio::test]
async fn console_csp_frames_registry_origins_only() {
    let res = router_with(true)
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let csp = res.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(
        csp.contains(&format!(
            "frame-src {HELLO_ORIGIN} https://tight.tools.test"
        )),
        "{csp}"
    );
    let res = send(Method::GET, "/healthz", false).await;
    let csp = res.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(csp.contains("frame-src 'none'"), "{csp}");
}

#[tokio::test]
async fn jwks_is_public_and_cacheable() {
    let res = router_with(true)
        .oneshot(
            Request::get("/.well-known/jwks.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CACHE_CONTROL], "public, max-age=300");
    let v: serde_json::Value = serde_json::from_str(&body_string(res).await).unwrap();
    assert_eq!(v["keys"].as_array().unwrap().len(), 1);
    assert_eq!(v["keys"][0]["alg"], "ES256");
    assert!(v["keys"][0].get("d").is_none());
    let res = send(Method::GET, "/.well-known/jwks.json", false).await;
    assert_eq!(body_string(res).await, r#"{"keys":[]}"#);
}

#[tokio::test]
async fn tool_api_requires_a_session() {
    for (m, p) in [
        (Method::GET, "/api/tools"),
        (Method::POST, "/api/tools/hello/launch"),
    ] {
        let res = router_with(true)
            .oneshot(
                Request::builder()
                    .method(m.clone())
                    .uri(p)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{m} {p}");
    }
}

/// Tool origin: hardening headers, no console CSP, 404/421/401 contract.
#[tokio::test]
async fn tool_origin_routing_and_headers() {
    let r = router_with(true);
    // Unknown path on the tool origin.
    let res = r
        .clone()
        .oneshot(
            on_host(Method::GET, HELLO_HOST, "/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let csps: Vec<&str> = res
        .headers()
        .get_all(header::CONTENT_SECURITY_POLICY)
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    assert_eq!(
        csps,
        vec!["frame-ancestors http://localhost:8080; base-uri 'self'; object-src 'none'"],
        "tool-origin CSP only (never the console's)"
    );
    assert_eq!(res.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(res.headers()["cross-origin-resource-policy"], "same-site");
    assert!(
        res.headers()
            .contains_key(header::STRICT_TRANSPORT_SECURITY)
    );
    // The Host match ignores case and the default port.
    let res = r
        .clone()
        .oneshot(
            on_host(Method::GET, "HELLO.tools.test:443", "/t/hello/x")
                .header(header::ACCEPT, "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let v: serde_json::Value = serde_json::from_str(&body_string(res).await).unwrap();
    assert_eq!(v["error"], "unauthenticated");
    assert!(
        v.get("login_url").is_none(),
        "no /login redirect from a tool origin"
    );
    // Navigation without a tool session: 401 page linking back to the console.
    let res = r
        .clone()
        .oneshot(
            on_host(Method::GET, HELLO_HOST, "/t/hello/")
                .header("sec-fetch-mode", "navigate")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let html = body_string(res).await;
    assert!(
        html.contains("http://localhost:8080/tools/hello/"),
        "{html}"
    );
    assert!(html.contains("http://localhost:8080/auth.css"));
    // Another tool's path on this origin.
    let res = r
        .clone()
        .oneshot(
            on_host(Method::GET, HELLO_HOST, "/t/tight/x")
                .header(header::ACCEPT, "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::MISDIRECTED_REQUEST);
    assert_eq!(body_string(res).await, r#"{"error":"misdirected_request"}"#);
    // `/t/<id>` → `/t/<id>/`.
    let res = r
        .clone()
        .oneshot(
            on_host(Method::GET, HELLO_HOST, "/t/hello?a=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(res.headers()[header::LOCATION], "/t/hello/?a=1");
    // Traversal is rejected before any session lookup.
    let res = r
        .clone()
        .oneshot(
            on_host(Method::GET, HELLO_HOST, "/t/hello/%2e%2e/x")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_string(res).await, r#"{"error":"bad_request"}"#);
    // A malformed launch code: 401, no-referrer, never a DB-backed session.
    let res = r
        .oneshot(
            on_host(Method::GET, HELLO_HOST, "/_console/launch?code=short")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(res.headers()[header::REFERRER_POLICY], "no-referrer");
    assert!(res.headers().get(header::SET_COOKIE).is_none());
}

/// A host that is not a registry origin keeps the M3 console behaviour.
#[tokio::test]
async fn unknown_host_is_the_console() {
    let res = router_with(true)
        .oneshot(
            on_host(Method::GET, "evil.tools.test", "/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
}

/// SEC-011: a tool the session may not use answers exactly like an unknown
/// id, and the denial audit is queued rather than awaited: the audit pool
/// here points at a black-hole address with a 60 s acquire timeout, so an
/// awaited DB write would blow the 2 s bound.
#[tokio::test]
async fn forbidden_tool_launch_matches_unknown_id_without_awaiting_db() {
    use axum::extract::{Path, State};
    use base64::Engine;
    use console_gateway::session::Session;

    let cfg = config(ISSUER, &[]);
    let tools = tools_state(&cfg, &registry_toml(9));
    let blackhole = || {
        PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_secs(60))
            .connect_lazy("postgres://nobody@192.0.2.1:5432/none")
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
    let origin = cfg.public_origin();
    let state = Arc::new(AppState {
        cfg: Arc::new(cfg),
        store: SessionStore::new(blackhole()),
        audit: AuditSink::new(blackhole()),
        oidc: Arc::new(oidc),
        clock,
        login_limiter: RateLimiter::per_minute(30),
        tools: Arc::new(tools),
    });
    let csrf = vec![7u8; 32];
    let sess = Arc::new(Session {
        id_hash: vec![1; 32],
        user_sub: "sub".into(),
        username: "user".into(),
        display_name: "User".into(),
        roles: vec!["hello:user".into()],
        csrf_token: csrf.clone(),
        created_at: t0(),
        last_seen_at: t0(),
        absolute_expires_at: t0() + chrono::Duration::hours(8),
        idle_timeout_secs: None,
        acr: None,
    });
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&csrf);
    let call = |id: &'static str| {
        let state = state.clone();
        let sess = sess.clone();
        let origin = origin.clone();
        let token = token.clone();
        async move {
            let mut req = Request::builder()
                .method(Method::POST)
                .uri(format!("/api/tools/{id}/launch"))
                .header(header::ORIGIN, origin)
                .header(console_gateway::csrf::HEADER, token)
                .body(Body::empty())
                .unwrap();
            req.extensions_mut().insert(sess);
            let res = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                console_gateway::tools_api::launch(State(state), Path(id.to_owned()), req),
            )
            .await
            .expect("launch must not wait on the audit DB");
            let status = res.status();
            let hdrs = res.headers().clone();
            (status, hdrs, body_string(res).await)
        }
    };
    let unknown = call("no-such-tool").await;
    let denied = call("denied").await;
    assert_eq!(unknown.0, StatusCode::NOT_FOUND);
    assert_eq!(unknown, denied, "status, headers and body are identical");
}
