//! CSRF defence for state-changing endpoints: synchronizer token compared in
//! constant time, plus an `Origin`/`Referer` same-origin check.

use axum::http::{HeaderMap, header};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use console_protocol::AuthErrorReason;
use subtle::ConstantTimeEq;
use url::Url;

pub const HEADER: &str = "x-csrf-token";

// COMPLIANCE MAPPING:
// - NIST 800-53: SC-23 (Session Authenticity) — per-session CSRF synchronizer token
/// Constant-time comparison of a presented base64url token with the
/// session's raw token bytes.
pub fn token_matches(presented: Option<&str>, expected: &[u8]) -> bool {
    let Some(p) = presented else { return false };
    let Ok(bytes) = B64URL.decode(p.trim()) else {
        return false;
    };
    bytes.len() == expected.len() && bool::from(bytes.ct_eq(expected))
}

/// `Origin` must equal the console origin; without `Origin`, `Referer` must
/// be same-origin. Neither present is a failure.
pub fn origin_allowed(headers: &HeaderMap, public_origin: &str) -> bool {
    if let Some(o) = headers.get(header::ORIGIN) {
        return o.to_str().is_ok_and(|o| o == public_origin);
    }
    headers
        .get(header::REFERER)
        .and_then(|r| r.to_str().ok())
        .and_then(|r| Url::parse(r).ok())
        .is_some_and(|u| u.origin().ascii_serialization() == public_origin)
}

/// Validates both checks; returns the failing reason.
pub fn check(
    headers: &HeaderMap,
    presented: Option<&str>,
    expected: &[u8],
    public_origin: &str,
) -> Result<(), AuthErrorReason> {
    if !origin_allowed(headers, public_origin) {
        return Err(AuthErrorReason::Origin);
    }
    if !token_matches(presented, expected) {
        return Err(AuthErrorReason::CsrfToken);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const ORIGIN: &str = "https://console.example.mil";

    #[test]
    fn token_compare() {
        let raw = [7u8; 32];
        let good = B64URL.encode(raw);
        assert!(token_matches(Some(&good), &raw));
        assert!(!token_matches(Some(&B64URL.encode([8u8; 32])), &raw));
        assert!(!token_matches(Some(&B64URL.encode([7u8; 31])), &raw));
        assert!(!token_matches(Some("not base64!"), &raw));
        assert!(!token_matches(None, &raw));
    }

    #[test]
    fn origin_rules() {
        let mut h = HeaderMap::new();
        assert!(!origin_allowed(&h, ORIGIN), "neither header");
        h.insert(
            header::REFERER,
            HeaderValue::from_static("https://console.example.mil/x"),
        );
        assert!(origin_allowed(&h, ORIGIN));
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert!(!origin_allowed(&h, ORIGIN), "Origin wins over Referer");
        h.insert(header::ORIGIN, HeaderValue::from_static(ORIGIN));
        assert!(origin_allowed(&h, ORIGIN));
        h.insert(header::ORIGIN, HeaderValue::from_static("null"));
        assert!(!origin_allowed(&h, ORIGIN));
        let mut r = HeaderMap::new();
        r.insert(
            header::REFERER,
            HeaderValue::from_static("https://console.example.mil.evil/x"),
        );
        assert!(!origin_allowed(&r, ORIGIN));
    }

    #[test]
    fn check_reports_reason() {
        let raw = [1u8; 32];
        let mut h = HeaderMap::new();
        assert_eq!(check(&h, None, &raw, ORIGIN), Err(AuthErrorReason::Origin));
        h.insert(header::ORIGIN, HeaderValue::from_static(ORIGIN));
        assert_eq!(
            check(&h, None, &raw, ORIGIN),
            Err(AuthErrorReason::CsrfToken)
        );
        assert_eq!(check(&h, Some(&B64URL.encode(raw)), &raw, ORIGIN), Ok(()));
    }
}
