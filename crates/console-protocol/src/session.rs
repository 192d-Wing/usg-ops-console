//! Gateway HTTP session contract (`/api/session`, 401/403 bodies, logout).
//!
//! These are HTTP JSON shapes served by the gateway, not `postMessage`
//! messages, but they live here so the shell and (in M4) tools share one
//! definition. Additive only: new fields carry `#[serde(default)]`, and the
//! enums tolerate unknown values via `Unknown`.

use serde::{Deserialize, Serialize};

/// `GET /api/session` and `POST /api/session/keepalive` response (v1).
///
/// Timestamps are RFC 3339 UTC strings (`2026-11-03T18:15:00Z`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub user: SessionUser,
    #[serde(default)]
    pub roles: Vec<String>,
    /// `min(last_seen + idle, absolute_expires_at)`.
    pub idle_expires_at: String,
    pub absolute_expires_at: String,
    pub idle_timeout_secs: u64,
    /// Gateway clock at response time, for client skew correction.
    pub server_time: String,
    /// Per-session synchronizer token for `X-CSRF-Token`; keep in memory only.
    pub csrf_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionUser {
    /// IdP subject (`sub`).
    pub id: String,
    pub username: String,
    pub display_name: String,
}

/// Body of `401` (`error = unauthenticated`), CSRF `403` (`error = csrf`),
/// and the gateway's other JSON errors (`not_found`, `unavailable`,
/// `payload_too_large`, `rate_limited`), which carry no `reason`/`login_url`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthErrorBody {
    pub error: AuthErrorKind,
    /// Advisory, for UI copy only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<AuthErrorReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthErrorKind {
    Unauthenticated,
    Csrf,
    /// Unmatched `/api/*` path.
    NotFound,
    /// Session store or IdP temporarily unreachable (`503`).
    Unavailable,
    /// Request body over the endpoint's limit (`413`).
    PayloadTooLarge,
    /// Too many requests (`429`, see `Retry-After`).
    RateLimited,
    /// A value added by a newer gateway.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthErrorReason {
    NoSession,
    IdleTimeout,
    AbsoluteTimeout,
    /// CSRF token missing or wrong.
    CsrfToken,
    /// `Origin`/`Referer` missing or not the console origin.
    Origin,
    /// Ended early by the IdP (refresh rejected, user disabled, role removed).
    Revoked,
    /// A value added by a newer gateway.
    #[serde(other)]
    Unknown,
}

/// `POST /logout` XHR response: the shell sets `location` to `redirect`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogoutResponse {
    pub redirect: String,
    /// Origin (`scheme://host[:port]`) of the IdP end-session endpoint. The
    /// shell follows `redirect` only to this origin or to the console's own
    /// `/logged-out`. Absent when the IdP has no end-session endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_session_origin: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_v1_round_trips() {
        let json = include_str!("../../console-gateway/tests/fixtures/session_v1.json");
        let info: SessionInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.user.username, "jdoe");
        assert_eq!(info.idle_timeout_secs, 900);
        let back: serde_json::Value = serde_json::to_value(&info).unwrap();
        let orig: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(back, orig);
    }

    /// The v1 fixture is frozen: never edit `session_v1.json`. A newer
    /// client must still parse what a v1 gateway sends, so every field added
    /// to `SessionInfo` after v1 needs `#[serde(default)]`; this test fails
    /// otherwise.
    #[test]
    fn frozen_v1_fixture_still_parses() {
        let json = include_str!("../../console-gateway/tests/fixtures/session_v1.json");
        let orig: serde_json::Value = serde_json::from_str(json).unwrap();
        let keys: Vec<&str> = orig
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "absolute_expires_at",
                "csrf_token",
                "idle_expires_at",
                "idle_timeout_secs",
                "roles",
                "server_time",
                "user"
            ],
            "session_v1.json is frozen"
        );
        let info: SessionInfo = serde_json::from_value(orig.clone()).unwrap();
        // Every v1 key is still emitted under the same name (the shell's
        // hand-rolled decoder reads these literals).
        let back = serde_json::to_value(&info).unwrap();
        for k in &keys {
            assert_eq!(back[k], orig[k], "{k}");
        }
        let user: Vec<&str> = back["user"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(user, ["display_name", "id", "username"]);
    }

    #[test]
    fn new_error_kinds_and_logout_origin() {
        let nf = AuthErrorBody {
            error: AuthErrorKind::NotFound,
            reason: None,
            login_url: None,
        };
        assert_eq!(
            serde_json::to_string(&nf).unwrap(),
            r#"{"error":"not_found"}"#
        );
        let r: AuthErrorReason = serde_json::from_str(r#""revoked""#).unwrap();
        assert_eq!(r, AuthErrorReason::Revoked);
        let old: LogoutResponse = serde_json::from_str(r#"{"redirect":"/logged-out"}"#).unwrap();
        assert_eq!(old.end_session_origin, None);
        let lr = LogoutResponse {
            redirect: "https://kc/x".into(),
            end_session_origin: Some("https://kc".into()),
        };
        assert_eq!(
            serde_json::to_string(&lr).unwrap(),
            r#"{"redirect":"https://kc/x","end_session_origin":"https://kc"}"#
        );
    }

    #[test]
    fn tolerates_unknown_fields_and_values() {
        let body: AuthErrorBody = serde_json::from_str(
            r#"{"error":"something_new","reason":"later","login_url":"/login","extra":1}"#,
        )
        .unwrap();
        assert_eq!(body.error, AuthErrorKind::Unknown);
        assert_eq!(body.reason, Some(AuthErrorReason::Unknown));
        let minimal: AuthErrorBody = serde_json::from_str(r#"{"error":"csrf"}"#).unwrap();
        assert_eq!(minimal.reason, None);
    }

    #[test]
    fn auth_error_shape() {
        let body = AuthErrorBody {
            error: AuthErrorKind::Unauthenticated,
            reason: Some(AuthErrorReason::IdleTimeout),
            login_url: Some("/login".into()),
        };
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            r#"{"error":"unauthenticated","reason":"idle_timeout","login_url":"/login"}"#
        );
    }
}
