//! hello-tool backend: the template for a console tool server.
//!
//! The gateway forwards `/t/<id>/...` verbatim, so this server mounts one
//! sub-router per configured tool id (`hello`, and the hostile `evil` fixture
//! from the same binary). Every request under `/t/<id>/` (API and static
//! files) needs a valid `X-Console-Assertion` with `aud == <id>`, otherwise it
//! gets 401. Only `/healthz` is open.
//!
//! API (relative to `/t/<id>/`):
//! - `GET  api/whoami`  the verified claims (the only identity the tool trusts)
//! - `GET  api/headers` the request headers that actually arrived (E2E proof
//!   that the gateway stripped spoofed identity/hop-by-hop headers)
//! - `GET  api/ping`    `pong` (proxy overhead bench)
//! - `GET  api/counter` / `POST api/counter` per-user counter; POST is the
//!   state-changing request for `audit_writes` tests and needs
//!   `X-Hello-CSRF: 1` (the tool's own CSRF header, `forward_headers`).

pub mod assertion;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::{OriginalUri, Request, State};
use axum::handler::HandlerWithoutStateExt;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use base64::Engine as _;
use console_protocol::tools::{ASSERTION_HEADER, tool_base_path};
use console_protocol::{AssertionClaims, valid_id};
use serde_json::json;
use sha2::{Digest, Sha256};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

pub use assertion::{Verifier, VerifyError};

/// The tool's own CSRF header (must be in the registry `forward_headers`).
pub const CSRF_HEADER: &str = "x-hello-csrf";

#[derive(Debug, Clone)]
pub struct Config {
    /// Tool ids served by this process. Each must be a registry id.
    pub tool_ids: Vec<String>,
    /// The id that renders the hostile sandbox-escape page and gets a
    /// permissive CSP so its attempts reach the browser/gateway controls.
    pub evil_id: Option<String>,
    /// Trunk `dist` of `examples/hello-tool/web`.
    pub web_dist: PathBuf,
}

#[derive(Default)]
struct Counters(Mutex<HashMap<(String, String), u64>>);

struct ToolState {
    id: String,
    verifier: Arc<Verifier>,
    counters: Arc<Counters>,
}

pub fn app(cfg: &Config, verifier: Arc<Verifier>) -> Result<Router, String> {
    let index = std::fs::read_to_string(cfg.web_dist.join("index.html"))
        .map_err(|e| format!("{}: {e}", cfg.web_dist.join("index.html").display()))?;
    let counters = Arc::new(Counters::default());
    let mut router = Router::new().route("/healthz", get(|| async { "ok" }));
    for id in &cfg.tool_ids {
        if !valid_id(id) {
            return Err(format!("invalid tool id {id:?}"));
        }
        let evil = cfg.evil_id.as_deref() == Some(id.as_str());
        let base = tool_base_path(id);
        let st = Arc::new(ToolState {
            id: id.clone(),
            verifier: verifier.clone(),
            counters: counters.clone(),
        });
        let tr = tool_router(st, &cfg.web_dist, &index, &base, evil);
        // `nest` does not match the bare `/t/<id>/`; mount it explicitly (the
        // tool router's static fallback then serves the SPA for it).
        router = router
            .route_service(&base, tr.clone().into_service::<axum::body::Body>())
            .nest(base.trim_end_matches('/'), tr);
    }
    Ok(router)
}

fn tool_router(st: Arc<ToolState>, dist: &Path, index: &str, base: &str, evil: bool) -> Router {
    let csp = HeaderValue::try_from(tool_csp(index, evil)).expect("CSP is ASCII");
    let page = Arc::new(inject_base(index, base));
    let spa = {
        let page = page.clone();
        move || {
            let page = page.clone();
            async move { ([no_store()], Html(page.as_str().to_owned())) }
        }
    };
    let static_files = ServeDir::new(dist)
        .append_index_html_on_directories(false)
        .fallback(spa.clone().into_service());
    Router::new()
        .route("/", get(spa.clone()))
        .route("/index.html", get(spa))
        .route("/api/whoami", get(whoami))
        .route("/api/headers", get(headers))
        .route("/api/ping", get(|| async { "pong" }))
        .route("/api/counter", get(counter_get).post(counter_post))
        .route("/api/{*rest}", axum::routing::any(api_not_found))
        .fallback_service(static_files)
        .layer(middleware::from_fn_with_state(
            st.clone(),
            require_assertion,
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            csp,
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .with_state(st)
}

fn no_store() -> (HeaderName, HeaderValue) {
    (header::CACHE_CONTROL, HeaderValue::from_static("no-store"))
}

/// Trunk builds with relative asset URLs (`public_url = "./"`); one `<base>`
/// per tool id makes them resolve under `/t/<id>/` for any deep link.
fn inject_base(index: &str, base: &str) -> String {
    index.replacen("<head>", &format!("<head><base href=\"{base}\">"), 1)
}

/// The tool's own CSP. The gateway adds a second one
/// (`frame-ancestors <console>; base-uri 'self'; object-src 'none'`);
/// browsers enforce both. The evil fixture opens `connect-src`/`form-action`/`frame-src`
/// so that its console-API attempts are stopped by the console (CORS, CSRF,
/// Origin check) and not merely by its own CSP.
fn tool_csp(index: &str, evil: bool) -> String {
    let hashes: String = inline_scripts(index)
        .map(|b| {
            let d = Sha256::digest(b.as_bytes());
            format!(
                " 'sha256-{}'",
                base64::engine::general_purpose::STANDARD.encode(d)
            )
        })
        .collect();
    let open = if evil { "*" } else { "'self'" };
    format!(
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'{hashes}; style-src 'self'; \
         connect-src {open}; form-action {open}; frame-src {open}; base-uri 'self'; \
         object-src 'none'"
    )
}

fn inline_scripts(html: &str) -> impl Iterator<Item = &str> {
    html.split("<script").skip(1).filter_map(|rest| {
        let (tag, after) = rest.split_once('>')?;
        let (body, _) = after.split_once("</script>")?;
        (!tag.contains("src=")).then_some(body)
    })
}

fn json_error(status: StatusCode, error: &str) -> Response {
    (status, [no_store()], Json(json!({ "error": error }))).into_response()
}

async fn require_assertion(
    State(st): State<Arc<ToolState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let token = req
        .headers()
        .get(ASSERTION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let res = match token {
        None => Err(VerifyError::Missing),
        Some(t) => st.verifier.verify(&t, &st.id).await,
    };
    match res {
        Ok(claims) => {
            req.extensions_mut().insert(Arc::new(claims));
            next.run(req).await
        }
        Err(e) => {
            let path = req
                .extensions()
                .get::<OriginalUri>()
                .map_or_else(|| req.uri().path().to_owned(), |u| u.path().to_owned());
            tracing::warn!(tool = %st.id, error = %e, %path, "assertion rejected");
            let status = if e == VerifyError::KeysUnavailable {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::UNAUTHORIZED
            };
            json_error(status, "invalid_assertion")
        }
    }
}

async fn whoami(
    State(st): State<Arc<ToolState>>,
    Extension(c): Extension<Arc<AssertionClaims>>,
) -> Response {
    (
        [no_store()],
        Json(json!({ "tool_id": st.id, "verified": true, "claims": &*c })),
    )
        .into_response()
}

/// Header names and values as received. The assertion value is replaced by a
/// marker (it was verified), and cookies are reduced to their names.
async fn headers(headers: HeaderMap) -> Response {
    let mut out: Vec<serde_json::Value> = headers
        .iter()
        .map(|(name, value)| {
            let v = match name.as_str() {
                ASSERTION_HEADER => "<verified>".to_owned(),
                "cookie" => value
                    .to_str()
                    .unwrap_or("")
                    .split(';')
                    .filter_map(|p| p.split_once('=').map(|(n, _)| n.trim().to_owned()))
                    .collect::<Vec<_>>()
                    .join("; "),
                _ => String::from_utf8_lossy(value.as_bytes()).into_owned(),
            };
            json!({ "name": name.as_str(), "value": v })
        })
        .collect();
    out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    ([no_store()], Json(json!({ "headers": out }))).into_response()
}

fn counter_key(st: &ToolState, c: &AssertionClaims) -> (String, String) {
    (st.id.clone(), c.sub.clone())
}

async fn counter_get(
    State(st): State<Arc<ToolState>>,
    Extension(c): Extension<Arc<AssertionClaims>>,
) -> Response {
    let n = st
        .counters
        .0
        .lock()
        .map(|m| m.get(&counter_key(&st, &c)).copied().unwrap_or(0))
        .unwrap_or(0);
    ([no_store()], Json(json!({ "count": n }))).into_response()
}

async fn counter_post(
    State(st): State<Arc<ToolState>>,
    Extension(c): Extension<Arc<AssertionClaims>>,
    headers: HeaderMap,
) -> Response {
    if headers.get(CSRF_HEADER).map(HeaderValue::as_bytes) != Some(b"1") {
        return json_error(StatusCode::FORBIDDEN, "csrf");
    }
    let Ok(mut m) = st.counters.0.lock() else {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
    };
    let n = m.entry(counter_key(&st, &c)).or_insert(0);
    *n += 1;
    tracing::info!(tool = %st.id, sub = %c.sub, count = *n, "counter incremented");
    ([no_store()], Json(json!({ "count": *n }))).into_response()
}

async fn api_not_found() -> Response {
    json_error(StatusCode::NOT_FOUND, "not_found")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn dist() -> PathBuf {
        let d = std::env::temp_dir().join(format!("hello-tool-dist-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("index.html"),
            "<html><head><script type=\"module\">boot()</script></head><body></body></html>",
        )
        .unwrap();
        std::fs::write(d.join("app.js"), "x").unwrap();
        d
    }

    fn router() -> Router {
        let cfg = Config {
            tool_ids: vec!["hello".into(), "evil".into()],
            evil_id: Some("evil".into()),
            web_dist: dist(),
        };
        app(&cfg, Arc::new(assertion::test_verifier())).unwrap()
    }

    async fn call(
        r: &Router,
        method: &str,
        uri: &str,
        token: Option<&str>,
        extra: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, String) {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            b = b.header(ASSERTION_HEADER, t);
        }
        for (k, v) in extra {
            b = b.header(*k, *v);
        }
        let res = r
            .clone()
            .oneshot(b.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let (p, body) = res.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes();
        (p.status, p.headers, String::from_utf8_lossy(&bytes).into())
    }

    #[tokio::test]
    async fn rejects_missing_and_cross_tool_assertion() {
        let r = router();
        for uri in ["/t/hello/api/whoami", "/t/hello/", "/t/hello/app.js"] {
            let (s, _, b) = call(&r, "GET", uri, None, &[]).await;
            assert_eq!(s, StatusCode::UNAUTHORIZED, "{uri}");
            assert!(b.contains("invalid_assertion"));
        }
        // A token minted for evil is not accepted by hello.
        let evil = assertion::test_token("evil");
        let (s, _, _) = call(&r, "GET", "/t/hello/api/whoami", Some(&evil), &[]).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, _, _) = call(&r, "GET", "/healthz", None, &[]).await;
        assert_eq!(s, StatusCode::OK);
    }

    #[tokio::test]
    async fn whoami_returns_verified_claims() {
        let r = router();
        let t = assertion::test_token("hello");
        let (s, h, b) = call(&r, "GET", "/t/hello/api/whoami", Some(&t), &[]).await;
        assert_eq!(s, StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&b).unwrap();
        assert_eq!(v["tool_id"], "hello");
        assert_eq!(v["claims"]["preferred_username"], "jdoe");
        assert_eq!(v["claims"]["ctx"]["site"], "dc1");
        assert_eq!(h[header::CACHE_CONTROL], "no-store");
    }

    #[tokio::test]
    async fn headers_echo_redacts_assertion_and_cookie_values() {
        let r = router();
        let t = assertion::test_token("hello");
        let (_, _, b) = call(
            &r,
            "GET",
            "/t/hello/api/headers",
            Some(&t),
            &[("cookie", "a=secret; b=2"), ("x-test", "v")],
        )
        .await;
        let v: serde_json::Value = serde_json::from_str(&b).unwrap();
        let find = |n: &str| {
            v["headers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|h| h["name"] == n)
                .map(|h| h["value"].as_str().unwrap().to_owned())
        };
        assert_eq!(find("x-console-assertion").as_deref(), Some("<verified>"));
        assert_eq!(find("cookie").as_deref(), Some("a; b"));
        assert_eq!(find("x-test").as_deref(), Some("v"));
        assert!(!b.contains("secret"));
    }

    #[tokio::test]
    async fn counter_post_needs_csrf_header() {
        let r = router();
        let t = assertion::test_token("hello");
        let (s, _, _) = call(&r, "POST", "/t/hello/api/counter", Some(&t), &[]).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, _, b) = call(
            &r,
            "POST",
            "/t/hello/api/counter",
            Some(&t),
            &[(CSRF_HEADER, "1")],
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert!(b.contains("\"count\":1"));
        let (_, _, b) = call(&r, "GET", "/t/hello/api/counter", Some(&t), &[]).await;
        assert!(b.contains("\"count\":1"));
    }

    #[tokio::test]
    async fn spa_injects_base_and_csp_hash() {
        let r = router();
        let t = assertion::test_token("hello");
        for uri in ["/t/hello/", "/t/hello/deep/link"] {
            let (s, h, b) = call(&r, "GET", uri, Some(&t), &[]).await;
            assert_eq!(s, StatusCode::OK, "{uri}");
            assert!(b.contains("<base href=\"/t/hello/\">"));
            let csp = h[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
            let hash = base64::engine::general_purpose::STANDARD.encode(Sha256::digest(b"boot()"));
            assert!(csp.contains(&hash));
            assert!(csp.contains("connect-src 'self'"));
        }
        let (s, _, b) = call(&r, "GET", "/t/hello/app.js", Some(&t), &[]).await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "x"));
        let (s, _, _) = call(&r, "GET", "/t/hello/api/nope", Some(&t), &[]).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn evil_csp_is_permissive_for_its_attempts() {
        let r = router();
        let t = assertion::test_token("evil");
        let (_, h, b) = call(&r, "GET", "/t/evil/", Some(&t), &[]).await;
        assert!(b.contains("<base href=\"/t/evil/\">"));
        let csp = h[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.contains("connect-src *"));
    }

    #[test]
    fn manifest_matches_registry_entries() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let m = console_protocol::Manifest::from_toml(
            &std::fs::read_to_string(dir.join("web/console-tool.toml")).unwrap(),
        )
        .unwrap();
        assert_eq!(m.id, "hello");
        assert_eq!(m.sdk_protocol, console_protocol::PROTOCOL_VERSION);
        // Registries are owned by deploy/ (WU5); check whichever exist.
        let reg_dir = dir.join("../../deploy/compose/tools");
        let Ok(entries) = std::fs::read_dir(&reg_dir) else {
            return;
        };
        for e in entries.flatten() {
            let src = std::fs::read_to_string(e.path()).unwrap();
            let v: toml::Value = toml::from_str(&src).unwrap();
            let tools = v.get("tool").and_then(|t| t.as_array()).cloned();
            for t in tools.unwrap_or_default() {
                if t.get("id").and_then(|x| x.as_str()) != Some("hello") {
                    continue;
                }
                let scopes: Vec<String> = t
                    .get("scopes")
                    .and_then(|s| s.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(scopes, m.scopes, "{}", e.path().display());
                let fwd = t.get("forward_headers").and_then(|s| s.as_array());
                assert!(
                    fwd.is_some_and(|a| a.iter().any(|x| x.as_str() == Some(CSRF_HEADER))),
                    "{}: hello needs forward_headers = [\"{CSRF_HEADER}\"]",
                    e.path().display()
                );
            }
        }
    }
}
