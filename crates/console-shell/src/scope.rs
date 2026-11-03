//! Global scope (site -> tenant -> environment): URL query parsing, query
//! writing and cascade rules. Pure and Yew-free so it is tested natively.
//!
//! NIST 800-53: SI-10 - values are accepted only if they pass a strict charset
//! and exist in the allow-listed hierarchy; no percent-decoding is done.
//! NIST 800-53: SI-11 - errors carry the key name only, never the value.

use crate::demo::Site;
use console_protocol::Scope;
use std::fmt;

/// Upper bounds applied before parsing (plan 4.3 rule 6).
pub const MAX_QUERY_BYTES: usize = 2048;
pub const MAX_QUERY_PAIRS: usize = 32;

/// Recognised query keys, in the order they are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Site,
    Tenant,
    Env,
}

impl Key {
    pub const ALL: [Key; 3] = [Key::Site, Key::Tenant, Key::Env];

    pub fn as_str(self) -> &'static str {
        match self {
            Key::Site => "site",
            Key::Tenant => "tenant",
            Key::Env => "env",
        }
    }

    fn from_str(s: &str) -> Option<Key> {
        Key::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// A rejected part of the query. `Display` names the key only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeError {
    /// Bad charset, unknown id, duplicate key, or not valid under its parent.
    Invalid(Key),
    /// Query over [`MAX_QUERY_BYTES`] / [`MAX_QUERY_PAIRS`]; all of it ignored.
    TooLarge,
}

impl fmt::Display for ScopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScopeError::Invalid(k) => f.write_str(k.as_str()),
            ScopeError::TooLarge => f.write_str("query too long"),
        }
    }
}

/// Fixed user-facing message for a set of errors (no values echoed).
pub fn error_message(errors: &[ScopeError]) -> Option<String> {
    if errors.is_empty() {
        return None;
    }
    let names: Vec<String> = errors.iter().map(ToString::to_string).collect();
    Some(format!(
        "Ignored invalid scope in URL: {}",
        names.join(", ")
    ))
}

/// `[a-z0-9][a-z0-9-]{0,62}`, checked byte by byte.
pub fn valid_charset(v: &str) -> bool {
    let b = v.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

pub fn find_site<'a>(sites: &'a [Site], id: &str) -> Option<&'a Site> {
    sites.iter().find(|s| s.id == id)
}

pub fn find_tenant<'a>(site: &'a Site, id: &str) -> Option<&'a crate::demo::Tenant> {
    site.tenants.iter().find(|t| t.id == id)
}

/// Raw state of one key while scanning the query.
#[derive(Clone, Copy, PartialEq)]
enum Raw<'a> {
    Absent,
    /// Present with an empty value or without `=`: the "all" state.
    Empty,
    Value(&'a str),
    Duplicate,
}

fn split_pairs(q: &str) -> impl Iterator<Item = &str> {
    q.split('&').filter(|p| !p.is_empty())
}

fn strip_q(search: &str) -> &str {
    search.strip_prefix('?').unwrap_or(search)
}

/// Parse `location.search`. Invalid keys become `None` and are reported;
/// a lower level is dropped (and reported, if it had a value) when a higher
/// level is invalid or absent.
pub fn parse_query(search: &str, sites: &[Site]) -> (Scope, Vec<ScopeError>) {
    let q = strip_q(search);
    if q.len() > MAX_QUERY_BYTES || split_pairs(q).count() > MAX_QUERY_PAIRS {
        return (Scope::default(), vec![ScopeError::TooLarge]);
    }
    let mut raw = [Raw::Absent; 3];
    for pair in split_pairs(q) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let Some(key) = Key::from_str(k) else {
            continue;
        };
        let slot = &mut raw[key as usize];
        *slot = match *slot {
            Raw::Absent if v.is_empty() => Raw::Empty,
            Raw::Absent => Raw::Value(v),
            _ => Raw::Duplicate,
        };
    }

    let mut errors = Vec::new();
    let mut scope = Scope::default();
    // `parent_ok`: the level above is set and valid (site has no parent).
    let site = match raw[Key::Site as usize] {
        Raw::Value(v) if valid_charset(v) => find_site(sites, v),
        _ => None,
    };
    let reject = |r: Raw, key: Key, errors: &mut Vec<ScopeError>| {
        if matches!(r, Raw::Value(_) | Raw::Duplicate) {
            errors.push(ScopeError::Invalid(key));
        }
    };
    match site {
        Some(s) => scope.site = Some(s.id.to_owned()),
        None => reject(raw[Key::Site as usize], Key::Site, &mut errors),
    }
    let tenant = match (site, raw[Key::Tenant as usize]) {
        (Some(s), Raw::Value(v)) if valid_charset(v) => find_tenant(s, v),
        _ => None,
    };
    match tenant {
        Some(t) => scope.tenant = Some(t.id.to_owned()),
        None => reject(raw[Key::Tenant as usize], Key::Tenant, &mut errors),
    }
    let env = match (tenant, raw[Key::Env as usize]) {
        (Some(t), Raw::Value(v)) if valid_charset(v) => t.envs.iter().find(|e| **e == v),
        _ => None,
    };
    match env {
        Some(e) => scope.environment = Some((*e).to_owned()),
        None => reject(raw[Key::Env as usize], Key::Env, &mut errors),
    }
    (scope, errors)
}

/// Build `?site=..&tenant=..&env=..&<other params verbatim>` (or `""`), from
/// the existing `location.search`. Scope keys come first in fixed order;
/// other parameters keep their original order. A `#hash` suffix in `existing`
/// is preserved. Never emits an empty `?`.
pub fn to_query(existing: &str, scope: &Scope) -> String {
    let (search, hash) = match existing.find('#') {
        Some(i) => existing.split_at(i),
        None => (existing, ""),
    };
    let mut parts: Vec<String> = Vec::new();
    let vals = [&scope.site, &scope.tenant, &scope.environment];
    for (k, v) in Key::ALL.into_iter().zip(vals) {
        if let Some(v) = v {
            parts.push(format!("{}={v}", k.as_str()));
        }
    }
    for pair in split_pairs(strip_q(search)) {
        let k = pair.split_once('=').map_or(pair, |(k, _)| k);
        if Key::from_str(k).is_none() {
            parts.push(pair.to_owned());
        }
    }
    let mut out = String::new();
    if !parts.is_empty() {
        out.push('?');
        out.push_str(&parts.join("&"));
    }
    out.push_str(hash);
    out
}

/// What a cascade cleared, for the polite live-region announcement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reset {
    pub tenant: bool,
    pub env: bool,
    /// Reason text, built from allow-list labels (never from the URL).
    pub reason: String,
}

impl Reset {
    pub fn message(&self) -> Option<String> {
        let what = match (self.tenant, self.env) {
            (true, true) => "Tenant and environment cleared",
            (true, false) => "Tenant cleared",
            (false, true) => "Environment cleared",
            (false, false) => return None,
        };
        Some(format!("{what}: {}", self.reason))
    }
}

/// Change the site; clears tenant/env that are no longer valid under it.
pub fn set_site(cur: &Scope, site: Option<&str>, sites: &[Site]) -> (Scope, Option<Reset>) {
    let s = site.and_then(|id| find_site(sites, id));
    let mut next = Scope {
        site: s.map(|s| s.id.to_owned()),
        ..cur.clone()
    };
    let tenant = s.and_then(|s| next.tenant.as_deref().and_then(|t| find_tenant(s, t)));
    let tenant_cleared = next.tenant.is_some() && tenant.is_none();
    if tenant_cleared {
        next.tenant = None;
    }
    let env_ok = tenant.is_some_and(|t| {
        next.environment
            .as_deref()
            .is_some_and(|e| t.envs.contains(&e))
    });
    let env_cleared = next.environment.is_some() && !env_ok;
    if env_cleared {
        next.environment = None;
    }
    let reset = (tenant_cleared || env_cleared).then(|| Reset {
        tenant: tenant_cleared,
        env: env_cleared,
        reason: s.map_or_else(
            || "no site selected".to_owned(),
            |s| format!("not available at {}", s.name),
        ),
    });
    (next, reset)
}

/// Change the tenant (within the current site); clears an invalid env.
pub fn set_tenant(cur: &Scope, tenant: Option<&str>, sites: &[Site]) -> (Scope, Option<Reset>) {
    let s = cur.site.as_deref().and_then(|id| find_site(sites, id));
    let t = s.zip(tenant).and_then(|(s, id)| find_tenant(s, id));
    let mut next = Scope {
        tenant: t.map(|t| t.id.to_owned()),
        ..cur.clone()
    };
    let env_ok = t.is_some_and(|t| {
        next.environment
            .as_deref()
            .is_some_and(|e| t.envs.contains(&e))
    });
    let env_cleared = next.environment.is_some() && !env_ok;
    if env_cleared {
        next.environment = None;
    }
    let reset = env_cleared.then(|| Reset {
        tenant: false,
        env: true,
        reason: match (s, t) {
            (Some(s), Some(t)) => format!("not available for {} at {}", t.name, s.name),
            _ => "no tenant selected".to_owned(),
        },
    });
    (next, reset)
}

/// Change the environment (must exist for the current site+tenant).
pub fn set_env(cur: &Scope, env: Option<&str>, sites: &[Site]) -> Scope {
    let t = cur
        .site
        .as_deref()
        .and_then(|id| find_site(sites, id))
        .zip(cur.tenant.as_deref())
        .and_then(|(s, id)| find_tenant(s, id));
    let e = t
        .zip(env)
        .and_then(|(t, e)| t.envs.iter().find(|x| **x == e));
    Scope {
        environment: e.map(|e| (*e).to_owned()),
        ..cur.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::SITES;

    fn sc(site: Option<&str>, tenant: Option<&str>, env: Option<&str>) -> Scope {
        Scope {
            site: site.map(Into::into),
            tenant: tenant.map(Into::into),
            environment: env.map(Into::into),
        }
    }
    fn p(q: &str) -> (Scope, Vec<ScopeError>) {
        parse_query(q, SITES)
    }
    use ScopeError::Invalid;

    #[test]
    fn charset() {
        for ok in ["a", "0", "east", "a-b", "a1-", &"a".repeat(63)] {
            assert!(valid_charset(ok), "{ok}");
        }
        for bad in [
            "",
            "-a",
            "EAST",
            "East",
            "a_b",
            "a.b",
            "a b",
            "%3C",
            "a+b",
            "é",
            &"a".repeat(64),
        ] {
            assert!(!valid_charset(bad), "{bad}");
        }
    }

    #[test]
    fn deep_link_full_scope() {
        assert_eq!(
            p("?site=east&tenant=ops&env=prod"),
            (sc(Some("east"), Some("ops"), Some("prod")), vec![])
        );
        assert_eq!(p("site=hq"), (sc(Some("hq"), None, None), vec![]));
        assert_eq!(p(""), (Scope::default(), vec![]));
        assert_eq!(p("?"), (Scope::default(), vec![]));
    }

    #[test]
    fn rejects_bad_values_without_decoding() {
        for q in [
            "?site=EAST",
            "?site=%3Cscript%3E",
            "?site=<script>",
            "?site=ea+st",
            "?site=nowhere",
            "?site=east%20",
        ] {
            assert_eq!(p(q), (Scope::default(), vec![Invalid(Key::Site)]), "{q}");
        }
    }

    #[test]
    fn keys_exact_and_case_sensitive() {
        assert_eq!(
            p("?Site=east&SITE=east&environment=prod"),
            (Scope::default(), vec![])
        );
    }

    #[test]
    fn empty_and_bare_keys_are_all() {
        assert_eq!(p("?site=&tenant&env="), (Scope::default(), vec![]));
        assert_eq!(p("?site=hq&tenant="), (sc(Some("hq"), None, None), vec![]));
    }

    #[test]
    fn duplicates_rejected() {
        assert_eq!(
            p("?site=a&site=b"),
            (Scope::default(), vec![Invalid(Key::Site)])
        );
        assert_eq!(
            p("?site=hq&site=hq"),
            (Scope::default(), vec![Invalid(Key::Site)])
        );
        assert_eq!(
            p("?site=hq&site="),
            (Scope::default(), vec![Invalid(Key::Site)])
        );
    }

    #[test]
    fn oversize_ignored() {
        let long = format!("?site=hq&x={}", "a".repeat(MAX_QUERY_BYTES));
        assert_eq!(p(&long), (Scope::default(), vec![ScopeError::TooLarge]));
        let many = format!("?site=hq{}", "&x=1".repeat(MAX_QUERY_PAIRS));
        assert_eq!(p(&many), (Scope::default(), vec![ScopeError::TooLarge]));
        let ok = format!("?site=hq{}", "&x=1".repeat(MAX_QUERY_PAIRS - 1));
        assert_eq!(p(&ok).0.site.as_deref(), Some("hq"));
    }

    #[test]
    fn relationship_checks() {
        // tenant without site
        assert_eq!(
            p("?tenant=ops"),
            (Scope::default(), vec![Invalid(Key::Tenant)])
        );
        // tenant not at site (west has only logistics)
        assert_eq!(
            p("?site=west&tenant=ops"),
            (sc(Some("west"), None, None), vec![Invalid(Key::Tenant)])
        );
        // env not available for (site, tenant)
        assert_eq!(
            p("?site=east&tenant=ops&env=dev"),
            (sc(Some("east"), Some("ops"), None), vec![Invalid(Key::Env)])
        );
        // env without tenant
        assert_eq!(
            p("?site=hq&env=prod"),
            (sc(Some("hq"), None, None), vec![Invalid(Key::Env)])
        );
        // lower levels drop when a higher level is invalid
        assert_eq!(
            p("?site=EAST&tenant=ops&env=prod"),
            (
                Scope::default(),
                vec![Invalid(Key::Site), Invalid(Key::Tenant), Invalid(Key::Env)]
            )
        );
        // ...but an empty lower key is not reported
        assert_eq!(
            p("?site=bad&tenant="),
            (Scope::default(), vec![Invalid(Key::Site)])
        );
        // same tenant valid at two sites
        assert!(p("?site=hq&tenant=ops").1.is_empty());
        assert!(p("?site=east&tenant=ops").1.is_empty());
    }

    #[test]
    fn errors_never_contain_value() {
        let (_, errs) = p("?site=<script>alert(1)</script>&tenant=evil&env=x");
        let msg = error_message(&errs).unwrap();
        assert_eq!(msg, "Ignored invalid scope in URL: site, tenant, env");
        for bad in ["script", "alert", "evil", "<", "="] {
            assert!(!msg.contains(bad), "{bad}");
        }
        assert!(error_message(&[]).is_none());
        assert_eq!(
            error_message(&[ScopeError::TooLarge]).unwrap(),
            "Ignored invalid scope in URL: query too long"
        );
    }

    #[test]
    fn to_query_order_and_preservation() {
        let s = sc(Some("east"), Some("ops"), Some("prod"));
        assert_eq!(to_query("", &s), "?site=east&tenant=ops&env=prod");
        assert_eq!(
            to_query("?b=2&env=dev&a=1&site=x", &s),
            "?site=east&tenant=ops&env=prod&b=2&a=1"
        );
        assert_eq!(to_query("?a=%3C+&&b", &Scope::default()), "?a=%3C+&b");
        assert_eq!(to_query("?site=hq", &Scope::default()), "");
        assert_eq!(to_query("?site=hq#frag", &Scope::default()), "#frag");
        assert_eq!(
            to_query("?x=1#h", &sc(Some("hq"), None, None)),
            "?site=hq&x=1#h"
        );
    }

    #[test]
    fn round_trip_all_combinations() {
        let mut n = 0;
        let mut check = |s: Scope| {
            assert_eq!(p(&to_query("?keep=1", &s)), (s, vec![]));
            n += 1;
        };
        check(Scope::default());
        for site in SITES {
            check(sc(Some(site.id), None, None));
            for t in site.tenants {
                check(sc(Some(site.id), Some(t.id), None));
                for e in t.envs {
                    check(sc(Some(site.id), Some(t.id), Some(e)));
                }
            }
        }
        assert!(n > 10);
    }

    #[test]
    fn cascade_site_change() {
        let cur = sc(Some("hq"), Some("ops"), Some("dev"));
        // ops exists at east but dev does not
        let (n, r) = set_site(&cur, Some("east"), SITES);
        assert_eq!(n, sc(Some("east"), Some("ops"), None));
        assert_eq!(
            r.unwrap().message().unwrap(),
            "Environment cleared: not available at East DC"
        );
        // ops does not exist at west
        let (n, r) = set_site(&cur, Some("west"), SITES);
        assert_eq!(n, sc(Some("west"), None, None));
        assert_eq!(
            r.unwrap().message().unwrap(),
            "Tenant and environment cleared: not available at West DC"
        );
        let (n, r) = set_site(&sc(Some("hq"), Some("intel"), None), Some("east"), SITES);
        assert_eq!(n, sc(Some("east"), None, None));
        assert_eq!(
            r.unwrap().message().unwrap(),
            "Tenant cleared: not available at East DC"
        );
        // still valid: no reset
        let (n, r) = set_site(
            &sc(Some("hq"), Some("ops"), Some("prod")),
            Some("east"),
            SITES,
        );
        assert_eq!(n, sc(Some("east"), Some("ops"), Some("prod")));
        assert!(r.is_none());
        // clearing site clears everything
        let (n, r) = set_site(&cur, None, SITES);
        assert_eq!(n, Scope::default());
        assert_eq!(
            r.unwrap().message().unwrap(),
            "Tenant and environment cleared: no site selected"
        );
        // unknown site id is treated as "all"
        assert_eq!(
            set_site(&Scope::default(), Some("zzz"), SITES).0,
            Scope::default()
        );
    }

    #[test]
    fn cascade_tenant_and_env_change() {
        let cur = sc(Some("hq"), Some("ops"), Some("stage"));
        let (n, r) = set_tenant(&cur, Some("intel"), SITES);
        assert_eq!(n, sc(Some("hq"), Some("intel"), None));
        assert_eq!(
            r.unwrap().message().unwrap(),
            "Environment cleared: not available for Intelligence at Headquarters"
        );
        let (n, r) = set_tenant(&sc(Some("hq"), Some("ops"), None), Some("intel"), SITES);
        assert_eq!(n, sc(Some("hq"), Some("intel"), None));
        assert!(r.is_none());
        // tenant not at site / no site
        assert_eq!(
            set_tenant(&sc(Some("west"), None, None), Some("ops"), SITES)
                .0
                .tenant,
            None
        );
        assert_eq!(
            set_tenant(&Scope::default(), Some("ops"), SITES).0.tenant,
            None
        );
        // env
        assert_eq!(
            set_env(&cur, Some("dev"), SITES),
            sc(Some("hq"), Some("ops"), Some("dev"))
        );
        assert_eq!(
            set_env(&sc(Some("east"), Some("ops"), None), Some("dev"), SITES).environment,
            None
        );
        assert_eq!(set_env(&cur, None, SITES).environment, None);
    }

    /// F2: scope is a pure function of the URL, so every history entry,
    /// visited in either direction, yields the same scope (and therefore the
    /// same picker values) as when it was first pushed.
    #[test]
    fn history_back_forward_round_trip() {
        let start = "?site=hq&tenant=intel";
        let (s0, e0) = p(start);
        assert!(e0.is_empty());
        let (s1, _) = set_site(&s0, Some("east"), SITES);
        let next = to_query(start, &s1);
        let entries = [start.to_string(), next];
        let expect = [
            sc(Some("hq"), Some("intel"), None),
            sc(Some("east"), None, None),
        ];
        // Push, Back, Forward, Back, Forward.
        for idx in [0usize, 1, 0, 1, 0, 1] {
            assert_eq!(p(&entries[idx]), (expect[idx].clone(), vec![]), "{idx}");
        }
    }

    // ---- QA additions (M2 qa-engineer): parse edge cases, cascade, history ----

    #[test]
    fn qa_repeated_lower_keys_rejected_upper_kept() {
        assert_eq!(
            p("?site=hq&tenant=ops&tenant=ops"),
            (sc(Some("hq"), None, None), vec![Invalid(Key::Tenant)])
        );
        assert_eq!(
            p("?site=hq&tenant=ops&env=prod&env=dev"),
            (sc(Some("hq"), Some("ops"), None), vec![Invalid(Key::Env)])
        );
        // duplicate empty is still a duplicate
        assert_eq!(
            p("?site=&site="),
            (Scope::default(), vec![Invalid(Key::Site)])
        );
    }

    #[test]
    fn qa_size_limits_exact_boundaries() {
        // exactly MAX_QUERY_BYTES (after '?') is accepted
        let head = "site=hq&x=";
        let q = format!("?{head}{}", "a".repeat(MAX_QUERY_BYTES - head.len()));
        assert_eq!(q.len() - 1, MAX_QUERY_BYTES);
        assert_eq!(p(&q), (sc(Some("hq"), None, None), vec![]));
        // one byte more is rejected entirely
        let q = format!("?{head}{}", "a".repeat(MAX_QUERY_BYTES - head.len() + 1));
        assert_eq!(p(&q), (Scope::default(), vec![ScopeError::TooLarge]));
        // exactly MAX_QUERY_PAIRS accepted, +1 rejected
        let q = format!("?site=hq{}", "&x=1".repeat(MAX_QUERY_PAIRS - 1));
        assert_eq!(p(&q).1, vec![]);
        let q = format!("?site=hq{}", "&x=1".repeat(MAX_QUERY_PAIRS));
        assert_eq!(p(&q).1, vec![ScopeError::TooLarge]);
        // empty segments ("&&") are not pairs
        let q = format!("?site=hq{}", "&&x=1".repeat(MAX_QUERY_PAIRS - 1));
        assert_eq!(p(&q).1, vec![]);
        // the TooLarge message is fixed text
        assert_eq!(
            error_message(&[ScopeError::TooLarge]).unwrap(),
            "Ignored invalid scope in URL: query too long"
        );
    }

    #[test]
    fn qa_uppercase_and_percent_encoding_rejected_at_every_level() {
        for (q, want) in [
            (
                "?site=hq&tenant=OPS",
                (sc(Some("hq"), None, None), vec![Invalid(Key::Tenant)]),
            ),
            (
                "?site=hq&tenant=ops&env=PROD",
                (sc(Some("hq"), Some("ops"), None), vec![Invalid(Key::Env)]),
            ),
            // %68%71 == "hq": values are never percent-decoded
            ("?site=%68%71", (Scope::default(), vec![Invalid(Key::Site)])),
            (
                "?site=hq&tenant=%6Fps",
                (sc(Some("hq"), None, None), vec![Invalid(Key::Tenant)]),
            ),
            (
                "?site=hq&tenant=ops&env=pro%64",
                (sc(Some("hq"), Some("ops"), None), vec![Invalid(Key::Env)]),
            ),
            // encoded key names are not the key
            ("?%73ite=hq", (Scope::default(), vec![])),
        ] {
            assert_eq!(p(q), want, "{q}");
        }
    }

    #[test]
    fn qa_tenant_or_env_without_site() {
        assert_eq!(p("?env=prod"), (Scope::default(), vec![Invalid(Key::Env)]));
        assert_eq!(
            p("?tenant=ops&env=prod"),
            (
                Scope::default(),
                vec![Invalid(Key::Tenant), Invalid(Key::Env)]
            )
        );
        // key order in the URL does not matter
        assert_eq!(
            p("?env=prod&tenant=ops&site=hq"),
            (sc(Some("hq"), Some("ops"), Some("prod")), vec![])
        );
    }

    #[test]
    fn qa_cross_site_pairs() {
        // intel exists only at hq
        assert_eq!(
            p("?site=east&tenant=intel"),
            (sc(Some("east"), None, None), vec![Invalid(Key::Tenant)])
        );
        // ops/dev valid at hq, not at east
        assert!(p("?site=hq&tenant=ops&env=dev").1.is_empty());
        assert_eq!(
            p("?site=east&tenant=ops&env=dev"),
            (sc(Some("east"), Some("ops"), None), vec![Invalid(Key::Env)])
        );
        // every (site, tenant, env) in demo data parses cleanly; every tenant
        // id at a site where it is absent is rejected
        for s in SITES {
            for t in s.tenants {
                for e in t.envs {
                    let q = format!("?site={}&tenant={}&env={}", s.id, t.id, e);
                    assert_eq!(p(&q), (sc(Some(s.id), Some(t.id), Some(e)), vec![]), "{q}");
                }
                for other in SITES.iter().filter(|o| find_tenant(o, t.id).is_none()) {
                    let q = format!("?site={}&tenant={}", other.id, t.id);
                    assert_eq!(p(&q).1, vec![Invalid(Key::Tenant)], "{q}");
                }
            }
        }
    }

    #[test]
    fn qa_cascade_results_are_always_valid() {
        // Exhaustive: from every valid scope, every set_site/set_tenant/set_env
        // yields a scope that re-parses with no errors (cascade cleared all
        // invalid lower levels in the same step).
        let mut scopes = vec![Scope::default()];
        for s in SITES {
            scopes.push(sc(Some(s.id), None, None));
            for t in s.tenants {
                scopes.push(sc(Some(s.id), Some(t.id), None));
                for e in t.envs {
                    scopes.push(sc(Some(s.id), Some(t.id), Some(e)));
                }
            }
        }
        let ids: Vec<&str> = SITES
            .iter()
            .flat_map(|s| {
                std::iter::once(s.id)
                    .chain(s.tenants.iter().map(|t| t.id))
                    .chain(s.tenants.iter().flat_map(|t| t.envs.iter().copied()))
            })
            .collect();
        for cur in &scopes {
            for id in ids.iter().map(|i| Some(*i)).chain([None]) {
                let a = set_site(cur, id, SITES).0;
                let b = set_tenant(cur, id, SITES).0;
                let c = set_env(cur, id, SITES);
                for n in [a, b, c] {
                    let (back, errs) = p(&to_query("", &n));
                    assert!(errs.is_empty(), "{cur:?} -> {n:?}");
                    assert_eq!(back, n);
                }
            }
        }
    }

    #[test]
    fn qa_reset_none_when_nothing_cleared() {
        assert!(set_site(&Scope::default(), Some("hq"), SITES).1.is_none());
        assert!(
            set_site(&sc(Some("hq"), None, None), None, SITES)
                .1
                .is_none()
        );
        assert!(
            set_tenant(&sc(Some("hq"), Some("ops"), None), None, SITES)
                .1
                .is_none()
        );
    }

    #[test]
    fn qa_history_multi_step_with_cascades() {
        // Each picker change is one entry; cascaded clears live in the same
        // entry. Walking Back over all entries restores each prior full scope.
        let mut cur_q = String::from("?foo=1");
        let mut entries = vec![(cur_q.clone(), Scope::default())];
        let steps: [&dyn Fn(&Scope) -> Scope; 5] = [
            &|s| set_site(s, Some("hq"), SITES).0,
            &|s| set_tenant(s, Some("ops"), SITES).0,
            &|s| set_env(s, Some("dev"), SITES),
            &|s| set_site(s, Some("east"), SITES).0, // clears env
            &|s| set_site(s, Some("west"), SITES).0, // clears tenant
        ];
        for step in steps {
            let next = step(&entries.last().unwrap().1);
            cur_q = to_query(&cur_q, &next);
            entries.push((cur_q.clone(), next));
        }
        assert_eq!(entries[3].1, sc(Some("hq"), Some("ops"), Some("dev")));
        assert_eq!(entries[4].1, sc(Some("east"), Some("ops"), None));
        assert_eq!(entries[5].1, sc(Some("west"), None, None));
        for (q, want) in entries.iter().rev().chain(entries.iter()) {
            assert_eq!(p(q), (want.clone(), vec![]), "{q}");
            assert!(q.contains("foo=1"), "{q}");
        }
    }
}
