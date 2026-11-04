//! Tool origins (plan.md sections 0, 4.2, 4.4): requests whose `Host` is a
//! registry origin are served here, never by the console app. Routes:
//! `/_console/launch` (code exchange), `/_console/context`, and `/t/<id>/*`
//! (authenticated proxy). Everything else is 404. The console's CSP and
//! 30 s request timeout do not apply (streaming); every response gets the
//! tool-origin hardening headers instead.

use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::Router;
use axum::extract::{RawQuery, Request, State};
use axum::http::{Extensions, HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use console_protocol::tools::{
    ASSERTION_HEADER, SCOPE_HEADER, TOOL_SESSION_COOKIE, parse_scope_header,
};
use console_protocol::{
    AssertionClaims, AuthErrorBody, AuthErrorKind, AuthErrorReason, PROTOCOL_VERSION, Scope,
    ToolContext,
};
use tower::ServiceExt;

use crate::assertion::new_jti;
use crate::audit::{AuditEvent, EventType, Outcome};
use crate::headers::no_store;
use crate::proxy::{self, ForwardError};
use crate::rate_limit::Decision;
use crate::registry::{Tool, host_key};
use crate::routes::{
    Meta, Shared, append_cookie, bad_request, is_navigation, json_error, redirect, wants_json,
};
use crate::session::{self, Session};
use crate::tools_api::path_only;

/// The tool a request's `Host` selected (set by [`dispatch`]).
#[derive(Clone)]
pub struct ToolHost(pub Arc<Tool>);

/// `Host` of a request: the URI authority (HTTP/2) or the `Host` header.
pub fn request_host(req: &Request) -> Option<String> {
    req.uri()
        .authority()
        .map(|a| a.as_str().to_owned())
        .or_else(|| {
            req.headers()
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        })
        .map(|h| host_key(&h))
}

/// Outer middleware: a registry host goes to the tool-origin router; any
/// other host continues to the console app (M3 behaviour unchanged).
// COMPLIANCE MAPPING:
// - NIST 800-53: SC-7 (Boundary Protection), SC-39 (Process Isolation) — host-based split between console and per-tool origins
pub async fn dispatch(
    State((s, tools_router)): State<(Shared, Router)>,
    mut req: Request,
    next: Next,
) -> Response {
    let tool = request_host(&req).and_then(|h| s.tools.by_host(&h).cloned());
    match tool {
        Some(tool) => {
            req.extensions_mut().insert(ToolHost(tool));
            match tools_router.oneshot(req).await {
                Ok(res) => res,
                Err(never) => match never {},
            }
        }
        None => next.run(req).await,
    }
}

pub fn router(state: Shared) -> Router {
    Router::new()
        .route(
            console_protocol::tools::ROUTE_TOOL_LAUNCH,
            get(launch_exchange),
        )
        .route(console_protocol::tools::ROUTE_TOOL_CONTEXT, get(context))
        .fallback(fallback)
        .with_state(state.clone())
        .layer(from_fn_with_state(state, hardening))
}

/// Hardening on every tool-origin response. The CSP is *appended*: a
/// tool's own CSP is kept, and two policies intersect, so a tool can only
/// tighten it.
// COMPLIANCE MAPPING:
// - NIST 800-53: SC-18 (Mobile Code), AC-4 — frame-ancestors = console only; base-uri/object-src locked
// - NIST 800-53: SC-8 (Transmission Confidentiality) — gateway HSTS on tool origins
pub async fn hardening(State(s): State<Shared>, req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let csp = format!(
        "frame-ancestors {}; base-uri 'self'; object-src 'none'",
        s.cfg.public_origin()
    );
    let h = res.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&csp) {
        h.append(header::CONTENT_SECURITY_POLICY, v);
    }
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::STRICT_TRANSPORT_SECURITY,
        HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );
    if !h.contains_key(header::REFERRER_POLICY) {
        h.insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        );
    }
    if !h.contains_key("cross-origin-resource-policy") {
        h.insert(
            "cross-origin-resource-policy",
            HeaderValue::from_static("same-site"),
        );
    }
    res
}

fn tool_of(req: &Request) -> Option<Arc<Tool>> {
    req.extensions().get::<ToolHost>().map(|t| t.0.clone())
}

// ---------------------------------------------------------------------------
// Error pages (tool origin): JSON for XHR (INT-007), else small HTML.
// ---------------------------------------------------------------------------

fn attr(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn page(s: &Shared, status: StatusCode, title: &str, body: &str) -> Response {
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"color-scheme\" content=\"light dark\">\
         <link rel=\"stylesheet\" href=\"{}/auth.css\">\
         <title>{title} - USG Ops Console</title></head>\
         <body><main><h1>{title}</h1>{body}</main></body></html>",
        attr(&s.cfg.public_origin())
    );
    (
        status,
        [
            no_store(),
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
        ],
        html,
    )
        .into_response()
}

/// 401 on a tool origin: never a redirect to `/login` (the shell owns
/// re-login); the HTML page links back to the tool inside the console.
fn tool_unauthenticated(
    s: &Shared,
    tool: &Tool,
    headers: &HeaderMap,
    reason: AuthErrorReason,
    clear: bool,
) -> Response {
    let mut res = if wants_json(headers) {
        let body = AuthErrorBody {
            error: AuthErrorKind::Unauthenticated,
            reason: Some(reason),
            login_url: None,
        };
        let mut r = (StatusCode::UNAUTHORIZED, [no_store()], Json(body)).into_response();
        r.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Session realm=\"usg-ops-console-tool\""),
        );
        r
    } else {
        let link = format!("{}/tools/{}/", s.cfg.public_origin(), tool.cfg.id);
        page(
            s,
            StatusCode::UNAUTHORIZED,
            "Open this tool from the console",
            &format!(
                "<p>This tool session is not active. Open the tool from the console to continue.</p>\
                 <p><a href=\"{}\" target=\"_top\">Open {} in the console</a></p>",
                attr(&link),
                attr(&tool.cfg.title)
            ),
        )
    };
    if clear {
        append_cookie(&mut res, &session::clear_cookie(TOOL_SESSION_COOKIE));
    }
    res
}

fn tool_error(
    s: &Shared,
    headers: &HeaderMap,
    status: StatusCode,
    kind: AuthErrorKind,
) -> Response {
    if wants_json(headers) {
        return json_error(status, kind);
    }
    let (title, body) = match kind {
        AuthErrorKind::Forbidden => (
            "Not authorized",
            "Your account is not authorized to use this tool. To request access, contact your administrator.",
        ),
        AuthErrorKind::GatewayTimeout => (
            "Tool did not respond",
            "The tool took too long to respond. Try again in a moment.",
        ),
        AuthErrorKind::BadGateway => (
            "Tool unavailable",
            "The tool could not be reached. Try again in a moment.",
        ),
        AuthErrorKind::Unavailable => (
            "Tool busy",
            "The tool is handling too many requests. Try again in a moment.",
        ),
        AuthErrorKind::PayloadTooLarge => ("Request too large", "The request was too large."),
        AuthErrorKind::NotImplemented => ("Not supported", "This request type is not supported."),
        AuthErrorKind::MisdirectedRequest => (
            "Wrong tool address",
            "This address belongs to a different tool.",
        ),
        _ => ("Not found", "There is nothing here."),
    };
    page(s, status, title, &format!("<p>{body}</p>"))
}

// ---------------------------------------------------------------------------
// Tool session gate
// ---------------------------------------------------------------------------

/// Authenticates a tool-origin request: `__Host-tool_sid` joined to a live
/// console session, rechecked on every request, never extending idle.
// COMPLIANCE MAPPING:
// - NIST 800-53: AC-3 (Access Enforcement) — every tool request needs a tool session bound to a live console session
// - NIST 800-53: AC-11/AC-12 — console idle and absolute deadlines apply; tool traffic never extends them
async fn tool_session(
    s: &Shared,
    tool: &Tool,
    headers: &HeaderMap,
    ext: &Extensions,
) -> Result<(Arc<Session>, Vec<u8>), Box<Response>> {
    let Some(hash) = session::read_cookie(headers, TOOL_SESSION_COOKIE)
        .as_deref()
        .and_then(session::hash_cookie_value)
    else {
        return Err(Box::new(tool_unauthenticated(
            s,
            tool,
            headers,
            AuthErrorReason::NoSession,
            false,
        )));
    };
    let sess = match s.store.load_tool_session(&hash, &tool.cfg.id).await {
        Ok(Some(sess)) => sess,
        Ok(None) => {
            return Err(Box::new(tool_unauthenticated(
                s,
                tool,
                headers,
                AuthErrorReason::NoSession,
                true,
            )));
        }
        Err(e) => {
            tracing::error!(error = %e, "tool session lookup failed");
            return Err(Box::new(tool_error(
                s,
                headers,
                StatusCode::SERVICE_UNAVAILABLE,
                AuthErrorKind::Unavailable,
            )));
        }
    };
    if let Some(reason) = s.end_if_expired(&sess, headers, ext).await {
        return Err(Box::new(tool_unauthenticated(
            s, tool, headers, reason, true,
        )));
    }
    Ok((Arc::new(sess), hash))
}

fn tool_event(
    s: &Shared,
    m: &Meta,
    tool: &Tool,
    sess: Option<&Session>,
    (t, o): (EventType, Outcome),
    reason: &'static str,
    path: &str,
) -> AuditEvent {
    let mut ev = match sess {
        Some(sess) => s.session_event(m, sess, t, o, reason),
        None => {
            let mut ev = s.event(m, t, o);
            ev.reason = Some(reason);
            ev
        }
    };
    ev.tool_id = Some(tool.cfg.id.clone());
    ev.path = Some(path_only(path));
    ev
}

/// Audit an unauthenticated tool-origin denial, at most
/// `DENIAL_AUDIT_PER_MINUTE` per client IP (tracing always logs). The DB
/// row is written off the response path (PERF-002).
async fn audit_unauth_denial(s: &Shared, m: &Meta, mut ev: AuditEvent) {
    match s.tools.denial_audit.check(m.ip, Instant::now()) {
        Decision::Allow => s.audit.record_background(ev),
        Decision::Limited { .. } => {
            ev.reason = ev.reason.or(Some("rate_limited"));
            tracing::warn!(
                tool = ev.tool_id.as_deref(),
                reason = ev.reason,
                "tool-origin denial (audit rate-limited)"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// SEC-001: the code exchange must be the shell's tool iframe loading the
/// launch URL, not a top-level navigation or a cross-site embed (otherwise an
/// attacker could plant their own tool session in a victim's browser). A
/// browser that sends Fetch Metadata must send `Sec-Fetch-Dest: iframe` and
/// `Sec-Fetch-Site: same-site|same-origin` (console and tool origins are
/// siblings under one registrable domain; SEC-010: that domain must be
/// dedicated to the console and its tools, since `same-site` covers every
/// host under it — see `Config::public_url`). Without Fetch Metadata, a
/// `Referer` whose origin is the console origin is required. A `Referer`
/// from any other origin is refused in both cases.
// COMPLIANCE MAPPING:
// - NIST 800-53: SC-23 (Session Authenticity) — launch code redeemable only from the console's tool frame (no login CSRF / fixation)
pub(crate) fn launch_context_ok(h: &HeaderMap, console_origin: &str) -> bool {
    let get = |n: &str| h.get(n).and_then(|v| v.to_str().ok());
    let referer_origin = get("referer").map(|r| {
        url::Url::parse(r)
            .map(|u| u.origin().ascii_serialization())
            .unwrap_or_default()
    });
    if referer_origin
        .as_deref()
        .is_some_and(|o| o != console_origin)
    {
        return false;
    }
    match (get("sec-fetch-site"), get("sec-fetch-dest")) {
        (None, None) => referer_origin.is_some(),
        (Some(site), Some(dest)) => {
            dest.eq_ignore_ascii_case("iframe")
                && (site.eq_ignore_ascii_case("same-site")
                    || site.eq_ignore_ascii_case("same-origin"))
        }
        _ => false,
    }
}

/// `GET /_console/launch?code=`: single-use exchange for `__Host-tool_sid`,
/// then 303 to `/t/<id><path>`. Bad, reused, expired or cross-tool codes
/// get the 401 page. The query (code) is never logged: the request span
/// records the path only. The request must come from the console's tool
/// iframe ([`launch_context_ok`]); otherwise 401 and the code is left
/// unconsumed (it expires in 30 s).
// COMPLIANCE MAPPING:
// - NIST 800-53: SC-23 (Session Authenticity) — no fixation: tool session minted server-side after a single-use code
// - NIST 800-53: AU-2 — bad codes audited as tool.denied (rate-bounded)
pub async fn launch_exchange(
    State(s): State<Shared>,
    RawQuery(query): RawQuery,
    req: Request,
) -> Response {
    let Some(tool) = tool_of(&req) else {
        return json_error(StatusCode::NOT_FOUND, AuthErrorKind::NotFound);
    };
    let headers = req.headers();
    let m = s.meta(headers, req.extensions());
    let deny = |why: &'static str| {
        tool_event(
            &s,
            &m,
            &tool,
            None,
            (EventType::ToolDenied, Outcome::Denied),
            why,
            console_protocol::tools::ROUTE_TOOL_LAUNCH,
        )
    };
    if let Decision::Limited { .. } = s.tools.code_limiter.check(m.ip, Instant::now()) {
        let mut res = tool_error(
            &s,
            headers,
            StatusCode::TOO_MANY_REQUESTS,
            AuthErrorKind::RateLimited,
        );
        res.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
        return res;
    }
    if !launch_context_ok(headers, &s.cfg.public_origin()) {
        tracing::warn!(tool = %tool.cfg.id, "launch code presented outside the console tool frame");
        audit_unauth_denial(&s, &m, deny("launch_context")).await;
        return no_referrer(tool_unauthenticated(
            &s,
            &tool,
            headers,
            AuthErrorReason::NoSession,
            false,
        ));
    }
    let code = query.as_deref().filter(|q| q.len() <= 512).and_then(|q| {
        url::form_urlencoded::parse(q.as_bytes())
            .find(|(k, _)| k == "code")
            .map(|(_, v)| v.into_owned())
    });
    let Some(code_hash) = code.as_deref().and_then(session::hash_cookie_value) else {
        audit_unauth_denial(&s, &m, deny("bad_code")).await;
        return no_referrer(tool_unauthenticated(
            &s,
            &tool,
            headers,
            AuthErrorReason::NoSession,
            false,
        ));
    };
    let lc = match s.store.take_launch_code(&code_hash, s.clock.now()).await {
        Ok(Some(lc)) if lc.tool_id == tool.cfg.id => lc,
        Ok(_) => {
            audit_unauth_denial(&s, &m, deny("bad_code")).await;
            return no_referrer(tool_unauthenticated(
                &s,
                &tool,
                headers,
                AuthErrorReason::NoSession,
                false,
            ));
        }
        Err(e) => {
            tracing::error!(error = %e, "launch code lookup failed");
            return tool_error(
                &s,
                headers,
                StatusCode::SERVICE_UNAVAILABLE,
                AuthErrorKind::Unavailable,
            );
        }
    };
    let sess = match s.store.load(&lc.session_hash).await {
        Ok(Some(sess)) => sess,
        Ok(None) => {
            audit_unauth_denial(&s, &m, deny("bad_code")).await;
            return no_referrer(tool_unauthenticated(
                &s,
                &tool,
                headers,
                AuthErrorReason::NoSession,
                false,
            ));
        }
        Err(e) => {
            tracing::error!(error = %e, "session lookup failed");
            return tool_error(
                &s,
                headers,
                StatusCode::SERVICE_UNAVAILABLE,
                AuthErrorKind::Unavailable,
            );
        }
    };
    if let Some(reason) = s.end_if_expired(&sess, headers, req.extensions()).await {
        return no_referrer(tool_unauthenticated(&s, &tool, headers, reason, true));
    }
    if !tool.cfg.permits(&sess.roles) {
        let ev = tool_event(
            &s,
            &m,
            &tool,
            Some(&sess),
            (EventType::ToolDenied, Outcome::Denied),
            "missing_role",
            &lc.path,
        );
        // SEC-011: queued, never awaited, so this 404 takes the same time as
        // the unknown-id 404 (no DB round trip distinguishes them).
        s.audit.record_background(ev);
        // INT-009: same answer as an unknown tool.
        return tool_error(&s, headers, StatusCode::NOT_FOUND, AuthErrorKind::NotFound);
    }
    let Ok((sid, sid_hash)) = session::new_opaque_id() else {
        return tool_error(
            &s,
            headers,
            StatusCode::SERVICE_UNAVAILABLE,
            AuthErrorKind::Unavailable,
        );
    };
    match s
        .store
        .insert_tool_session(&sid_hash, &sess.id_hash, &tool.cfg.id, s.clock.now())
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return no_referrer(tool_unauthenticated(
                &s,
                &tool,
                headers,
                AuthErrorReason::NoSession,
                true,
            ));
        }
        Err(e) => {
            tracing::error!(error = %e, "tool session insert failed");
            return tool_error(
                &s,
                headers,
                StatusCode::SERVICE_UNAVAILABLE,
                AuthErrorKind::Unavailable,
            );
        }
    }
    // `lc.path` passed `valid_path` at launch: always "/..." (no `//`).
    let target = format!("/t/{}{}", tool.cfg.id, lc.path);
    let mut res = redirect(StatusCode::SEE_OTHER, &target);
    append_cookie(
        &mut res,
        &session::set_cookie_strict(TOOL_SESSION_COOKIE, sid, None),
    );
    no_referrer(res)
}

fn no_referrer(mut res: Response) -> Response {
    res.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    res
}

/// `GET /_console/context`: the SDK's console-origin pin.
pub async fn context(State(s): State<Shared>, req: Request) -> Response {
    let Some(tool) = tool_of(&req) else {
        return json_error(StatusCode::NOT_FOUND, AuthErrorKind::NotFound);
    };
    if let Err(res) = tool_session(&s, &tool, req.headers(), req.extensions()).await {
        return *res;
    }
    let body = ToolContext {
        tool_id: tool.cfg.id.clone(),
        console_origin: s.cfg.public_origin(),
        protocol: PROTOCOL_VERSION,
    };
    ([no_store()], Json(body)).into_response()
}

/// `/t/*` → proxy; anything else on a tool origin → 404.
pub async fn fallback(State(s): State<Shared>, req: Request) -> Response {
    let Some(tool) = tool_of(&req) else {
        return json_error(StatusCode::NOT_FOUND, AuthErrorKind::NotFound);
    };
    let path = req.uri().path().to_owned();
    let Some(rest) = path.strip_prefix("/t/") else {
        return tool_error(
            &s,
            req.headers(),
            StatusCode::NOT_FOUND,
            AuthErrorKind::NotFound,
        );
    };
    let target_id = rest.split('/').next().unwrap_or_default();
    if target_id != tool.cfg.id {
        // Another tool's path on this origin (or garbage): never proxied.
        let m = s.meta(req.headers(), req.extensions());
        let ev = tool_event(
            &s,
            &m,
            &tool,
            None,
            (EventType::ToolDenied, Outcome::Denied),
            "misdirected",
            &path,
        );
        audit_unauth_denial(&s, &m, ev).await;
        return tool_error(
            &s,
            req.headers(),
            StatusCode::MISDIRECTED_REQUEST,
            AuthErrorKind::MisdirectedRequest,
        );
    }
    if rest == tool.cfg.id {
        // `/t/<id>` → `/t/<id>/` (keeps the query).
        let q = req
            .uri()
            .query()
            .map(|q| format!("?{q}"))
            .unwrap_or_default();
        return redirect(
            StatusCode::PERMANENT_REDIRECT,
            &format!("/t/{}/{q}", tool.cfg.id),
        );
    }
    proxy_request(s, tool, req).await
}

/// Navigations as the plan defines them for audit: `Sec-Fetch-Mode:
/// navigate`, or no `Sec-Fetch-Mode` at all (non-browser clients).
fn audited_navigation(method: &Method, h: &HeaderMap) -> bool {
    match h.get("sec-fetch-mode") {
        Some(v) => v.as_bytes() == b"navigate",
        None => is_navigation(method, h) || !h.contains_key("sec-fetch-site"),
    }
}

fn unsafe_method(m: &Method) -> bool {
    !matches!(
        *m,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

// COMPLIANCE MAPPING:
// - NIST 800-53: AC-3 (Access Enforcement) — per-request role check at the proxy (403 + tool.denied)
// - NIST 800-53: AC-4 (Information Flow Enforcement) — header allow-list; identity only via the signed assertion
// - NIST 800-53: IA-2, IA-9 — ES256 assertion per request (iss/aud/sub/roles/ctx/sid/jti/exp)
// - NIST 800-53: AU-2/AU-3/AU-12 — tool.access per navigation, tool.request for audit_writes tools (DB row via the bounded background queue, PERF-002)
// - NIST 800-53: SC-5 — body cap, concurrency cap, timeouts
async fn proxy_request(s: Shared, tool: Arc<Tool>, req: Request) -> Response {
    let path = req.uri().path().to_owned();
    if !proxy::path_ok(&path) {
        return bad_request();
    }
    let (sess, tool_sid_hash) = match tool_session(&s, &tool, req.headers(), req.extensions()).await
    {
        Ok(v) => v,
        Err(res) => return *res,
    };
    let m = s.meta(req.headers(), req.extensions());
    if !tool.cfg.permits(&sess.roles) {
        let mut ev = tool_event(
            &s,
            &m,
            &tool,
            Some(&sess),
            (EventType::ToolDenied, Outcome::Denied),
            "missing_role",
            &path,
        );
        ev.http_status = Some(403);
        s.audit.record_background(ev);
        return tool_error(
            &s,
            req.headers(),
            StatusCode::FORBIDDEN,
            AuthErrorKind::Forbidden,
        );
    }
    // POAM: SC-7 - WebSocket upgrades are not proxied in M4 (501); revisit with finch in M5 (brief Q2).
    if proxy::is_upgrade(req.method(), req.headers()) {
        return tool_error(
            &s,
            req.headers(),
            StatusCode::NOT_IMPLEMENTED,
            AuthErrorKind::NotImplemented,
        );
    }
    if (proxy::spoof_attempt(req.headers())
        || req
            .extensions()
            .get::<crate::headers::SpoofedIdentity>()
            .is_some())
        && let Decision::Allow = s
            .tools
            .spoof_audit
            .check_key(tool_sid_hash.clone(), Instant::now())
    {
        let ev = tool_event(
            &s,
            &m,
            &tool,
            Some(&sess),
            (EventType::ToolDenied, Outcome::Denied),
            "identity_header_stripped",
            &path,
        );
        s.audit.record_background(ev);
    }
    let Some(keys) = s.tools.keys() else {
        // Unreachable by construction (ToolsState::new requires keys).
        return tool_error(
            &s,
            req.headers(),
            StatusCode::SERVICE_UNAVAILABLE,
            AuthErrorKind::Unavailable,
        );
    };

    // Assertion: per request, after authz. Not cached (PERF-006): signing
    // costs 30-60 us (< 10% of proxy overhead) and a cached token would
    // repeat its `jti`, which tools may use for replay detection. Revisit
    // only if `assertion.sign` p99 exceeds 200 us.
    let now = u64::try_from(s.clock.now().timestamp()).unwrap_or(0);
    let ctx = req
        .headers()
        .get(SCOPE_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_scope_header(v, &tool.cfg.scopes))
        .unwrap_or_else(Scope::default);
    let Ok(jti) = new_jti() else {
        return tool_error(
            &s,
            req.headers(),
            StatusCode::SERVICE_UNAVAILABLE,
            AuthErrorKind::Unavailable,
        );
    };
    let claims = AssertionClaims {
        iss: s.cfg.public_origin(),
        aud: tool.cfg.id.clone(),
        sub: sess.user_sub.clone(),
        preferred_username: sess.username.clone(),
        name: sess.display_name.clone(),
        roles: tool.cfg.forwarded_roles(&sess.roles),
        ctx,
        sid: keys.sid(&sess.id_hash, &tool.cfg.id),
        jti,
        iat: now,
        nbf: now,
        exp: now + console_protocol::tools::ASSERTION_TTL_SECS,
        acr: sess.acr.clone(),
    };
    // F5: timed like `session.lookup`, so the PERF-006 trigger (p99 > 200 us)
    // can be read from debug logs without span-close events.
    let token = {
        let _span = tracing::debug_span!("assertion.sign", tool = %tool.cfg.id).entered();
        let t0 = std::time::Instant::now();
        let token = keys.sign(&claims);
        tracing::debug!(
            sign_us = u64::try_from(t0.elapsed().as_micros()).unwrap_or(u64::MAX),
            "assertion signed"
        );
        token
    };
    let Ok(token) = token.map_err(|e| tracing::error!(error = %e, "assertion signing failed"))
    else {
        return tool_error(
            &s,
            req.headers(),
            StatusCode::SERVICE_UNAVAILABLE,
            AuthErrorKind::Unavailable,
        );
    };

    let mut up = proxy::upstream_request_headers(req.headers(), &tool.cfg.forward_headers);
    let mut add = |name: &'static str, v: &str| {
        if let Ok(v) = HeaderValue::from_str(v) {
            up.insert(name, v);
        }
    };
    add(ASSERTION_HEADER, &token);
    if let Some(ip) = m.ip {
        add("x-forwarded-for", &ip.to_string());
    }
    add("x-forwarded-proto", "https");
    add("x-forwarded-host", &tool.cfg.host);
    add("x-request-id", &m.request_id);

    let method = req.method().clone();
    let nav = audited_navigation(&method, req.headers());
    let audit_kind = if tool.cfg.audit_writes && unsafe_method(&method) {
        Some(EventType::ToolRequest)
    } else if nav {
        Some(EventType::ToolAccess)
    } else {
        None
    };
    // PERF-005: only the error-page format is needed after `req` moves.
    let json_errors = wants_json(req.headers());
    let result = proxy::forward(&tool.cfg, &tool.upstream, req, up).await;
    let (res, status, latency) = match result {
        Ok(f) => {
            let st = f.response.status();
            (f.response, st, Some(f.ttfb))
        }
        Err(e) => {
            let (st, kind) = match e {
                ForwardError::TooLarge => (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    AuthErrorKind::PayloadTooLarge,
                ),
                ForwardError::Busy => (StatusCode::SERVICE_UNAVAILABLE, AuthErrorKind::Unavailable),
                ForwardError::BadGateway => (StatusCode::BAD_GATEWAY, AuthErrorKind::BadGateway),
                ForwardError::Timeout => {
                    (StatusCode::GATEWAY_TIMEOUT, AuthErrorKind::GatewayTimeout)
                }
            };
            let mut res = if json_errors {
                json_error(st, kind)
            } else {
                tool_error(&s, &HeaderMap::new(), st, kind)
            };
            if e == ForwardError::Busy {
                res.headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            }
            (res, st, None)
        }
    };
    if let Some(kind) = audit_kind {
        let outcome = if status.is_server_error() {
            Outcome::Failure
        } else {
            Outcome::Success
        };
        let reason = if kind == EventType::ToolRequest {
            "write"
        } else {
            "navigation"
        };
        let mut ev = tool_event(&s, &m, &tool, Some(&sess), (kind, outcome), reason, &path);
        ev.http_status = Some(status.as_u16());
        ev.latency_ms = latency.map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        s.audit.record_background(ev);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.insert(*k, HeaderValue::from_static(v));
        }
        m
    }

    #[test]
    fn navigation_audit_classification() {
        assert!(audited_navigation(
            &Method::GET,
            &h(&[("sec-fetch-mode", "navigate")])
        ));
        assert!(audited_navigation(
            &Method::POST,
            &h(&[("sec-fetch-mode", "navigate")])
        ));
        assert!(!audited_navigation(
            &Method::GET,
            &h(&[
                ("sec-fetch-mode", "cors"),
                ("sec-fetch-site", "same-origin")
            ])
        ));
        assert!(
            audited_navigation(&Method::GET, &HeaderMap::new()),
            "non-browser client"
        );
        assert!(unsafe_method(&Method::POST) && !unsafe_method(&Method::GET));
    }

    #[test]
    fn launch_context_requires_console_iframe() {
        let console = "https://console.ops.example.mil";
        let ok = |pairs: &[(&'static str, &'static str)]| launch_context_ok(&h(pairs), console);
        assert!(ok(&[
            ("sec-fetch-dest", "iframe"),
            ("sec-fetch-site", "same-site")
        ]));
        assert!(ok(&[
            ("sec-fetch-dest", "iframe"),
            ("sec-fetch-site", "same-origin")
        ]));
        assert!(
            ok(&[("referer", "https://console.ops.example.mil/tools/hello/")]),
            "no Fetch Metadata: console Referer"
        );
        for bad in [
            // SEC-001: top-level navigation from a link (mail, chat, ...).
            &[("sec-fetch-dest", "document"), ("sec-fetch-site", "none")][..],
            &[
                ("sec-fetch-dest", "document"),
                ("sec-fetch-site", "same-site"),
            ][..],
            &[
                ("sec-fetch-dest", "iframe"),
                ("sec-fetch-site", "cross-site"),
            ][..],
            &[("sec-fetch-dest", "iframe")][..],
            &[("sec-fetch-site", "same-site")][..],
            &[][..],
            &[("referer", "https://evil.example/")][..],
            &[
                ("sec-fetch-dest", "iframe"),
                ("sec-fetch-site", "same-site"),
                ("referer", "https://evil.tools.ops.example.mil/"),
            ][..],
            &[("referer", "not a url")][..],
        ] {
            assert!(!ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn hosts_normalise() {
        let req = Request::builder()
            .uri("/t/x/")
            .header(header::HOST, "Hello.Tools.Example.Mil:443")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            request_host(&req).as_deref(),
            Some("hello.tools.example.mil")
        );
    }
}
