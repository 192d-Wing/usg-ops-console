use std::net::SocketAddr;

use axum::Router;
use axum::http::{HeaderName, HeaderValue, header};
use axum::routing::get;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use sha2::{Digest, Sha256};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let addr: SocketAddr = std::env::var("CONSOLE_LISTEN")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()
        .map_err(std::io::Error::other)?;
    let dist = std::env::var("CONSOLE_DIST").unwrap_or_else(|_| "crates/console-shell/dist".into());
    let index = std::fs::read_to_string(format!("{dist}/index.html"))?;

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, %dist, "console-gateway listening");
    axum::serve(listener, app(&dist, &index))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
}

fn app(dist: &str, index_html: &str) -> Router {
    let spa = ServeDir::new(dist).fallback(ServeFile::new(format!("{dist}/index.html")));
    let csp = HeaderValue::from_str(&content_security_policy(index_html))
        .expect("CSP contains only ASCII");
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .fallback_service(spa)
        // NIST 800-53: SC-8, SI-10 — baseline browser hardening headers.
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
        .layer(TraceLayer::new_for_http())
}

/// CSP that allows only same-origin scripts plus the exact inline bootstrap
/// scripts Trunk wrote into `index.html` (by SHA-256), never `'unsafe-inline'`.
fn content_security_policy(index_html: &str) -> String {
    let hashes: String = inline_scripts(index_html)
        .map(|body| format!(" 'sha256-{}'", B64.encode(Sha256::digest(body.as_bytes()))))
        .collect();
    format!(
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'{hashes}; style-src 'self'; \
         frame-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"
    )
}

/// Bodies of `<script>` elements without a `src` attribute.
fn inline_scripts(html: &str) -> impl Iterator<Item = &str> {
    html.split("<script").skip(1).filter_map(|rest| {
        let (tag, after) = rest.split_once('>')?;
        let (body, _) = after.split_once("</script>")?;
        (!tag.contains("src=")).then_some(body)
    })
}

fn header_layer(name: HeaderName, value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn healthz_ok_with_security_headers() {
        let res = app("/nonexistent", "")
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let csp = res.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(!csp.contains("unsafe-inline"));
        assert_eq!(res.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    }

    #[test]
    fn hashes_only_inline_scripts() {
        let html = r#"<script src="/a.js"></script><script type="module">boot()</script>"#;
        let csp = content_security_policy(html);
        let expected = B64.encode(Sha256::digest(b"boot()"));
        assert!(csp.contains(&format!("'sha256-{expected}'")));
        assert_eq!(csp.matches("'sha256-").count(), 1);
    }
}
