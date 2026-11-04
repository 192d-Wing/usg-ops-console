//! Source address for audit records, honouring `X-Forwarded-For` only when
//! the TCP peer is a configured trusted proxy.

use std::net::IpAddr;

use axum::http::HeaderMap;
use ipnet::IpNet;

// COMPLIANCE MAPPING:
// - NIST 800-53: AU-3 (Content of Audit Records) — "where": spoof-resistant source address
/// `peer` is the TCP peer (`None` in in-process tests).
pub fn resolve(peer: Option<IpAddr>, headers: &HeaderMap, trusted: &[IpNet]) -> Option<IpAddr> {
    let peer = peer?;
    let is_trusted = |ip: &IpAddr| trusted.iter().any(|n| n.contains(ip));
    if !is_trusted(&peer) {
        return Some(peer);
    }
    // Walk all X-Forwarded-For values right-to-left, skipping trusted hops.
    let mut hops = Vec::new();
    for value in headers.get_all("x-forwarded-for") {
        let Ok(s) = value.to_str() else {
            tracing::warn!("malformed X-Forwarded-For from trusted proxy; using peer");
            return Some(peer);
        };
        hops.extend(s.split(',').map(str::trim));
    }
    for hop in hops.iter().rev() {
        match hop.parse::<IpAddr>() {
            Ok(ip) if is_trusted(&ip) => continue,
            Ok(ip) => return Some(ip),
            Err(_) => {
                tracing::warn!("malformed X-Forwarded-For entry from trusted proxy; using peer");
                return Some(peer);
            }
        }
    }
    Some(peer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn xff(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_str(v).unwrap());
        h
    }

    #[test]
    fn table() {
        let trusted: Vec<IpNet> = vec!["10.0.0.0/8".parse().unwrap()];
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        let cases: &[(&str, &str, Option<IpAddr>)] = &[
            // untrusted peer: header ignored
            ("203.0.113.9", "1.2.3.4", ip("203.0.113.9")),
            // trusted peer: first untrusted from the right
            ("10.0.0.2", "1.2.3.4, 10.0.0.5", ip("1.2.3.4")),
            ("10.0.0.2", "6.6.6.6, 1.2.3.4", ip("1.2.3.4")),
            // all trusted: peer
            ("10.0.0.2", "10.1.1.1", ip("10.0.0.2")),
            // malformed: peer
            ("10.0.0.2", "1.2.3.4, garbage", ip("10.0.0.2")),
            ("10.0.0.2", "", ip("10.0.0.2")),
        ];
        for (peer, header, want) in cases {
            let got = resolve(peer.parse().ok(), &xff(header), &trusted);
            assert_eq!(got, *want, "peer={peer} xff={header}");
        }
        assert_eq!(resolve(None, &xff("1.2.3.4"), &trusted), None);
        assert_eq!(
            resolve(ip("10.0.0.2"), &HeaderMap::new(), &trusted),
            ip("10.0.0.2")
        );
    }
}
