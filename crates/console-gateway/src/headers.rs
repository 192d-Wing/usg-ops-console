//! Response hardening headers, CSP, and the inbound identity-header strip.

use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use sha2::{Digest, Sha256};
use tower_http::set_header::SetResponseHeaderLayer;

/// Identity headers the gateway alone may set upstream (M4). Anything a
/// client sends under these names is dropped before routing.
pub const IDENTITY_HEADERS: [&str; 4] = [
    "x-console-user",
    "x-console-roles",
    "x-console-session",
    "x-forwarded-user",
];

/// Request extension: identity headers were present and stripped (the
/// tool proxy audits it as a spoof attempt).
#[derive(Debug, Clone, Copy)]
pub struct SpoofedIdentity;

// COMPLIANCE MAPPING:
// - NIST 800-53: AC-3 (Access Enforcement) — client-supplied identity headers never reach handlers/upstreams
/// Defence in depth: the tool proxy rebuilds headers from an allow-list
/// anyway (`proxy::upstream_request_headers`).
pub async fn strip_identity_headers(mut req: Request, next: Next) -> Response {
    let mut stripped = false;
    for name in IDENTITY_HEADERS {
        stripped |= req.headers_mut().remove(name).is_some();
    }
    if stripped {
        req.extensions_mut().insert(SpoofedIdentity);
    }
    next.run(req).await
}

/// CSP that allows only same-origin scripts plus the exact inline bootstrap
/// scripts Trunk wrote into `index.html` (by SHA-256), never `'unsafe-inline'`.
/// `form_action_origin` (the IdP origin) lets the no-JS logout form follow
/// its 303 to the end-session endpoint (SEC-004); browsers apply
/// `form-action` to redirects after a form submission. `frame_origins` (the
/// registry tool origins) is the whole `frame-src` (`'none'` when empty).
// COMPLIANCE MAPPING:
// - NIST 800-53: AC-4 (Information Flow Enforcement), SC-18 — the shell may frame registry tool origins only
pub fn content_security_policy(
    index_html: &str,
    form_action_origin: Option<&str>,
    frame_origins: &[String],
) -> String {
    let hashes: String = inline_scripts(index_html)
        .map(|body| format!(" 'sha256-{}'", B64.encode(Sha256::digest(body.as_bytes()))))
        .collect();
    let source_ok = |o: &&str| {
        o.bytes()
            .all(|b| b.is_ascii_graphic() && b != b';' && b != b',' && b != b'\'')
    };
    let extra = form_action_origin
        .filter(source_ok)
        .map(|o| format!(" {o}"))
        .unwrap_or_default();
    let frames: Vec<&str> = frame_origins
        .iter()
        .map(String::as_str)
        .filter(source_ok)
        .collect();
    let frame_src = if frames.is_empty() {
        "'none'".to_owned()
    } else {
        frames.join(" ")
    };
    format!(
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'{hashes}; style-src 'self'; \
         frame-src {frame_src}; frame-ancestors 'none'; base-uri 'none'; form-action 'self'{extra}"
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

pub fn header_layer(name: HeaderName, value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
}

pub fn no_store() -> (HeaderName, HeaderValue) {
    (header::CACHE_CONTROL, HeaderValue::from_static("no-store"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_only_inline_scripts() {
        let html = r#"<script src="/a.js"></script><script type="module">boot()</script>"#;
        let csp = content_security_policy(html, None, &[]);
        assert!(csp.contains("frame-src 'none';"));
        let expected = B64.encode(Sha256::digest(b"boot()"));
        assert!(csp.contains(&format!("'sha256-{expected}'")));
        assert_eq!(csp.matches("'sha256-").count(), 1);
        assert!(csp.ends_with("form-action 'self'"));
        let csp = content_security_policy("", Some("https://kc.example.mil"), &[]);
        assert!(csp.ends_with("form-action 'self' https://kc.example.mil"));
        let csp = content_security_policy("", Some("https://x; script-src *"), &[]);
        assert!(
            csp.ends_with("form-action 'self'"),
            "no directive injection"
        );
        let csp = content_security_policy(
            "",
            None,
            &[
                "https://hello.tools.x".into(),
                "https://localhost:9448".into(),
            ],
        );
        assert!(csp.contains("frame-src https://hello.tools.x https://localhost:9448;"));
    }
}
