//! HTTP surface: public auth endpoints, the `require_session` gate, and the
//! session API. Contract: `.agent-runs/.../plan.md` section 4, plus the
//! round-1 CONTRACT DELTA in `fixes-round-1-gateway.md`.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{ConnectInfo, RawQuery, Request, State};
use axum::http::{Extensions, HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use chrono::{DateTime, SecondsFormat, Utc};
use console_protocol::{
    AuthErrorBody, AuthErrorKind, AuthErrorReason, LogoutResponse, SessionInfo, SessionUser,
};
use openidconnect::PkceCodeVerifier;
use subtle::ConstantTimeEq;
use url::form_urlencoded;

use crate::audit::{AuditEvent, AuditSink, EventType, Outcome};
use crate::config::Config;
use crate::headers::no_store;
use crate::oidc::{OidcClient, RefreshFailure, filter_roles_with, has_role};
use crate::rate_limit::{Decision, RateLimiter};
use crate::registry::ToolsState;
use crate::session::{
    self, AUTH_COOKIE, AUTH_REQUEST_TTL, AuthRequest, Clock, Expiry, NewSession, REAUTH_COOKIE,
    Refreshed, SID_COOKIE, Session, SessionStore,
};
use crate::{client_ip, csrf, return_to};

pub const LOGIN_URL: &str = "/login";
/// Public (no session) paths besides the configured callback path.
pub const PUBLIC_PATHS: [&str; 4] = ["/healthz", "/login", "/logged-out", "/auth.css"];
const MAX_QUERY: usize = 4096;
const MAX_FORM_BODY: usize = 4096;
const MAX_UA: usize = 256;
/// `session.refresh` is audited at most once per this window per session.
const REFRESH_AUDIT_EVERY: chrono::Duration = chrono::Duration::seconds(60);
/// Tolerated IdP/gateway clock difference for the `auth_time` floor.
const AUTH_TIME_SKEW: chrono::Duration = chrono::Duration::seconds(30);

pub struct AppState {
    pub cfg: Arc<Config>,
    pub store: SessionStore,
    pub audit: AuditSink,
    pub oidc: Arc<OidcClient>,
    pub clock: Arc<dyn Clock>,
    pub login_limiter: RateLimiter,
    /// M4 registry, assertion keys and tool rate limits.
    pub tools: Arc<ToolsState>,
}

pub type Shared = Arc<AppState>;

/// Where/who-agent/request id for audit records.
pub(crate) struct Meta {
    pub(crate) ip: Option<IpAddr>,
    pub(crate) user_agent: Option<String>,
    pub(crate) request_id: String,
}

impl AppState {
    pub(crate) fn meta(&self, headers: &HeaderMap, ext: &Extensions) -> Meta {
        let peer = ext.get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());
        let request_id = session::random_bytes::<8>()
            .map(|b| b.iter().map(|x| format!("{x:02x}")).collect())
            .unwrap_or_else(|_| "unknown".into());
        Meta {
            ip: client_ip::resolve(peer, headers, &self.cfg.trusted_proxies),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(|v| session::truncate(v, MAX_UA)),
            request_id,
        }
    }

    pub(crate) fn event(&self, m: &Meta, t: EventType, o: Outcome) -> AuditEvent {
        let mut ev = AuditEvent::new(self.clock.now(), t, o);
        ev.client_ip = m.ip;
        ev.user_agent.clone_from(&m.user_agent);
        ev.request_id = Some(m.request_id.clone());
        ev
    }

    /// An event about an existing session (who + correlation handle).
    pub(crate) fn session_event(
        &self,
        m: &Meta,
        sess: &Session,
        t: EventType,
        o: Outcome,
        reason: &'static str,
    ) -> AuditEvent {
        let mut ev = self.event(m, t, o);
        ev.user_sub = Some(sess.user_sub.clone());
        ev.username = Some(sess.username.clone());
        ev.session_ref = Some(session::session_ref(&sess.id_hash));
        ev.reason = Some(reason);
        ev
    }

    pub(crate) fn reauth_ttl(&self) -> Duration {
        self.cfg.session_absolute
    }

    /// Session roles kept: `console-*` plus any role a registry tool uses.
    fn filter_roles(&self, roles: &[String]) -> Vec<String> {
        filter_roles_with(roles, |r| self.tools.keeps_role(r))
    }

    /// Ends `sess` if it is past its idle (row limit, POAM-031) or absolute
    /// deadline: deletes the row (tool sessions cascade) and audits the
    /// expiry if this request did the delete. `None` while live.
    pub(crate) async fn end_if_expired(
        &self,
        sess: &Session,
        headers: &HeaderMap,
        ext: &Extensions,
    ) -> Option<AuthErrorReason> {
        let now = self.clock.now();
        let state = session::expiry(
            sess.last_seen_at,
            sess.absolute_expires_at,
            sess.idle(self.cfg.session_idle),
            now,
        );
        let (reason, ev_type) = match state {
            Expiry::Live => return None,
            Expiry::Absolute => (AuthErrorReason::AbsoluteTimeout, EventType::ExpiredAbsolute),
            Expiry::Idle => (AuthErrorReason::IdleTimeout, EventType::ExpiredIdle),
        };
        // Only the request that actually deletes the row audits it (the
        // sweeper may race us).
        match self.store.delete(&sess.id_hash).await {
            Ok(Some(_)) => {
                let m = self.meta(headers, ext);
                let ev = self.session_event(&m, sess, ev_type, Outcome::Success, "on_access");
                self.audit.record_best_effort(&ev).await;
            }
            Ok(None) => {}
            Err(e) => tracing::error!(error = %e, "expired session delete failed"),
        }
        Some(reason)
    }
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// `GET` + (`Sec-Fetch-Mode: navigate`, or no `Sec-Fetch-Mode` and an
/// `Accept` that includes `text/html`).
pub fn is_navigation(method: &Method, headers: &HeaderMap) -> bool {
    if method != Method::GET {
        return false;
    }
    match headers.get("sec-fetch-mode") {
        Some(v) => v.as_bytes() == b"navigate",
        None => headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/html")),
    }
}

/// `Accept` names `application/json` (INT-005: an XHR/fetch client).
pub fn wants_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("application/json"))
}

pub fn auth_error_json(
    status: StatusCode,
    kind: AuthErrorKind,
    reason: AuthErrorReason,
) -> Response {
    let body = AuthErrorBody {
        error: kind,
        reason: Some(reason),
        login_url: Some(LOGIN_URL.into()),
    };
    let mut res = (status, [no_store()], Json(body)).into_response();
    if status == StatusCode::UNAUTHORIZED {
        res.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Session realm=\"usg-ops-console\""),
        );
    }
    res
}

/// Non-auth gateway errors in the same JSON shape (`{"error": ...}`), INT-004.
pub fn json_error(status: StatusCode, kind: AuthErrorKind) -> Response {
    let body = AuthErrorBody {
        error: kind,
        reason: None,
        login_url: None,
    };
    (status, [no_store()], Json(body)).into_response()
}

fn reason_code(r: AuthErrorReason) -> &'static str {
    match r {
        AuthErrorReason::CsrfToken => "csrf_token",
        AuthErrorReason::Origin => "origin",
        _ => "other",
    }
}

/// The unauthenticated contract: navigations get a 302 to `/login` with a
/// validated `return_to` (plus `reason=expired` when a session ended);
/// everything else gets the 401 JSON. `ended` (a presented session is dead)
/// clears the sid and sets the reauth marker for that long.
fn unauthenticated(
    method: &Method,
    headers: &HeaderMap,
    path_and_query: &str,
    reason: AuthErrorReason,
    ended: Option<Duration>,
) -> Response {
    let mut res = if is_navigation(method, headers) {
        let mut location = if return_to::is_safe(path_and_query) && path_and_query != "/" {
            let enc: String = form_urlencoded::byte_serialize(path_and_query.as_bytes()).collect();
            format!("{LOGIN_URL}?return_to={enc}")
        } else {
            LOGIN_URL.to_owned()
        };
        if matches!(
            reason,
            AuthErrorReason::IdleTimeout
                | AuthErrorReason::AbsoluteTimeout
                | AuthErrorReason::Revoked
        ) {
            location.push(if location.contains('?') { '&' } else { '?' });
            location.push_str("reason=expired");
        }
        redirect(StatusCode::FOUND, &location)
    } else {
        auth_error_json(
            StatusCode::UNAUTHORIZED,
            AuthErrorKind::Unauthenticated,
            reason,
        )
    };
    if let Some(ttl) = ended {
        append_cookie(&mut res, &session::clear_cookie(SID_COOKIE));
        append_cookie(&mut res, &session::reauth_cookie(ttl));
    }
    res
}

pub(crate) fn redirect(status: StatusCode, location: &str) -> Response {
    let mut res = (status, [no_store()]).into_response();
    if let Ok(v) = HeaderValue::from_str(location) {
        res.headers_mut().insert(header::LOCATION, v);
    }
    res
}

pub(crate) fn append_cookie(res: &mut Response, cookie: &str) {
    if let Ok(v) = HeaderValue::from_str(cookie) {
        res.headers_mut().append(header::SET_COOKIE, v);
    }
}

/// `503`: an HTML page for navigations, `{"error":"unavailable"}` otherwise.
pub(crate) fn unavailable(method: &Method, headers: &HeaderMap) -> Response {
    if is_navigation(method, headers) {
        pages::unavailable()
    } else {
        json_error(StatusCode::SERVICE_UNAVAILABLE, AuthErrorKind::Unavailable)
    }
}

fn with_retry_after(mut res: Response) -> Response {
    res.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
    res
}

// ---------------------------------------------------------------------------
// require_session
// ---------------------------------------------------------------------------

// COMPLIANCE MAPPING:
// - NIST 800-53: AC-3 (Access Enforcement) — every non-public route requires a live server-side session
// - NIST 800-53: AC-11 (Device Lock) — idle expiry enforced on every request; reads never extend it
// - NIST 800-53: AC-12 (Session Termination) — absolute expiry; expired rows deleted and audited
pub async fn require_session(State(s): State<Shared>, mut req: Request, next: Next) -> Response {
    let pq = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "/".into());
    let method = req.method().clone();
    let headers = req.headers();
    let Some(hash) = session::read_cookie(headers, SID_COOKIE)
        .as_deref()
        .and_then(session::hash_cookie_value)
    else {
        return unauthenticated(&method, headers, &pq, AuthErrorReason::NoSession, None);
    };
    let sess = match s.store.load(&hash).await {
        Ok(Some(sess)) => sess,
        Ok(None) => {
            // A well-formed sid that no longer exists ended in this browser
            // (expired and swept, logged out elsewhere): require fresh auth.
            return unauthenticated(
                &method,
                headers,
                &pq,
                AuthErrorReason::NoSession,
                Some(s.reauth_ttl()),
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "session lookup failed");
            return unavailable(&method, headers);
        }
    };
    if let Some(reason) = s.end_if_expired(&sess, headers, req.extensions()).await {
        return unauthenticated(&method, req.headers(), &pq, reason, Some(s.reauth_ttl()));
    }
    req.extensions_mut().insert(Arc::new(sess));
    next.run(req).await
}

fn session_ext(req: &Request) -> Option<Arc<Session>> {
    req.extensions().get::<Arc<Session>>().cloned()
}

fn session_info(s: &AppState, sess: &Session, last_seen: DateTime<Utc>) -> SessionInfo {
    // POAM-031: the row's own idle limit (this gateway's for legacy rows).
    let idle = sess.idle(s.cfg.session_idle);
    SessionInfo {
        user: SessionUser {
            id: sess.user_sub.clone(),
            username: sess.username.clone(),
            display_name: sess.display_name.clone(),
        },
        roles: sess.roles.clone(),
        idle_expires_at: rfc3339(session::idle_deadline(
            last_seen,
            sess.absolute_expires_at,
            idle,
        )),
        absolute_expires_at: rfc3339(sess.absolute_expires_at),
        idle_timeout_secs: idle.as_secs(),
        server_time: rfc3339(s.clock.now()),
        csrf_token: B64URL.encode(&sess.csrf_token),
    }
}

/// CSRF rejection: audited (NIST-009) and answered with the CSRF 403 JSON.
async fn csrf_denied(s: &AppState, m: &Meta, sess: &Session, reason: AuthErrorReason) -> Response {
    tracing::warn!(?reason, "CSRF check failed");
    let ev = s.session_event(
        m,
        sess,
        EventType::AccessDenied,
        Outcome::Denied,
        reason_code(reason),
    );
    s.audit.record_best_effort(&ev).await;
    auth_error_json(StatusCode::FORBIDDEN, AuthErrorKind::Csrf, reason)
}

// ---------------------------------------------------------------------------
// Session API
// ---------------------------------------------------------------------------

/// `GET /api/session`: read-only; does not extend `last_seen`.
pub async fn api_session(State(s): State<Shared>, req: Request) -> Response {
    let Some(sess) = session_ext(&req) else {
        return unavailable(req.method(), req.headers());
    };
    (
        [no_store()],
        Json(session_info(&s, &sess, sess.last_seen_at)),
    )
        .into_response()
}

/// Unmatched `/api/*` (authenticated): honest JSON 404, never the SPA.
pub async fn api_not_found() -> Response {
    json_error(StatusCode::NOT_FOUND, AuthErrorKind::NotFound)
}

/// `/t/*` and `/_console/*` on the console origin: tool content is never
/// served same-origin with the shell (plan.md section 0), session or not.
// COMPLIANCE MAPPING:
// - NIST 800-53: SC-39 (Process Isolation), AC-4 — tools only on their own origins
pub async fn tool_paths_not_here() -> Response {
    json_error(StatusCode::NOT_FOUND, AuthErrorKind::NotFound)
}

/// `400 {"error":"bad_request"}` ([`AuthErrorKind::BadRequest`], INT-004;
/// clients older than M4 round 1 read it as `Unknown`).
pub fn bad_request() -> Response {
    json_error(StatusCode::BAD_REQUEST, AuthErrorKind::BadRequest)
}

/// `POST /api/session/keepalive`: the only thing that extends the idle
/// window, and only after the IdP accepts a refresh grant (NIST-001).
// COMPLIANCE MAPPING:
// - NIST 800-53: AC-2 (Account Management), AC-2(3) (Disable Accounts) — IdP re-validation on every extension
// - NIST 800-53: AC-12 (Session Termination) — revoked or de-roled sessions are deleted and audited
pub async fn keepalive(State(s): State<Shared>, req: Request) -> Response {
    let Some(sess) = session_ext(&req) else {
        return unavailable(req.method(), req.headers());
    };
    let headers = req.headers();
    let m = s.meta(headers, req.extensions());
    let presented = headers.get(csrf::HEADER).and_then(|v| v.to_str().ok());
    if let Err(reason) = csrf::check(headers, presented, &sess.csrf_token, &s.cfg.public_origin()) {
        return csrf_denied(&s, &m, &sess, reason).await;
    }
    let refresh_token = match s.store.refresh_token(&sess.id_hash).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "refresh token lookup failed");
            return unavailable(req.method(), headers);
        }
    };
    let Some(refresh_token) = refresh_token else {
        return revoke(&s, &m, &sess, "no_refresh_token").await;
    };
    let identity = match s.oidc.refresh_session(&refresh_token, &sess.user_sub).await {
        Ok(i) => i,
        Err(RefreshFailure::Rejected(reason)) => return revoke(&s, &m, &sess, reason).await,
        Err(RefreshFailure::Unavailable) => {
            // Not extended, not ended: the user may retry.
            return json_error(StatusCode::SERVICE_UNAVAILABLE, AuthErrorKind::Unavailable);
        }
    };
    if !has_role(&identity.roles, &s.cfg.required_role) {
        return revoke(&s, &m, &sess, "missing_role").await;
    }
    let roles = s.filter_roles(&identity.roles);
    let now = s.clock.now();
    let refreshed = Refreshed {
        roles: &roles,
        id_token: &identity.id_token,
        refresh_token: identity.refresh_token.as_deref(),
    };
    match s
        .store
        .touch_refreshed(&sess.id_hash, now, s.cfg.session_idle, &refreshed)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return unauthenticated(
                req.method(),
                headers,
                "/",
                AuthErrorReason::NoSession,
                Some(s.reauth_ttl()),
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "keepalive update failed");
            return unavailable(req.method(), headers);
        }
    }
    if now - sess.last_seen_at >= REFRESH_AUDIT_EVERY {
        let ev = s.session_event(&m, &sess, EventType::Refresh, Outcome::Success, "keepalive");
        s.audit.record_best_effort(&ev).await;
    }
    let mut updated = (*sess).clone();
    updated.roles = roles;
    if updated.idle_timeout_secs.is_none() {
        // A legacy row adopts this gateway's limit on keepalive (`touch*`).
        updated.idle_timeout_secs = i32::try_from(s.cfg.session_idle.as_secs()).ok();
    }
    ([no_store()], Json(session_info(&s, &updated, now))).into_response()
}

/// Ends a session the IdP no longer vouches for: delete, audit
/// `session.revoked`, and answer 401 `revoked` with the reauth marker.
async fn revoke(s: &AppState, m: &Meta, sess: &Session, reason: &'static str) -> Response {
    match s.store.delete(&sess.id_hash).await {
        Ok(_) => {
            let ev = s.session_event(m, sess, EventType::Revoked, Outcome::Success, reason);
            s.audit.record_best_effort(&ev).await;
        }
        Err(e) => {
            tracing::error!(error = %e, "revoked session delete failed");
            return json_error(StatusCode::SERVICE_UNAVAILABLE, AuthErrorKind::Unavailable);
        }
    }
    let mut res = auth_error_json(
        StatusCode::UNAUTHORIZED,
        AuthErrorKind::Unauthenticated,
        AuthErrorReason::Revoked,
    );
    append_cookie(&mut res, &session::clear_cookie(SID_COOKIE));
    append_cookie(&mut res, &session::reauth_cookie(s.reauth_ttl()));
    res
}

/// `POST /logout`: CSRF (header or `csrf` form field) + Origin, then delete
/// the session and hand back the IdP end-session URL.
// COMPLIANCE MAPPING:
// - NIST 800-53: AC-12 (Session Termination) — user-initiated logout; server-side delete ends every tab
pub async fn logout(State(s): State<Shared>, req: Request) -> Response {
    let Some(sess) = session_ext(&req) else {
        return unavailable(req.method(), req.headers());
    };
    let (parts, body) = req.into_parts();
    let header_token = parts
        .headers
        .get(csrf::HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let is_form = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.starts_with("application/x-www-form-urlencoded"));
    let form_token = if header_token.is_none() && is_form {
        match axum::body::to_bytes(body, MAX_FORM_BODY).await {
            Ok(bytes) => form_urlencoded::parse(&bytes)
                .find(|(k, _)| k == "csrf")
                .map(|(_, v)| v.into_owned()),
            Err(_) => {
                return json_error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    AuthErrorKind::PayloadTooLarge,
                );
            }
        }
    } else {
        None
    };
    let xhr = header_token.is_some() || wants_json(&parts.headers);
    let presented = header_token.or(form_token);
    let m = s.meta(&parts.headers, &parts.extensions);
    if let Err(reason) = csrf::check(
        &parts.headers,
        presented.as_deref(),
        &sess.csrf_token,
        &s.cfg.public_origin(),
    ) {
        return csrf_denied(&s, &m, &sess, reason).await;
    }
    let id_token = match s.store.delete(&sess.id_hash).await {
        Ok(gone) => gone.map(|g| g.id_token),
        Err(e) => {
            tracing::error!(error = %e, "logout delete failed");
            return unavailable(&parts.method, &parts.headers);
        }
    };
    // Logout is never refused for an audit failure (that would keep the
    // session alive); the tracing line is still emitted.
    let ev = s.session_event(&m, &sess, EventType::Logout, Outcome::Success, "user");
    s.audit.record_best_effort(&ev).await;

    let url = s.oidc.end_session_url(id_token.as_deref()).await;
    let mut res = if xhr {
        let body = LogoutResponse {
            redirect: url,
            end_session_origin: s.oidc.end_session_origin().await,
        };
        ([no_store()], Json(body)).into_response()
    } else {
        redirect(StatusCode::SEE_OTHER, &url)
    };
    append_cookie(&mut res, &session::clear_cookie(SID_COOKIE));
    append_cookie(&mut res, &session::reauth_cookie(s.reauth_ttl()));
    res
}

// ---------------------------------------------------------------------------
// Login / callback
// ---------------------------------------------------------------------------

fn first_param(query: Option<&str>, name: &str) -> Option<String> {
    form_urlencoded::parse(query?.as_bytes())
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

fn random_b64<const N: usize>() -> Option<String> {
    session::random_bytes::<N>().ok().map(|b| B64URL.encode(b))
}

/// `GET /login?return_to=&reason=`: start an authorization-code + PKCE flow.
/// `prompt=login` is sent when the reauth marker is present or the shell
/// reports an expired/ended session (SEC-001).
pub async fn login(State(s): State<Shared>, RawQuery(query): RawQuery, req: Request) -> Response {
    let headers = req.headers();
    let m = s.meta(headers, req.extensions());
    match s.login_limiter.check(m.ip, Instant::now()) {
        Decision::Allow => {}
        Decision::Limited { first } => {
            if first {
                let mut ev = s.event(&m, EventType::AccessDenied, Outcome::Denied);
                ev.reason = Some("rate_limited");
                s.audit.record_best_effort(&ev).await;
            }
            let res = if is_navigation(req.method(), headers) {
                pages::rate_limited()
            } else {
                json_error(StatusCode::TOO_MANY_REQUESTS, AuthErrorKind::RateLimited)
            };
            return with_retry_after(res);
        }
    }
    let query = query.filter(|q| q.len() <= MAX_QUERY);
    let raw_return = first_param(query.as_deref(), "return_to");
    let notice = return_to::notice(first_param(query.as_deref(), "reason").as_deref());
    let mut return_to = return_to::sanitize(raw_return.as_deref());
    if let Some(n) = notice {
        return_to = return_to::with_notice(&return_to, n);
    }
    let reauth = notice.is_some() || session::read_cookie(headers, REAUTH_COOKIE).is_some();

    let (Ok((auth_cookie, auth_hash)), Some(state), Some(nonce)) = (
        session::new_opaque_id(),
        random_b64::<32>(),
        random_b64::<32>(),
    ) else {
        return unavailable(req.method(), headers);
    };
    let verifier = PkceCodeVerifier::new(match random_b64::<64>() {
        Some(v) => v,
        None => return unavailable(req.method(), headers),
    });
    let now = s.clock.now();
    let row = AuthRequest {
        state,
        nonce,
        pkce_verifier: verifier.secret().clone(),
        return_to,
        expires_at: session::add(now, AUTH_REQUEST_TTL),
        reauth,
        issued_at: Some(now),
    };
    match s.store.insert_auth_request(&auth_hash, &row, now).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!("pending sign-in cap reached");
            return with_retry_after(unavailable(req.method(), headers));
        }
        Err(e) => {
            tracing::error!(error = %e, "auth request insert failed");
            return unavailable(req.method(), headers);
        }
    }
    let url = s
        .oidc
        .authorize_url(
            &row.state,
            &row.nonce,
            verifier.secret(),
            reauth,
            &s.cfg.required_acr,
        )
        .await;
    let mut res = redirect(StatusCode::FOUND, url.as_str());
    append_cookie(
        &mut res,
        &session::set_cookie(AUTH_COOKIE, auth_cookie, Some(AUTH_REQUEST_TTL)),
    );
    res
}

/// Callback rejection: audit `ev`; once state has been verified
/// (`previous` is `Some`), also end any session this browser presented so a
/// failed or denied sign-in never leaves another user's session behind
/// (SEC-007).
async fn reject(
    s: &AppState,
    m: &Meta,
    ev: AuditEvent,
    previous: Option<&[u8]>,
    mut page: Response,
) -> Response {
    s.audit.record_best_effort(&ev).await;
    if let Some(prev) = previous {
        match s.store.delete(prev).await {
            Ok(Some(gone)) => {
                let mut lo = s.event(m, EventType::Logout, Outcome::Success);
                lo.user_sub = Some(gone.user_sub);
                lo.username = Some(gone.username);
                lo.session_ref = Some(session::session_ref(prev));
                lo.reason = Some("superseded");
                s.audit.record_best_effort(&lo).await;
            }
            Ok(None) => {}
            Err(e) => tracing::error!(error = %e, "superseded session delete failed"),
        }
        append_cookie(&mut page, &session::clear_cookie(SID_COOKIE));
    }
    append_cookie(&mut page, &session::clear_cookie(AUTH_COOKIE));
    page
}

/// `GET /callback`: validate state against the browser-bound auth request,
/// exchange the code, verify the id_token, check freshness, assurance and
/// role, then rotate the session.
// COMPLIANCE MAPPING:
// - NIST 800-53: IA-2 (Identification and Authentication) — OIDC code + PKCE S256 + nonce via icam Keycloak
// - NIST 800-53: IA-2(12) (Acceptance of PIV Credentials) — id_token acr must be a configured CAC/PIV value
// - NIST 800-53: IA-11 (Re-authentication) — after expiry/sign-out, auth_time must postdate the login request
// - NIST 800-53: AC-3 (Access Enforcement) — required console role, else 403 and login.denied audit
// - NIST 800-53: SC-23 (Session Authenticity) — state bound to __Host-console_auth; session id rotated
pub async fn callback(
    State(s): State<Shared>,
    RawQuery(query): RawQuery,
    req: Request,
) -> Response {
    let headers = req.headers();
    let method = req.method().clone();
    let m = s.meta(headers, req.extensions());
    let failure = |reason: &'static str| {
        let mut ev = s.event(&m, EventType::LoginFailure, Outcome::Failure);
        ev.reason = Some(reason);
        ev
    };

    let Some(auth_hash) = session::read_cookie(headers, AUTH_COOKIE)
        .as_deref()
        .and_then(session::hash_cookie_value)
    else {
        return reject(
            &s,
            &m,
            failure("no_auth_request"),
            None,
            pages::callback_error(),
        )
        .await;
    };
    let auth = match s.store.take_auth_request(&auth_hash, s.clock.now()).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return reject(
                &s,
                &m,
                failure("no_auth_request"),
                None,
                pages::callback_error(),
            )
            .await;
        }
        Err(e) => {
            tracing::error!(error = %e, "auth request lookup failed");
            return unavailable(&method, headers);
        }
    };
    let query = query.filter(|q| q.len() <= MAX_QUERY);
    let state = first_param(query.as_deref(), "state").unwrap_or_default();
    if !bool::from(state.as_bytes().ct_eq(auth.state.as_bytes())) {
        return reject(
            &s,
            &m,
            failure("state_mismatch"),
            None,
            pages::callback_error(),
        )
        .await;
    }
    // State verified: from here on this browser really started this sign-in.
    let previous = session::read_cookie(headers, SID_COOKIE)
        .as_deref()
        .and_then(session::hash_cookie_value);
    let prev = previous.as_deref();
    if first_param(query.as_deref(), "error").is_some() {
        return reject(&s, &m, failure("idp_error"), prev, pages::callback_error()).await;
    }
    let Some(code) = first_param(query.as_deref(), "code") else {
        return reject(&s, &m, failure("idp_error"), prev, pages::callback_error()).await;
    };
    let identity = match s
        .oidc
        .exchange(&code, &auth.pkce_verifier, &auth.nonce)
        .await
    {
        Ok(i) => i,
        Err(f) => return reject(&s, &m, failure(f.code()), prev, pages::callback_error()).await,
    };
    let who = |mut ev: AuditEvent| {
        ev.user_sub = Some(identity.sub.clone());
        ev.username = Some(identity.username.clone());
        ev
    };

    if auth.reauth {
        let floor = auth
            .issued_at
            .unwrap_or_else(|| session::sub(auth.expires_at, AUTH_REQUEST_TTL));
        let fresh = identity
            .auth_time
            .is_some_and(|t| t + AUTH_TIME_SKEW >= floor);
        if !fresh {
            return reject(
                &s,
                &m,
                who(failure("stale_auth")),
                prev,
                pages::reauth_required(),
            )
            .await;
        }
    }
    let sso_logout = s.oidc.end_session_url(None).await;
    if !s.cfg.required_acr.is_empty()
        && !identity
            .acr
            .as_deref()
            .is_some_and(|a| s.cfg.required_acr.iter().any(|r| r == a))
    {
        let mut ev = who(s.event(&m, EventType::LoginDenied, Outcome::Denied));
        ev.reason = Some("insufficient_assurance");
        return reject(&s, &m, ev, prev, pages::insufficient_assurance(&sso_logout)).await;
    }
    if !has_role(&identity.roles, &s.cfg.required_role) {
        let mut ev = who(s.event(&m, EventType::LoginDenied, Outcome::Denied));
        ev.reason = Some("missing_role");
        return reject(&s, &m, ev, prev, pages::not_authorized(&sso_logout)).await;
    }

    let (Ok((sid_cookie, sid_hash)), Ok(csrf_token)) =
        (session::new_opaque_id(), session::random_bytes::<32>())
    else {
        return unavailable(&method, headers);
    };
    let now = s.clock.now();
    let roles = s.filter_roles(&identity.roles);
    let new = NewSession {
        id_hash: &sid_hash,
        user_sub: &identity.sub,
        username: &identity.username,
        display_name: &identity.display_name,
        roles: &roles,
        csrf_token: &csrf_token,
        id_token: &identity.id_token,
        refresh_token: identity.refresh_token.as_deref(),
        now,
        absolute_expires_at: session::add(now, s.cfg.session_absolute),
        idle: s.cfg.session_idle,
        acr: identity.acr.as_deref(),
        client_ip: m.ip,
        user_agent: m.user_agent.as_deref(),
    };
    let tx = match s.store.begin_login(prev, &new).await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "session insert failed");
            return unavailable(&method, headers);
        }
    };
    let mut ev = who(s.event(&m, EventType::LoginSuccess, Outcome::Success));
    ev.session_ref = Some(session::session_ref(&sid_hash));
    if auth.reauth {
        ev.reason = Some("reauthenticated");
    }
    // AU-5: no audit record, no session.
    if s.audit.record(&ev).await.is_err() {
        let _ = tx.rollback().await;
        return unavailable(&method, headers);
    }
    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "session commit failed");
        return unavailable(&method, headers);
    }
    let target = format!("{}{}", s.cfg.public_origin(), auth.return_to);
    let mut res = redirect(StatusCode::FOUND, &target);
    append_cookie(&mut res, &session::set_cookie(SID_COOKIE, sid_cookie, None));
    append_cookie(&mut res, &session::clear_cookie(AUTH_COOKIE));
    append_cookie(&mut res, &session::clear_cookie(REAUTH_COOKIE));
    res
}

pub async fn logged_out() -> Response {
    pages::logged_out()
}

/// `GET /auth.css`: public stylesheet for the gateway pages. System colours
/// only, so it follows `color-scheme` (A11Y-4) and needs no tokens.
pub async fn auth_css() -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/css; charset=utf-8"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            ),
        ],
        pages::AUTH_CSS,
    )
        .into_response()
}

/// Static, server-rendered pages: no reflected input, same-origin assets only.
pub mod pages {
    use super::*;

    pub const AUTH_CSS: &str = "\
:root{color-scheme:light dark;font-family:system-ui,-apple-system,\"Segoe UI\",Roboto,sans-serif;\
line-height:1.5;background:Canvas;color:CanvasText}\
body{margin:0;padding:1rem}\
main{max-width:40rem;margin:2rem auto;padding:1.5rem;border:1px solid GrayText;border-radius:.5rem}\
h1{font-size:1.5rem;margin-top:0}\
a{color:LinkText;text-underline-offset:.15em}\
a:focus-visible{outline:2px solid Highlight;outline-offset:2px}\
ul.actions{list-style:none;padding:0;display:flex;flex-wrap:wrap;gap:1rem}\
";

    const HELP: &str = "<p>If this keeps happening, contact your administrator or the help desk \
                        and mention the time of this sign-in attempt.</p>";

    /// Minimal HTML attribute escaping for config-derived URLs.
    fn attr(v: &str) -> String {
        v.replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    fn page(status: StatusCode, title: &str, body: &str) -> Response {
        let html = format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
             <meta name=\"color-scheme\" content=\"light dark\">\
             <link rel=\"stylesheet\" href=\"/auth.css\">\
             <title>{title} - USG Ops Console</title></head>\
             <body><main><h1>{title}</h1>{body}</main></body></html>"
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

    pub fn logged_out() -> Response {
        page(
            StatusCode::OK,
            "You are signed out",
            "<p>Your console session has ended.</p><p><a href=\"/login\">Sign in again</a></p>",
        )
    }

    pub fn callback_error() -> Response {
        page(
            StatusCode::BAD_REQUEST,
            "Sign-in failed",
            &format!(
                "<p>The sign-in attempt could not be completed.</p>\
                 <p><a href=\"/login\">Try again</a></p>{HELP}"
            ),
        )
    }

    pub fn reauth_required() -> Response {
        page(
            StatusCode::BAD_REQUEST,
            "Sign in again",
            &format!(
                "<p>Your previous console session ended, so a fresh sign-in with your CAC/PIV is \
                 required. The single sign-on session was reused instead.</p>\
                 <p><a href=\"/login?reason=expired\">Sign in again</a></p>{HELP}"
            ),
        )
    }

    fn denied(title: &str, why: &str, sso_logout: &str) -> Response {
        page(
            StatusCode::FORBIDDEN,
            title,
            &format!(
                "<p>{why}</p>\
                 <ul class=\"actions\">\
                 <li><a href=\"{}\">Sign out of single sign-on</a></li>\
                 <li><a href=\"/login\">Try signing in again</a></li></ul>\
                 <p>Signing out of single sign-on lets you sign in with a different account or card.</p>\
                 <p>To request access, contact your administrator or the help desk.</p>",
                attr(sso_logout)
            ),
        )
    }

    pub fn not_authorized(sso_logout: &str) -> Response {
        denied(
            "Not authorized",
            "Your account is not authorized to use the USG Ops Console.",
            sso_logout,
        )
    }

    pub fn insufficient_assurance(sso_logout: &str) -> Response {
        denied(
            "CAC/PIV sign-in required",
            "The USG Ops Console requires sign-in with a CAC or PIV card. The sign-in method you \
             used does not meet that requirement.",
            sso_logout,
        )
    }

    pub fn unavailable() -> Response {
        page(
            StatusCode::SERVICE_UNAVAILABLE,
            "Service temporarily unavailable",
            "<p>The console cannot be reached right now. Wait a minute, then \
             <a href=\"/\">try again</a>.</p>",
        )
    }

    pub fn rate_limited() -> Response {
        page(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many sign-in attempts",
            "<p>Wait a minute, then <a href=\"/login\">try signing in again</a>.</p>",
        )
    }
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
    fn navigation_classification() {
        let get = Method::GET;
        assert!(is_navigation(&get, &h(&[("sec-fetch-mode", "navigate")])));
        assert!(!is_navigation(
            &get,
            &h(&[("sec-fetch-mode", "cors"), ("accept", "text/html")])
        ));
        assert!(is_navigation(
            &get,
            &h(&[("accept", "text/html,application/xhtml+xml")])
        ));
        assert!(!is_navigation(&get, &h(&[("accept", "application/json")])));
        assert!(!is_navigation(&get, &HeaderMap::new()));
        assert!(!is_navigation(
            &Method::POST,
            &h(&[("sec-fetch-mode", "navigate")])
        ));
    }

    #[test]
    fn navigation_redirect_keeps_validated_return_to() {
        let nav = h(&[("sec-fetch-mode", "navigate")]);
        let res = unauthenticated(
            &Method::GET,
            &nav,
            "/?site=x",
            AuthErrorReason::NoSession,
            None,
        );
        assert_eq!(res.status(), StatusCode::FOUND);
        assert_eq!(
            res.headers()[header::LOCATION],
            "/login?return_to=%2F%3Fsite%3Dx"
        );
        let res = unauthenticated(
            &Method::GET,
            &nav,
            "//evil",
            AuthErrorReason::NoSession,
            Some(Duration::from_secs(43_200)),
        );
        assert_eq!(res.headers()[header::LOCATION], "/login");
        let cookies: Vec<&str> = res
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect();
        assert!(
            cookies
                .iter()
                .any(|c| c.starts_with("__Host-console_sid=;") && c.contains("Max-Age=0"))
        );
        assert!(
            cookies
                .iter()
                .any(|c| c.starts_with("__Host-console_reauth=1"))
        );
        // An ended session tells /login why (A11Y-7, SEC-001).
        let res = unauthenticated(
            &Method::GET,
            &nav,
            "/a",
            AuthErrorReason::IdleTimeout,
            Some(Duration::from_secs(60)),
        );
        assert_eq!(
            res.headers()[header::LOCATION],
            "/login?return_to=%2Fa&reason=expired"
        );
    }

    #[tokio::test]
    async fn json_errors_and_xhr_detection() {
        let res = json_error(StatusCode::NOT_FOUND, AuthErrorKind::NotFound);
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
        let body = axum::body::to_bytes(res.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], br#"{"error":"not_found"}"#);
        let res = unavailable(&Method::GET, &h(&[("accept", "application/json")]));
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "application/json");
        let res = unavailable(&Method::GET, &h(&[("sec-fetch-mode", "navigate")]));
        assert!(
            res.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
        assert!(wants_json(&h(&[("accept", "application/json, */*")])));
        assert!(!wants_json(&h(&[("accept", "text/html")])));
    }

    #[tokio::test]
    async fn unauthenticated_json_shape() {
        let res = unauthenticated(
            &Method::GET,
            &HeaderMap::new(),
            "/api/session",
            AuthErrorReason::IdleTimeout,
            None,
        );
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            res.headers()[header::WWW_AUTHENTICATE],
            "Session realm=\"usg-ops-console\""
        );
        let body = axum::body::to_bytes(res.into_body(), 1024).await.unwrap();
        assert_eq!(
            std::str::from_utf8(&body).unwrap(),
            r#"{"error":"unauthenticated","reason":"idle_timeout","login_url":"/login"}"#
        );
    }

    #[test]
    fn params() {
        assert_eq!(
            first_param(Some("a=1&code=x%2By&code=z"), "code").as_deref(),
            Some("x+y")
        );
        assert_eq!(first_param(None, "code"), None);
    }

    #[tokio::test]
    async fn pages_reflect_nothing() {
        let sso =
            "https://kc.example/realms/icam/logout?client_id=c&post_logout_redirect_uri=x\"><x";
        for res in [
            pages::logged_out(),
            pages::callback_error(),
            pages::reauth_required(),
            pages::not_authorized(sso),
            pages::insufficient_assurance(sso),
            pages::unavailable(),
            pages::rate_limited(),
        ] {
            assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
            let body = axum::body::to_bytes(res.into_body(), 8192).await.unwrap();
            let html = std::str::from_utf8(&body).unwrap();
            assert!(html.contains(r#"<meta name="color-scheme" content="light dark">"#));
            assert!(html.contains(r#"href="/auth.css""#));
            assert!(!html.contains("\"><x"), "config URL is attribute-escaped");
        }
        let res = pages::not_authorized(sso);
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(res.into_body(), 8192).await.unwrap();
        let html = std::str::from_utf8(&body).unwrap();
        assert!(html.contains("Sign out of single sign-on"));
        assert!(html.contains("client_id=c&amp;post_logout_redirect_uri=x"));
        assert_eq!(pages::callback_error().status(), StatusCode::BAD_REQUEST);
        assert!(!pages::AUTH_CSS.contains("url("), "no external fetches");
    }
}
