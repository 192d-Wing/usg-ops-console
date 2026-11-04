//! Session helpers around the `console-protocol` `/api/session` types (brief
//! Q5): RFC 3339 parsing, the login URL built from the current location
//! (never from response data), and logout-redirect validation.

pub use console_protocol::session::{LogoutResponse, SessionInfo};

/// Parses `YYYY-MM-DDTHH:MM:SS[.frac](Z|+HH:MM|-HH:MM)` to Unix milliseconds.
pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = s.get(r)?;
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())
            .flatten()
    };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut i = 19;
    let mut ms = 0i64;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return None;
        }
        let frac = &s[start..i.min(start + 3)];
        ms = frac.parse::<i64>().ok()? * 10i64.pow(3 - frac.len() as u32);
    }
    let offset_min = match &s[i..] {
        "Z" | "z" => 0,
        tz if tz.len() == 6
            && matches!(tz.as_bytes()[0], b'+' | b'-')
            && tz.as_bytes()[3] == b':' =>
        {
            let oh = num(i + 1..i + 3)?;
            let om = num(i + 4..i + 6)?;
            let v = oh * 60 + om;
            if tz.starts_with('-') { -v } else { v }
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo, d);
    let secs = days * 86_400 + h * 3600 + mi * 60 + se - offset_min * 60;
    Some(secs * 1000 + ms)
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `encodeURIComponent` semantics.
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Why the shell is sending the user to `/login`. Sent as `reason=` and
/// returned by the gateway as `console_notice=` on `return_to` after the new
/// sign-in, so the shell can explain the sign-out (A11Y-7, NIST-001).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginNotice {
    /// Idle or absolute lifetime reached (AC-11/AC-12).
    Expired,
    /// The gateway ended the session early (e.g. revoked at the IdP or
    /// signed out elsewhere) while the user tried to stay signed in.
    Ended,
}

impl LoginNotice {
    pub fn as_str(self) -> &'static str {
        match self {
            LoginNotice::Expired => "expired",
            LoginNotice::Ended => "ended",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "expired" => Some(LoginNotice::Expired),
            "ended" => Some(LoginNotice::Ended),
            _ => None,
        }
    }
    /// Info flash shown after the user is back in the shell.
    pub fn message(self) -> &'static str {
        match self {
            LoginNotice::Expired => {
                "Your previous session expired, so you were signed out. You are now signed in again."
            }
            LoginNotice::Ended => {
                "Your previous session was ended by the server and could not be extended. You are now signed in again."
            }
        }
    }
}

/// `/login?return_to=<pathname+search>[&reason=expired|ended]`, from
/// `window.location` only. The path is forced same-origin-relative first;
/// the gateway re-validates it. `reason=expired` lets the gateway require a
/// fresh authentication (brief SEC-001) and show expiry copy.
pub fn login_url(pathname: &str, search: &str, notice: Option<LoginNotice>) -> String {
    let target = crate::same_origin_url(&format!("{pathname}{search}"));
    let mut url = format!("/login?return_to={}", encode_uri_component(&target));
    if let Some(n) = notice {
        url.push_str("&reason=");
        url.push_str(n.as_str());
    }
    url
}

/// Query parameter the gateway appends to `return_to` after a sign-in that
/// followed an expiry or revocation (CONTRACT DELTA 8).
pub const NOTICE_PARAM: &str = "console_notice";

/// Removes every `key=` pair from `search` (`?a=1&key=x`). Returns the first
/// value and the remaining search string (`""` when nothing is left).
pub fn take_param(search: &str, key: &str) -> (Option<String>, String) {
    let q = search.strip_prefix('?').unwrap_or(search);
    let mut found = None;
    let kept: Vec<&str> = q
        .split('&')
        .filter(|p| !p.is_empty())
        .filter(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            if k == key {
                found.get_or_insert_with(|| v.to_owned());
                false
            } else {
                true
            }
        })
        .collect();
    let rest = if kept.is_empty() {
        String::new()
    } else {
        format!("?{}", kept.join("&"))
    };
    (found, rest)
}

/// Same-origin page the gateway may send the browser to after sign-out.
pub const LOGGED_OUT_PATH: &str = "/logged-out";

/// `scheme://host[:port]`, lower-cased, default port dropped. `None` for
/// anything but http(s), userinfo (`user@host`), backslashes, or an empty
/// host.
pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let auth = rest[..end].to_ascii_lowercase();
    if auth.is_empty() || auth.contains(['@', '\\']) || auth.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    let auth = match (
        scheme.as_str(),
        auth.strip_suffix(":443"),
        auth.strip_suffix(":80"),
    ) {
        ("https", Some(h), _) | ("http", _, Some(h)) => h.to_owned(),
        _ => auth,
    };
    if auth.starts_with(':') {
        return None;
    }
    Some(format!("{scheme}://{auth}"))
}

fn is_loopback_origin(origin: &str) -> bool {
    let host = origin.split_once("://").map_or("", |(_, a)| a);
    let host = if host.starts_with('[') {
        host.split_once(']')
            .map_or(host, |(h, _)| h)
            .trim_start_matches('[')
    } else {
        host.rsplit_once(':').map_or(host, |(h, _)| h)
    };
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

fn is_logged_out_path(path_and_query: &str) -> bool {
    path_and_query.split(['?', '#']).next() == Some(LOGGED_OUT_PATH)
}

/// Logout redirect from the gateway (SEC-005, SI-10). Accepted only if it is
/// the console's own `/logged-out` page (relative, or absolute on
/// `own_origin`), or an absolute URL whose origin equals the gateway-provided
/// `end_session_origin`. `http://` is accepted only for a loopback host
/// (local dev). Control characters and backslashes are always rejected.
pub fn accept_logout_redirect<'a>(
    url: &'a str,
    end_session_origin: Option<&str>,
    own_origin: &str,
) -> Option<&'a str> {
    if url.chars().any(|c| c.is_control() || c == '\\') {
        return None;
    }
    if url.starts_with('/') {
        return (!url.starts_with("//") && is_logged_out_path(url)).then_some(url);
    }
    let got = origin_of(url)?;
    if !(got.starts_with("https://") || is_loopback_origin(&got)) {
        return None;
    }
    if origin_of(own_origin).as_deref() == Some(got.as_str()) {
        let path = url
            .split_once("://")
            .and_then(|(_, r)| r.find('/').map(|i| &r[i..]))
            .unwrap_or("");
        return is_logged_out_path(path).then_some(url);
    }
    (origin_of(end_session_origin?)? == got).then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_ms("2026-11-03T18:15:00Z"),
            Some(1_793_729_700_000)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-11-03T18:15:00.25Z"),
            Some(1_793_729_700_250)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-11-03T18:15:00.123456789Z"),
            Some(1_793_729_700_123)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-11-03T20:15:00+02:00"),
            Some(1_793_729_700_000)
        );
        for bad in [
            "",
            "2026-11-03",
            "2026-13-03T18:15:00Z",
            "2026-11-03T18:15:00",
            "2026-11-03T18:15:00.Z",
            "x026-11-03T18:15:00Z",
        ] {
            assert_eq!(parse_rfc3339_ms(bad), None, "{bad}");
        }
    }

    #[test]
    fn login_url_is_same_origin_and_encoded() {
        assert_eq!(login_url("/", "", None), "/login?return_to=%2F");
        assert_eq!(
            login_url("/gallery", "?site=hq&x=a b", None),
            "/login?return_to=%2Fgallery%3Fsite%3Dhq%26x%3Da%20b"
        );
        assert_eq!(
            login_url("//evil.example/", "", None),
            "/login?return_to=%2Fevil.example%2F"
        );
        assert_eq!(
            login_url("/a", "", Some(LoginNotice::Expired)),
            "/login?return_to=%2Fa&reason=expired"
        );
        assert_eq!(
            login_url("/a", "", Some(LoginNotice::Ended)),
            "/login?return_to=%2Fa&reason=ended"
        );
    }

    #[test]
    fn take_param_strips_only_that_key() {
        assert_eq!(
            take_param("?site=hq&console_notice=expired&x=1", NOTICE_PARAM),
            (Some("expired".into()), "?site=hq&x=1".into())
        );
        assert_eq!(
            take_param("?console_notice=ended", NOTICE_PARAM),
            (Some("ended".into()), String::new())
        );
        assert_eq!(
            take_param("?site=hq", NOTICE_PARAM),
            (None, "?site=hq".into())
        );
        assert_eq!(take_param("", NOTICE_PARAM), (None, String::new()));
        assert_eq!(
            take_param("?console_notice=a&console_notice=b", NOTICE_PARAM),
            (Some("a".into()), String::new())
        );
    }

    #[test]
    fn notice_round_trips() {
        for n in [LoginNotice::Expired, LoginNotice::Ended] {
            assert_eq!(LoginNotice::parse(n.as_str()), Some(n));
        }
        assert_eq!(LoginNotice::parse("token"), None);
    }

    #[test]
    fn origin_normalises() {
        assert_eq!(
            origin_of("HTTPS://KC.Example:443/realms/x?y").as_deref(),
            Some("https://kc.example")
        );
        assert_eq!(
            origin_of("http://localhost:9443/a").as_deref(),
            Some("http://localhost:9443")
        );
        for bad in [
            "javascript://x",
            "https://u@kc.example/",
            "https:///x",
            "https://a\\b",
            "kc",
        ] {
            assert_eq!(origin_of(bad), None, "{bad}");
        }
    }

    #[test]
    fn logout_redirect_only_to_end_session_origin_or_logged_out() {
        let me = "https://console.example";
        let ok = |u: &str, o: Option<&str>| accept_logout_redirect(u, o, me).is_some();
        let kc = Some("https://kc.example");
        let url = "https://kc.example/realms/icam/protocol/openid-connect/logout?client_id=x";
        assert_eq!(accept_logout_redirect(url, kc, me), Some(url));
        assert!(ok("/logged-out", None));
        assert!(ok("/logged-out?x=1", kc));
        // No end-session endpoint: absolute /logged-out on the console origin.
        assert!(ok("https://console.example/logged-out", None));
        assert!(ok("https://CONSOLE.example:443/logged-out", None));
        assert!(!ok("https://console.example/", None));
        assert!(!ok("https://console.example/other", kc));
        // Loopback http only (local compose).
        assert!(ok(
            "http://localhost:9443/logout",
            Some("http://localhost:9443")
        ));
        assert!(ok("http://127.0.0.1/logout", Some("http://127.0.0.1:80")));
        for (bad, origin) in [
            (url, None),
            (url, Some("https://other.example")),
            ("https://kc.example.evil/x", kc),
            ("https://kc.example@evil/x", kc),
            ("http://kc.example/logout", Some("http://kc.example")),
            ("javascript:alert(1)", kc),
            ("//kc.example/x", kc),
            ("/\\evil", kc),
            ("/", kc),
            ("/logged-outx", kc),
            ("data:text/html,x", kc),
            ("https://kc.example/a\nb", kc),
        ] {
            assert!(!ok(bad, origin), "{bad} {origin:?}");
        }
    }

    #[test]
    fn shared_fixture_deserializes() {
        let raw = include_str!("../../console-gateway/tests/fixtures/session_v1.json");
        let s: SessionInfo = serde_json::from_str(raw).expect("parse");
        assert!(parse_rfc3339_ms(&s.idle_expires_at).is_some());
        assert!(parse_rfc3339_ms(&s.absolute_expires_at).is_some());
        assert!(parse_rfc3339_ms(&s.server_time).is_some());
    }
}
