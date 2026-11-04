//! `return_to` validation: only same-origin absolute paths survive.

// COMPLIANCE MAPPING:
// - NIST 800-53: SI-10 (Information Input Validation) — open-redirect prevention

const MAX_LEN: usize = 2048;
const LOOP_PREFIXES: [&str; 3] = ["/login", "/callback", "/logout"];

/// Returns `raw` if it is a safe same-origin path+query, otherwise `/`.
pub fn sanitize(raw: Option<&str>) -> String {
    match raw {
        Some(v) if is_safe(v) => v.to_owned(),
        _ => "/".to_owned(),
    }
}

pub fn is_safe(v: &str) -> bool {
    if v.is_empty() || v.len() > MAX_LEN || !v.starts_with('/') {
        return false;
    }
    if matches!(v.as_bytes().get(1), Some(b'/' | b'\\')) {
        return false;
    }
    if v.bytes()
        .any(|b| b == b'\\' || b.is_ascii_control() || b == b' ' || b >= 0x80)
    {
        return false;
    }
    let lower = v.to_ascii_lowercase();
    if lower.contains("%2f") || lower.contains("%5c") {
        return false;
    }
    // Decode once; a decoded `//` or `/\` prefix is a protocol-relative URL.
    let decoded = percent_decode(v);
    if decoded.starts_with("//") || decoded.starts_with("/\\") {
        return false;
    }
    if decoded.bytes().any(|b| b.is_ascii_control()) {
        return false;
    }
    let path = v.split(['?', '#']).next().unwrap_or(v);
    // `path` has no scheme because it starts with `/`; a `:` before any `/`
    // can't occur. Reject the auth endpoints to avoid redirect loops.
    !LOOP_PREFIXES.iter().any(|p| {
        path == *p
            || path
                .strip_prefix(p)
                .is_some_and(|rest| rest.starts_with('/') || rest.starts_with('.'))
    })
}

/// Post-login notices the shell may show (A11Y-7). Fixed values only; the
/// `reason` query parameter on `/login` is never reflected verbatim.
pub fn notice(reason: Option<&str>) -> Option<&'static str> {
    match reason {
        Some("expired") => Some("expired"),
        Some("ended") => Some("ended"),
        _ => None,
    }
}

/// Appends `console_notice=<notice>` to a sanitized `return_to`, before any
/// fragment.
pub fn with_notice(return_to: &str, notice: &'static str) -> String {
    let (base, frag) = match return_to.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (return_to, None),
    };
    let sep = if base.contains('?') { '&' } else { '?' };
    match frag {
        Some(f) => format!("{base}{sep}console_notice={notice}#{f}"),
        None => format!("{base}{sep}console_notice={notice}"),
    }
}

fn percent_decode(v: &str) -> String {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (
                b.get(i + 1).and_then(|c| (*c as char).to_digit(16)),
                b.get(i + 2).and_then(|c| (*c as char).to_digit(16)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_paths() {
        for ok in [
            "/",
            "/?site=x",
            "/t/dcim/racks?id=4&x=%20y",
            "/loginx",
            "/a#frag",
        ] {
            assert!(is_safe(ok), "{ok}");
        }
    }

    #[test]
    fn rejects_open_redirects_and_loops() {
        let long = format!("/{}", "a".repeat(MAX_LEN));
        let bad = [
            "",
            "//evil.example",
            "/\\evil.example",
            "/%2f%2fevil.example",
            "/%2F/evil",
            "/%5cevil",
            "https://evil.example",
            "javascript:alert(1)",
            "evil",
            "/a\r\nSet-Cookie:x",
            "/a b",
            "/a\\b",
            "/%0d%0aX:1",
            "/%09/evil",
            long.as_str(),
            "/login",
            "/login?return_to=/x",
            "/callback?code=1",
            "/logout",
            "/logout/",
            "/caf\u{e9}",
        ];
        for b in bad {
            assert!(!is_safe(b), "{b:?}");
        }
        assert_eq!(sanitize(Some("//evil")), "/");
        assert_eq!(sanitize(None), "/");
        assert_eq!(sanitize(Some("/?site=x")), "/?site=x");
    }

    #[test]
    fn notices_are_fixed_values() {
        assert_eq!(notice(Some("expired")), Some("expired"));
        assert_eq!(notice(Some("ended")), Some("ended"));
        assert_eq!(notice(Some("<script>")), None);
        assert_eq!(notice(None), None);
        assert_eq!(with_notice("/", "expired"), "/?console_notice=expired");
        assert_eq!(
            with_notice("/a?x=1#frag", "ended"),
            "/a?x=1&console_notice=ended#frag"
        );
        assert!(is_safe(&with_notice("/a?x=1", "expired")));
    }
}
