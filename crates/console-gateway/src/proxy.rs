//! Streaming reverse proxy for `/t/<id>/*` on tool origins (plan.md section
//! 4.5): per-tool pooled hyper client over rustls (ring), request header
//! allow-list rebuild (SEC-010 / POAM-026), the signed identity assertion,
//! response header filtering, and bounded I/O (connect, response-header and
//! per-frame idle timeouts; body cap; per-tool concurrency).

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use http_body::{Frame, SizeHint};
use http_body_util::{LengthLimitError, Limited};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioTimer};
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Sleep;
use tracing::Instrument;
use url::Url;

use crate::registry::ToolConfig;

type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type HttpsClient = Client<HttpsConnector<HttpConnector>, Body>;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
pub const BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
pub const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// Request headers forwarded as-is (plus `sec-fetch-*` and the registry's
/// `forward_headers`). Everything else is dropped.
const ALLOWED_REQUEST: [&str; 18] = [
    "accept",
    "accept-language",
    "accept-encoding",
    "cache-control",
    "content-type",
    "content-length",
    "content-encoding",
    "if-match",
    "if-none-match",
    "if-modified-since",
    "if-unmodified-since",
    "if-range",
    "range",
    "origin",
    "referer",
    "user-agent",
    "x-requested-with",
    "last-event-id",
];

/// Hop-by-hop headers (RFC 9110 section 7.6.1) plus legacy proxy ones.
const HOP_BY_HOP: [&str; 7] = [
    "connection",
    "keep-alive",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "proxy-connection",
];

/// One tool's upstream: pooled client and concurrency permits.
pub struct Upstream {
    client: HttpsClient,
    permits: Arc<Semaphore>,
}

impl std::fmt::Debug for Upstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Upstream")
            .field("available", &self.permits.available_permits())
            .finish()
    }
}

/// Mozilla (webpki) roots plus the platform store.
pub fn base_roots() -> RootCertStore {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        let _ = roots.add(cert);
    }
    roots
}

// COMPLIANCE MAPPING:
// - NIST 800-53: SC-8 (Transmission Confidentiality and Integrity), IA-9 (Service Identification) — verified TLS to https upstreams; optional per-tool CA
// - NIST 800-53: SC-5 (Denial of Service Protection) — connect timeout, bounded pool, per-tool concurrency
/// Builds the per-tool client. `roots` is shared; `upstream_ca_file` adds
/// one tool-specific trust anchor bundle.
pub fn build_upstream(
    tool: &ToolConfig,
    roots: &RootCertStore,
    read_file: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Result<Upstream, String> {
    let mut roots = roots.clone();
    if let Some(path) = &tool.upstream_ca_file {
        let pem = read_file(path)
            .ok_or_else(|| format!("tool {:?}: upstream_ca_file: could not read file", tool.id))?;
        let mut added = 0;
        for cert in CertificateDer::pem_slice_iter(&pem) {
            let cert =
                cert.map_err(|_| format!("tool {:?}: upstream_ca_file: bad PEM", tool.id))?;
            roots
                .add(cert)
                .map_err(|_| format!("tool {:?}: upstream_ca_file: bad certificate", tool.id))?;
            added += 1;
        }
        if added == 0 {
            return Err(format!(
                "tool {:?}: upstream_ca_file: no certificates",
                tool.id
            ));
        }
    }
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| "rustls: no safe protocol versions".to_owned())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let mut http = HttpConnector::new();
    http.enforce_http(false);
    http.set_connect_timeout(Some(CONNECT_TIMEOUT));
    http.set_nodelay(true);
    let builder = hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(tls);
    let https = if tool.upstream.scheme() == "https" {
        builder
            .https_only()
            .enable_http1()
            .enable_http2()
            .wrap_connector(http)
    } else {
        builder
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .wrap_connector(http)
    };
    let client = Client::builder(TokioExecutor::new())
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .pool_timer(TokioTimer::new())
        .timer(TokioTimer::new())
        .build(https);
    let permits = usize::try_from(tool.max_concurrent).unwrap_or(1);
    Ok(Upstream {
        client,
        permits: Arc::new(Semaphore::new(permits)),
    })
}

// ---------------------------------------------------------------------------
// Pure header / path rules (unit-tested)
// ---------------------------------------------------------------------------

/// Paths rejected before forwarding: dot segments, encoded dot / slash /
/// backslash / NUL, a raw backslash, or a non-UTF-8 decoding.
// COMPLIANCE MAPPING:
// - NIST 800-53: SI-10 (Information Input Validation) — no traversal or encoded-separator smuggling to upstreams
pub fn path_ok(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "%00"]
        .iter()
        .any(|p| lower.contains(p))
        || path.contains('\\')
        || path.contains('\0')
    {
        return false;
    }
    if path.split('/').any(|seg| seg == "." || seg == "..") {
        return false;
    }
    percent_decodes_utf8(path)
}

fn percent_decodes_utf8(path: &str) -> bool {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok());
            match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(b) => {
                    out.push(b);
                    i += 3;
                }
                None => return false,
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    std::str::from_utf8(&out).is_ok()
}

/// Names listed in `Connection` (lower-case), which are hop-by-hop too.
fn connection_listed(h: &HeaderMap) -> Vec<String> {
    h.get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

/// `Connection: upgrade` / `Upgrade:` / CONNECT: WebSockets and friends
/// (not proxied in M4, POAM).
pub fn is_upgrade(method: &Method, h: &HeaderMap) -> bool {
    method == Method::CONNECT
        || h.contains_key(header::UPGRADE)
        || connection_listed(h).iter().any(|t| t == "upgrade")
}

/// Identity-looking headers a client sent (spoof attempt; they are dropped
/// either way). `x-console-scope` is the SDK's legitimate header.
pub fn spoof_attempt(h: &HeaderMap) -> bool {
    h.keys().any(|n| {
        let n = n.as_str();
        (n.starts_with("x-console-") && n != console_protocol::tools::SCOPE_HEADER)
            || matches!(
                n,
                "x-forwarded-user" | "x-remote-user" | "remote-user" | "x-user" | "x-auth-user"
            )
    })
}

/// `Cookie` without any console or tool-session cookie (the tool's own
/// cookies pass). `None` when nothing is left.
pub fn rebuild_cookie(h: &HeaderMap) -> Option<HeaderValue> {
    let kept: Vec<&str> = h
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .map(str::trim)
        .filter(|pair| {
            let name = pair.split('=').next().unwrap_or_default().trim();
            !pair.is_empty()
                && !name.starts_with("__Host-console")
                && name != console_protocol::tools::TOOL_SESSION_COOKIE
        })
        .collect();
    if kept.is_empty() {
        None
    } else {
        HeaderValue::from_str(&kept.join("; ")).ok()
    }
}

/// The allow-list rebuild: only listed headers (minus any named in
/// `Connection`), plus the rebuilt `Cookie`. Identity and hop-by-hop
/// headers never pass, whatever their case or count.
// COMPLIANCE MAPPING:
// - NIST 800-53: AC-3 (Access Enforcement), AC-4 (Information Flow Enforcement) — allow-list rebuild; identity only from the gateway (closes POAM-026 / SEC-010)
// - NIST 800-53: SC-23 (Session Authenticity) — console and tool session cookies never forwarded upstream
pub fn upstream_request_headers(incoming: &HeaderMap, extra: &[HeaderName]) -> HeaderMap {
    let listed = connection_listed(incoming);
    let mut out = HeaderMap::new();
    for (name, value) in incoming {
        let n = name.as_str();
        let allowed = ALLOWED_REQUEST.contains(&n)
            || n.starts_with("sec-fetch-")
            || extra.iter().any(|e| e == name);
        if allowed && !listed.iter().any(|l| l == n) && !HOP_BY_HOP.contains(&n) {
            out.append(name.clone(), value.clone());
        }
    }
    if let Some(c) = rebuild_cookie(incoming) {
        out.insert(header::COOKIE, c);
    }
    out
}

/// Upstream `Set-Cookie` is kept only when host-only and not one of ours.
pub fn keep_set_cookie(v: &HeaderValue) -> bool {
    let Ok(s) = v.to_str() else { return false };
    let mut parts = s.split(';');
    let name = parts
        .next()
        .and_then(|nv| nv.split('=').next())
        .map(str::trim)
        .unwrap_or_default();
    if name.is_empty()
        || name.starts_with("__Host-console")
        || name == console_protocol::tools::TOOL_SESSION_COOKIE
    {
        return false;
    }
    !parts.any(|a| {
        let a = a.trim().to_ascii_lowercase();
        a == "domain" || a.starts_with("domain=")
    })
}

/// SEC-004: an upstream `Location` may only be path-relative (`/x`, `x`,
/// `?q`, `#f`; never `//host` or `\\host`) or absolute on the upstream
/// authority (rewritten onto the tool origin) or on the tool origin itself.
/// Anything else (another origin, a scheme-relative URL, other schemes,
/// control characters) is `None`, and the proxy answers 502: a tool must not
/// turn the gateway into an open redirector off its own origin.
// COMPLIANCE MAPPING:
// - NIST 800-53: SC-7 (Boundary Protection), SI-10 — redirects confined to the tool origin
pub fn rewrite_location(
    v: &HeaderValue,
    upstream_origin: &str,
    tool_origin: &str,
) -> Option<HeaderValue> {
    let s = v.to_str().ok()?;
    if s.is_empty() || s.chars().any(|c| c.is_control() || c == '\\') {
        return None;
    }
    if path_relative(s) {
        return Some(v.clone());
    }
    let url = Url::parse(s).ok()?;
    let origin = url.origin().ascii_serialization();
    if origin == tool_origin {
        return Some(v.clone());
    }
    if origin != upstream_origin {
        return None;
    }
    let mut rest = url.path().to_owned();
    if let Some(q) = url.query() {
        rest.push('?');
        rest.push_str(q);
    }
    if let Some(f) = url.fragment() {
        rest.push('#');
        rest.push_str(f);
    }
    HeaderValue::from_str(&format!("{tool_origin}{rest}")).ok()
}

/// A relative reference without scheme or authority (RFC 3986 4.2).
fn path_relative(s: &str) -> bool {
    if s.starts_with("//") {
        return false;
    }
    // A ':' before the first '/', '?' or '#' would be a scheme.
    let head = s.split(['/', '?', '#']).next().unwrap_or_default();
    !head.contains(':')
}

/// The upstream sent a `Location` the gateway refuses to pass (SEC-004).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForeignLocation;

/// Response filtering for proxied responses. The tool-origin layer adds
/// CSP / HSTS / nosniff / Referrer-Policy / CORP on every response.
pub fn filter_response_headers(
    status: StatusCode,
    h: &mut HeaderMap,
    tool: &ToolConfig,
) -> Result<(), ForeignLocation> {
    for n in connection_listed(h) {
        if let Ok(name) = HeaderName::from_bytes(n.as_bytes()) {
            h.remove(name);
        }
    }
    for n in HOP_BY_HOP {
        h.remove(n);
    }
    let proxy_named: Vec<HeaderName> = h
        .keys()
        .filter(|k| k.as_str().starts_with("proxy-"))
        .cloned()
        .collect();
    for n in proxy_named {
        h.remove(n);
    }
    h.remove(header::STRICT_TRANSPORT_SECURITY);
    h.remove(header::SERVER);
    let cookies: Vec<HeaderValue> = h.get_all(header::SET_COOKIE).iter().cloned().collect();
    h.remove(header::SET_COOKIE);
    for c in cookies.into_iter().filter(keep_set_cookie) {
        h.append(header::SET_COOKIE, c);
    }
    if let Some(loc) = h.get(header::LOCATION).cloned() {
        let rewritten =
            rewrite_location(&loc, &tool.upstream_origin(), &tool.origin).ok_or(ForeignLocation)?;
        h.insert(header::LOCATION, rewritten);
    }
    let html = h
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.trim_start().to_ascii_lowercase().starts_with("text/html"));
    let ok = status.is_success() || status.is_redirection();
    if !h.contains_key(header::CACHE_CONTROL) && (!ok || html) {
        h.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Bodies
// ---------------------------------------------------------------------------

/// Response body with a per-frame idle timeout (no total timeout, so long
/// downloads and SSE stream). Holds the concurrency permit until the body
/// ends or is dropped.
pub struct IdleTimeoutBody<B> {
    inner: B,
    idle: Duration,
    sleep: Pin<Box<Sleep>>,
    _permit: Option<OwnedSemaphorePermit>,
}

impl<B> IdleTimeoutBody<B> {
    pub fn new(inner: B, idle: Duration, permit: Option<OwnedSemaphorePermit>) -> Self {
        Self {
            inner,
            idle,
            sleep: Box::pin(tokio::time::sleep(idle)),
            _permit: permit,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("upstream body idle timeout")]
pub struct IdleTimeout;

impl<B> http_body::Body for IdleTimeoutBody<B>
where
    B: http_body::Body<Data = Bytes> + Unpin,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = &mut *self;
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                let deadline = tokio::time::Instant::now() + this.idle;
                this.sleep.as_mut().reset(deadline);
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e.into()))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => match this.sleep.as_mut().poll(cx) {
                Poll::Ready(()) => {
                    tracing::warn!("upstream body idle timeout");
                    Poll::Ready(Some(Err(Box::new(IdleTimeout))))
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

// ---------------------------------------------------------------------------
// Forwarding
// ---------------------------------------------------------------------------

/// Why forwarding failed (mapped to 413/502/503/504 by the caller).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardError {
    /// Body over `max_body_bytes` (by Content-Length or while streaming).
    TooLarge,
    /// Per-tool concurrency exhausted.
    Busy,
    /// Connect / protocol failure.
    BadGateway,
    /// No response headers within the tool's timeout.
    Timeout,
}

fn too_large_source(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        if err.is::<LengthLimitError>() {
            return true;
        }
        cur = err.source();
    }
    false
}

/// Successful forward: the response plus upstream time to headers.
pub struct Forwarded {
    pub response: Response,
    pub ttfb: Duration,
}

/// Sends `req` (already authenticated/authorized) to the tool's upstream
/// with `headers` (the rebuilt set) and streams the answer back.
pub async fn forward(
    tool: &ToolConfig,
    upstream: &Upstream,
    req: Request,
    headers: HeaderMap,
) -> Result<Forwarded, ForwardError> {
    let declared = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > tool.max_body_bytes) {
        return Err(ForwardError::TooLarge);
    }
    let permit = upstream
        .permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| ForwardError::Busy)?;
    let (parts, body) = req.into_parts();
    let pq = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let uri: Uri = format!("{}{pq}", tool.upstream_origin())
        .parse()
        .map_err(|_| ForwardError::BadGateway)?;
    let limit = usize::try_from(tool.max_body_bytes).unwrap_or(usize::MAX);
    let mut out = axum::http::Request::new(Body::new(Limited::new(body, limit)));
    *out.method_mut() = parts.method;
    *out.uri_mut() = uri;
    *out.headers_mut() = headers;

    let span = tracing::info_span!(
        "proxy.upstream",
        tool = %tool.id,
        status = tracing::field::Empty,
        ttfb_ms = tracing::field::Empty
    );
    let t0 = Instant::now();
    let res = tokio::time::timeout(tool.response_header_timeout, upstream.client.request(out))
        .instrument(span.clone())
        .await;
    let ttfb = t0.elapsed();
    span.record(
        "ttfb_ms",
        u64::try_from(ttfb.as_millis()).unwrap_or(u64::MAX),
    );
    let res = match res {
        Err(_) => {
            tracing::warn!(parent: &span, tool = %tool.id, "upstream response timeout");
            return Err(ForwardError::Timeout);
        }
        Ok(Err(e)) => {
            if too_large_source(&e) {
                return Err(ForwardError::TooLarge);
            }
            tracing::warn!(parent: &span, tool = %tool.id, error = %e, "upstream request failed");
            return Err(ForwardError::BadGateway);
        }
        Ok(Ok(r)) => r,
    };
    span.record("status", res.status().as_u16());
    let (mut parts, incoming) = res.into_parts();
    if filter_response_headers(parts.status, &mut parts.headers, tool).is_err() {
        // The target is not logged: it may carry tokens in its query.
        tracing::warn!(parent: &span, tool = %tool.id, "upstream Location off the tool origin refused");
        return Err(ForwardError::BadGateway);
    }
    let body = Body::new(IdleTimeoutBody::new(
        incoming,
        BODY_IDLE_TIMEOUT,
        Some(permit),
    ));
    Ok(Forwarded {
        response: Response::from_parts(parts, body).into_response(),
        ttfb,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> ToolConfig {
        let reg = r#"
[[tool]]
id = "hello"
title = "Hello"
origin = "https://hello.tools.example.mil"
upstream = "http://127.0.0.1:8090"
forward_headers = ["x-hello-csrf"]
"#;
        crate::registry::parse(
            "r",
            reg,
            &Url::parse("https://console.example.mil").unwrap(),
            false,
            &mut Vec::new(),
        )
        .unwrap()
        .remove(0)
    }

    fn hm(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.append(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        m
    }

    #[test]
    fn allow_list_rebuild_drops_identity_and_hop_by_hop() {
        let t = tool();
        let incoming = hm(&[
            ("accept", "text/html"),
            ("user-agent", "ua"),
            ("sec-fetch-mode", "navigate"),
            ("x-hello-csrf", "tok"),
            ("authorization", "Bearer x"),
            ("x-console-user", "admin"),
            ("X-Console-Assertion", "forged"),
            ("x-console-scope", "site=a"),
            ("x-forwarded-for", "1.2.3.4"),
            ("x-forwarded-user", "admin"),
            ("x-real-ip", "1.2.3.4"),
            ("forwarded", "for=1.2.3.4"),
            ("remote-user", "admin"),
            ("host", "evil"),
            ("connection", "keep-alive, accept-language"),
            ("accept-language", "en"),
            ("keep-alive", "timeout=5"),
            ("te", "trailers"),
            ("transfer-encoding", "chunked"),
            ("upgrade", "websocket"),
            ("proxy-authorization", "x"),
            ("x-unknown", "y"),
            (
                "cookie",
                "a=1; __Host-console_sid=SECRET; __Host-tool_sid=TOOL",
            ),
            ("cookie", "__Host-console_auth=z; b=2"),
        ]);
        let out = upstream_request_headers(&incoming, &t.forward_headers);
        let names: Vec<&str> = out.keys().map(HeaderName::as_str).collect();
        for kept in [
            "accept",
            "user-agent",
            "sec-fetch-mode",
            "x-hello-csrf",
            "cookie",
        ] {
            assert!(names.contains(&kept), "{kept} kept: {names:?}");
        }
        for gone in [
            "authorization",
            "x-console-user",
            "x-console-assertion",
            "x-console-scope",
            "x-forwarded-for",
            "x-forwarded-user",
            "x-real-ip",
            "forwarded",
            "remote-user",
            "host",
            "connection",
            "accept-language",
            "keep-alive",
            "te",
            "transfer-encoding",
            "upgrade",
            "proxy-authorization",
            "x-unknown",
        ] {
            assert!(!names.contains(&gone), "{gone} dropped: {names:?}");
        }
        assert_eq!(out[header::COOKIE], "a=1; b=2");
        assert!(spoof_attempt(&incoming));
        assert!(!spoof_attempt(&hm(&[("x-console-scope", "site=a")])));
        let only_ours = hm(&[("cookie", "__Host-console_sid=x; __Host-tool_sid=y")]);
        assert!(
            upstream_request_headers(&only_ours, &[])
                .get(header::COOKIE)
                .is_none()
        );
    }

    #[test]
    fn set_cookie_filter() {
        let v = |s: &'static str| HeaderValue::from_static(s);
        assert!(keep_set_cookie(&v("pref=dark; Path=/; Secure")));
        assert!(!keep_set_cookie(&v("__Host-tool_sid=x; Path=/")));
        assert!(!keep_set_cookie(&v("__Host-console_sid=x; Path=/")));
        assert!(!keep_set_cookie(&v("a=1; Domain=tools.example.mil")));
        assert!(!keep_set_cookie(&v("a=1; path=/; DOMAIN=.example.mil")));
        assert!(!keep_set_cookie(&v("=x")));
    }

    #[test]
    fn location_rewrite_only_for_upstream_authority() {
        let up = "http://127.0.0.1:8090";
        let origin = "https://hello.tools.example.mil";
        let r = |s: &'static str| {
            rewrite_location(&HeaderValue::from_static(s), up, origin)
                .map(|v| v.to_str().unwrap().to_owned())
        };
        assert_eq!(
            r("http://127.0.0.1:8090/t/hello/x?y=1#f").as_deref(),
            Some("https://hello.tools.example.mil/t/hello/x?y=1#f")
        );
        for same in [
            "/t/hello/next",
            "next",
            "./a/b",
            "?q=1",
            "#f",
            "a/b:c",
            "https://hello.tools.example.mil/t/hello/",
        ] {
            assert_eq!(r(same).as_deref(), Some(same), "{same}");
        }
        for foreign in [
            "https://idp.example.mil/auth",
            "http://127.0.0.1:9999/x",
            "//evil.example/x",
            "/\\evil.example/x",
            "\\\\evil.example",
            "javascript:alert(1)",
            "data:text/html,x",
            "https://hello.tools.example.mil.evil.example/",
            "",
        ] {
            assert_eq!(r(foreign), None, "{foreign}");
        }
    }

    #[test]
    fn response_headers_filtered() {
        let t = tool();
        let mut h = hm(&[
            ("content-type", "text/html; charset=utf-8"),
            ("server", "upstream/1.0"),
            ("strict-transport-security", "max-age=0"),
            ("connection", "x-private"),
            ("x-private", "1"),
            ("transfer-encoding", "chunked"),
            ("proxy-authenticate", "Basic"),
            ("set-cookie", "ok=1; Path=/"),
            ("set-cookie", "__Host-tool_sid=evil; Path=/"),
            ("set-cookie", "x=1; Domain=example.mil"),
            ("location", "http://127.0.0.1:8090/t/hello/a"),
            ("x-app", "kept"),
        ]);
        filter_response_headers(StatusCode::FOUND, &mut h, &t).unwrap();
        for gone in [
            "server",
            "strict-transport-security",
            "connection",
            "x-private",
            "transfer-encoding",
            "proxy-authenticate",
        ] {
            assert!(!h.contains_key(gone), "{gone}");
        }
        let cookies: Vec<_> = h.get_all(header::SET_COOKIE).iter().collect();
        assert_eq!(cookies, vec!["ok=1; Path=/"]);
        assert_eq!(
            h[header::LOCATION],
            "https://hello.tools.example.mil/t/hello/a"
        );
        assert_eq!(
            h[header::CACHE_CONTROL],
            "private, no-store",
            "HTML gets no-store"
        );
        assert_eq!(h["x-app"], "kept");

        let mut json = hm(&[("content-type", "application/json")]);
        filter_response_headers(StatusCode::OK, &mut json, &t).unwrap();
        assert!(
            !json.contains_key(header::CACHE_CONTROL),
            "cacheable 2xx asset untouched"
        );
        let mut err = hm(&[("content-type", "application/json")]);
        filter_response_headers(StatusCode::INTERNAL_SERVER_ERROR, &mut err, &t).unwrap();
        assert_eq!(err[header::CACHE_CONTROL], "private, no-store");
        let mut own = hm(&[
            ("content-type", "text/html"),
            ("cache-control", "max-age=60"),
        ]);
        filter_response_headers(StatusCode::OK, &mut own, &t).unwrap();
        assert_eq!(
            own[header::CACHE_CONTROL],
            "max-age=60",
            "tool's own policy kept"
        );
    }

    #[test]
    fn path_rules() {
        for ok in [
            "/t/hello/",
            "/t/hello/a/b.js",
            "/t/hello/%20x",
            "/t/hello/a..b",
            "/t/hello//x",
        ] {
            assert!(path_ok(ok), "{ok}");
        }
        for bad in [
            "/t/hello/../evil/",
            "/t/hello/./x",
            "/t/hello/%2e%2e/x",
            "/t/hello/%2E/x",
            "/t/hello/a%2fb",
            "/t/hello/a%5cb",
            "/t/hello/a\\b",
            "/t/hello/%00",
            "/t/hello/%ff",
            "/t/hello/%zz",
            "/t/hello/..",
        ] {
            assert!(!path_ok(bad), "{bad}");
        }
    }

    #[test]
    fn upgrade_detection() {
        assert!(is_upgrade(&Method::GET, &hm(&[("upgrade", "websocket")])));
        assert!(is_upgrade(&Method::GET, &hm(&[("connection", "Upgrade")])));
        assert!(is_upgrade(&Method::CONNECT, &HeaderMap::new()));
        assert!(!is_upgrade(
            &Method::GET,
            &hm(&[("connection", "keep-alive")])
        ));
    }
}
