//! Console-origin tool API: `GET /api/tools` (registry view filtered by the
//! session's roles) and `POST /api/tools/{id}/launch` (one-time code for the
//! tool origin). Both sit behind `require_session`.

use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use console_protocol::tools::ROUTE_TOOL_LAUNCH;
use console_protocol::{AuthErrorKind, LaunchRequest, LaunchResponse, ToolsResponse, valid_path};

use crate::audit::{EventType, Outcome};
use crate::csrf;
use crate::headers::no_store;
use crate::rate_limit::Decision;
use crate::routes::{Shared, bad_request, json_error, unavailable};
use crate::session::{self, Session};
use crate::tool_session::{LAUNCH_CODE_TTL, MAX_LAUNCH_PATH};

/// Launch request body cap.
const MAX_LAUNCH_BODY: usize = 2048;

fn session_of(req: &Request) -> Option<Arc<Session>> {
    req.extensions().get::<Arc<Session>>().cloned()
}

/// Path without its query (audit records never carry query strings).
pub(crate) fn path_only(p: &str) -> String {
    p.split(['?', '#']).next().unwrap_or_default().to_owned()
}

/// `GET /api/tools`: tools whose `required_roles` the session satisfies.
// COMPLIANCE MAPPING:
// - NIST 800-53: AC-3 (Access Enforcement) — tool list filtered by roles (closes POAM-010)
pub async fn list(State(s): State<Shared>, req: Request) -> Response {
    let Some(sess) = session_of(&req) else {
        return unavailable(req.method(), req.headers());
    };
    let tools = s
        .tools
        .iter()
        .filter(|t| t.cfg.permits(&sess.roles))
        .map(|t| t.cfg.summary())
        .collect();
    ([no_store()], Json(ToolsResponse { tools })).into_response()
}

/// `POST /api/tools/{id}/launch`: CSRF + Origin, roles (404 when missing,
/// like an unknown id: INT-009), then a 256-bit single-use code valid for
/// 30 s at that tool's origin only.
// COMPLIANCE MAPPING:
// - NIST 800-53: AC-3 (Access Enforcement) — role check before any tool session exists, before the body is read
// - NIST 800-53: AU-2 — role denial recorded as tool.denied (missing_role) although the client sees 404
// - NIST 800-53: SC-23 (Session Authenticity) — CSRF-protected, single-use, short-lived, session-bound code
// - NIST 800-53: AU-2/AU-12 — tool.launch audited (success and denial)
// - NIST 800-53: SC-5 (Denial of Service Protection) — 60 launches per session per minute; 2 KiB body
pub async fn launch(State(s): State<Shared>, Path(id): Path<String>, req: Request) -> Response {
    let Some(sess) = session_of(&req) else {
        return unavailable(req.method(), req.headers());
    };
    let (parts, body) = req.into_parts();
    let m = s.meta(&parts.headers, &parts.extensions);
    let presented = parts
        .headers
        .get(csrf::HEADER)
        .and_then(|v| v.to_str().ok());
    if let Err(reason) = csrf::check(
        &parts.headers,
        presented,
        &sess.csrf_token,
        &s.cfg.public_origin(),
    ) {
        tracing::warn!(?reason, "CSRF check failed (tool launch)");
        let ev = s.session_event(
            &m,
            &sess,
            EventType::AccessDenied,
            Outcome::Denied,
            match reason {
                console_protocol::AuthErrorReason::Origin => "origin",
                _ => "csrf_token",
            },
        );
        s.audit.record_best_effort(&ev).await;
        return crate::routes::auth_error_json(StatusCode::FORBIDDEN, AuthErrorKind::Csrf, reason);
    }
    if let Decision::Limited { .. } = s
        .tools
        .launch_limiter
        .check_key(sess.id_hash.clone(), Instant::now())
    {
        let mut res = json_error(StatusCode::TOO_MANY_REQUESTS, AuthErrorKind::RateLimited);
        res.headers_mut().insert(
            axum::http::header::RETRY_AFTER,
            axum::http::HeaderValue::from_static("60"),
        );
        return res;
    }
    // INT-009: a tool the session may not use is indistinguishable from an
    // unknown id (404), and is decided before the body is read.
    let Some(tool) = s.tools.by_id(&id).cloned() else {
        return json_error(StatusCode::NOT_FOUND, AuthErrorKind::NotFound);
    };
    let mut ev = s.session_event(&m, &sess, EventType::ToolLaunch, Outcome::Success, "launch");
    ev.tool_id = Some(tool.cfg.id.clone());
    if !tool.cfg.permits(&sess.roles) {
        ev.event_type = EventType::ToolDenied;
        ev.outcome = Outcome::Denied;
        ev.reason = Some("missing_role");
        ev.http_status = Some(404);
        // SEC-011: queued, never awaited, so this 404 takes the same time as
        // the unknown-id 404 (no DB round trip distinguishes them).
        s.audit.record_background(ev);
        return json_error(StatusCode::NOT_FOUND, AuthErrorKind::NotFound);
    }
    let bytes = match axum::body::to_bytes(body, MAX_LAUNCH_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return json_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                AuthErrorKind::PayloadTooLarge,
            );
        }
    };
    let request: LaunchRequest = if bytes.iter().all(u8::is_ascii_whitespace) {
        LaunchRequest { path: None }
    } else {
        match serde_json::from_slice(&bytes) {
            Ok(r) => r,
            Err(_) => return bad_request(),
        }
    };
    let path = request.path.unwrap_or_else(|| tool.cfg.entry.clone());
    if path.len() > MAX_LAUNCH_PATH || !valid_path(&path) || path.chars().any(char::is_control) {
        return bad_request();
    }
    ev.path = Some(path_only(&path));
    let Ok((code, code_hash)) = session::new_opaque_id() else {
        return unavailable(&parts.method, &parts.headers);
    };
    let expires = session::add(s.clock.now(), LAUNCH_CODE_TTL);
    if let Err(e) = s
        .store
        .insert_launch_code(&code_hash, &sess.id_hash, &tool.cfg.id, &path, expires)
        .await
    {
        tracing::error!(error = %e, "launch code insert failed");
        return unavailable(&parts.method, &parts.headers);
    }
    ev.http_status = Some(200);
    s.audit.record_best_effort(&ev).await;
    let url = format!("{}{ROUTE_TOOL_LAUNCH}?code={code}", tool.cfg.origin);
    ([no_store()], Json(LaunchResponse { url })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_paths_drop_query() {
        assert_eq!(path_only("/x?code=secret"), "/x");
        assert_eq!(path_only("/a#frag"), "/a");
        assert_eq!(path_only("/"), "/");
    }
}
