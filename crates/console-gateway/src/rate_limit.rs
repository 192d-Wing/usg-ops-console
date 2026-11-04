//! Per-key token buckets (bounded memory): `/login` per client IP, tool
//! launches per session (M4).
//!
//! IPv6 clients are keyed by their /64 (one end-site prefix), so rotating
//! addresses inside a prefix does not mint fresh buckets. At the table cap
//! the oldest-touched 10% are evicted in one pass (amortised O(1) per new
//! key); live buckets of active clients are never reset wholesale.
//! Per-IP limiting at the ingress is still expected in production.

use std::collections::HashMap;
use std::hash::Hash;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Mutex;
use std::time::Instant;

const MAX_TRACKED: usize = 10_000;
const EVICT: usize = MAX_TRACKED / 10;

/// Result of [`RateLimiter::check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Refused. `first` is true only for the first refusal of an episode
    /// (until the client is allowed again), so callers audit once, not per
    /// request (no audit amplification under attack).
    Limited {
        first: bool,
    },
}

struct Bucket {
    tokens: f64,
    at: Instant,
    limited: bool,
}

// COMPLIANCE MAPPING:
// - NIST 800-53: SC-5 (Denial of Service Protection) — partial: /login rate limit, bounded table
pub struct RateLimiter<K = IpAddr> {
    capacity: f64,
    per_sec: f64,
    buckets: Mutex<HashMap<K, Bucket>>,
}

/// IPv4 as-is; IPv6 masked to its /64.
pub fn key_for(ip: Option<IpAddr>) -> IpAddr {
    match ip {
        None => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        Some(IpAddr::V4(v4)) => IpAddr::V4(v4),
        Some(IpAddr::V6(v6)) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let masked = u128::from(v6) & (u128::MAX << 64);
                IpAddr::V6(Ipv6Addr::from(masked))
            }
        },
    }
}

impl RateLimiter<IpAddr> {
    pub fn allow(&self, ip: Option<IpAddr>, now: Instant) -> bool {
        self.check(ip, now) == Decision::Allow
    }

    /// `None` IPs (in-process tests) share one bucket.
    pub fn check(&self, ip: Option<IpAddr>, now: Instant) -> Decision {
        self.check_key(key_for(ip), now)
    }
}

impl<K: Hash + Eq + Clone> RateLimiter<K> {
    /// `per_minute` requests per key, bursting to the same amount.
    pub fn per_minute(per_minute: u32) -> Self {
        Self {
            capacity: f64::from(per_minute),
            per_sec: f64::from(per_minute) / 60.0,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    pub fn check_key(&self, key: K, now: Instant) -> Decision {
        let mut map = match self.buckets.lock() {
            Ok(m) => m,
            Err(poisoned) => poisoned.into_inner(),
        };
        if map.len() >= MAX_TRACKED && !map.contains_key(&key) {
            evict_oldest(&mut map);
        }
        let b = map.entry(key).or_insert(Bucket {
            tokens: self.capacity,
            at: now,
            limited: false,
        });
        let refill = now.saturating_duration_since(b.at).as_secs_f64() * self.per_sec;
        b.tokens = (b.tokens + refill).min(self.capacity);
        b.at = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            b.limited = false;
            Decision::Allow
        } else {
            let first = !b.limited;
            b.limited = true;
            Decision::Limited { first }
        }
    }
}

/// Removes the `EVICT` least recently touched entries.
fn evict_oldest<K: Hash + Eq + Clone>(map: &mut HashMap<K, Bucket>) {
    let mut ages: Vec<(Instant, K)> = map.iter().map(|(k, b)| (b.at, k.clone())).collect();
    let n = EVICT.min(ages.len());
    if n == 0 {
        return;
    }
    ages.select_nth_unstable_by(n - 1, |a, b| a.0.cmp(&b.0));
    for (_, k) in &ages[..n] {
        map.remove(k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn bucket() {
        let rl = RateLimiter::<IpAddr>::per_minute(30);
        let t0 = Instant::now();
        let a = Some("192.0.2.1".parse().unwrap());
        let b = Some("192.0.2.2".parse().unwrap());
        assert!((0..30).all(|_| rl.allow(a, t0)));
        assert_eq!(rl.check(a, t0), Decision::Limited { first: true });
        assert_eq!(rl.check(a, t0), Decision::Limited { first: false });
        assert!(rl.allow(b, t0), "other IPs unaffected");
        assert!(rl.allow(a, t0 + Duration::from_secs(2)), "refills 0.5/s");
        assert_eq!(
            rl.check(a, t0 + Duration::from_secs(2)),
            Decision::Limited { first: true },
            "a new episode after an allow"
        );
    }

    #[test]
    fn ipv6_keyed_by_slash_64() {
        let rl = RateLimiter::<IpAddr>::per_minute(2);
        let t0 = Instant::now();
        let x = Some("2001:db8:1:2::1".parse().unwrap());
        let y = Some("2001:db8:1:2:ffff::9".parse().unwrap());
        let other = Some("2001:db8:1:3::1".parse().unwrap());
        assert!(rl.allow(x, t0) && rl.allow(y, t0));
        assert!(!rl.allow(x, t0), "same /64 shares the bucket");
        assert!(rl.allow(other, t0));
        assert_eq!(
            key_for(Some("::ffff:192.0.2.7".parse().unwrap())),
            "192.0.2.7".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn saturation_evicts_oldest_not_everyone() {
        let rl = RateLimiter::<IpAddr>::per_minute(1);
        let t0 = Instant::now();
        let attacker = Some("198.51.100.1".parse().unwrap());
        assert!(rl.allow(attacker, t0 + Duration::from_secs(5)));
        assert!(!rl.allow(attacker, t0 + Duration::from_secs(5)));
        for i in 0..MAX_TRACKED as u32 {
            let ip = IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + i));
            rl.allow(Some(ip), t0);
        }
        assert!(rl.buckets.lock().unwrap().len() <= MAX_TRACKED);
        assert!(
            !rl.allow(attacker, t0 + Duration::from_secs(5)),
            "a recently touched (limited) bucket survives saturation"
        );
    }
}
