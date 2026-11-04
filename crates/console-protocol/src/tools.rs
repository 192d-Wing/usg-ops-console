//! M4 tool-hosting contract: `/api/tools`, launch, `/_console/context`, the
//! identity assertion claims, and the header/cookie/route names shared by
//! the gateway, shell, SDK, and tool backends.
//!
//! Gateway-private registry config (upstream URLs, TLS, limits) is
//! deliberately not here: it is never serialized to browsers or tools.
//! Additive only: new fields carry `#[serde(default)]`; no
//! `deny_unknown_fields`, so older readers tolerate newer writers.
//!
//! Pinned proxy behaviour (INT-011; details in `docs/INTEGRATING-A-TOOL.md`):
//! - Every proxied request, static assets included, carries a fresh
//!   [`ASSERTION_HEADER`] (no caching: each has a unique `jti`).
//! - A malformed [`SCOPE_HEADER`] is dropped entirely (`ctx` all `null`).
//! - Upstreams receive `X-Forwarded-Host` = the tool origin host and
//!   `X-Forwarded-Proto: https`; client-supplied `X-Forwarded-*` are dropped.
//! - `/t/<id>` without a trailing slash answers `308` to `/t/<id>/`.
//! - `preferred_username`/`name` default to `""` when the IdP omits them.
//! - The JWKS is served with `Cache-Control: public, max-age=300`; verifiers
//!   should cache it for at most 10 minutes and refetch on an unknown `kid`.
//! - WebSocket / `Upgrade` / `CONNECT` requests answer `501` in v1.
//! - An upstream `Location` must be path-relative or on the tool origin;
//!   anything else is answered `502` (`bad_gateway`).

use serde::{Deserialize, Serialize};

use crate::message::Scope;

/// Request header carrying the gateway-signed identity assertion (compact
/// JWS) to tool upstreams. Client-supplied values are always stripped.
pub const ASSERTION_HEADER: &str = "x-console-assertion";
/// JOSE `typ` of the assertion.
pub const ASSERTION_TYP: &str = "console-assertion+jwt";
/// JOSE `alg` of the assertion; verifiers must pin it.
pub const ASSERTION_ALG: &str = "ES256";
/// Assertion lifetime (`exp - iat`), seconds.
pub const ASSERTION_TTL_SECS: u64 = 60;
/// Maximum clock leeway a verifier should allow, seconds.
pub const ASSERTION_MAX_LEEWAY_SECS: u64 = 30;
/// Advisory scope selection sent by the SDK fetch helper; the gateway parses
/// it into the assertion `ctx` and never forwards it raw.
pub const SCOPE_HEADER: &str = "x-console-scope";
/// Host-only session cookie on a tool origin.
pub const TOOL_SESSION_COOKIE: &str = "__Host-tool_sid";

/// Console origin: registry view for the current user (GET).
pub const ROUTE_TOOLS: &str = "/api/tools";
/// Console origin: `POST /api/tools/{id}/launch`.
pub fn launch_route(tool_id: &str) -> String {
    format!("/api/tools/{tool_id}/launch")
}
/// Console origin: public JWKS of assertion keys.
pub const ROUTE_JWKS: &str = "/.well-known/jwks.json";
/// Tool origin: one-time launch code handoff (`?code=`).
pub const ROUTE_TOOL_LAUNCH: &str = "/_console/launch";
/// Tool origin: [`ToolContext`] for the SDK origin pin (GET).
pub const ROUTE_TOOL_CONTEXT: &str = "/_console/context";
/// Tool origin: proxied prefix is `/t/<id>/`.
pub fn tool_base_path(tool_id: &str) -> String {
    format!("/t/{tool_id}/")
}

/// Max length of one scope header value.
pub const MAX_SCOPE_VALUE_LEN: usize = 64;

/// `GET /api/tools` response: tools the session's roles permit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolsResponse {
    #[serde(default)]
    pub tools: Vec<ToolSummary>,
}

/// Browser-safe registry view of one tool (no upstream, no limits).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSummary {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// console-ui icon key; unknown keys render a default icon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Tool browser origin, `https://host[:port]`, no path.
    pub origin: String,
    /// Path under `/t/<id>` loaded first.
    #[serde(default = "default_entry")]
    pub entry: String,
    #[serde(default)]
    pub nav: Vec<ToolNavItem>,
    /// Scope keys the tool honours (`site`, `tenant`, `environment`).
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// Lenient nav entry for HTTP (the manifest's `NavItem` is strict TOML).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolNavItem {
    pub label: String,
    /// Tool-relative path (`valid_path`).
    pub path: String,
}

fn default_entry() -> String {
    "/".into()
}

/// `POST /api/tools/{id}/launch` body (≤ 2 KiB).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchRequest {
    /// Tool-relative path incl. optional query, e.g. `/x?y=1`; defaults to
    /// the registry entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// `200` response to launch: set the iframe `src` to `url`
/// (`<origin>/_console/launch?code=<b64url>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchResponse {
    pub url: String,
}

/// `GET /_console/context` on a tool origin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolContext {
    pub tool_id: String,
    /// Exact `targetOrigin` for `postMessage` to the shell, and the only
    /// origin the SDK accepts messages from.
    pub console_origin: String,
    /// Highest SDK protocol the gateway's shell speaks.
    pub protocol: u32,
}

/// Claims of the `X-Console-Assertion` JWS (ES256). Unknown claims are
/// ignored; claims added later are optional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertionClaims {
    /// Console origin (`CONSOLE_PUBLIC_URL` origin).
    pub iss: String,
    /// Tool id; verifiers require `aud == own id`.
    pub aud: String,
    /// IdP subject.
    pub sub: String,
    #[serde(default)]
    pub preferred_username: String,
    #[serde(default)]
    pub name: String,
    /// Session roles filtered by the registry `forward_roles` prefixes.
    #[serde(default)]
    pub roles: Vec<String>,
    /// The user's current console *selection* (site/tenant/environment),
    /// taken from the client-supplied `X-Console-Scope` header and filtered
    /// to the registry `scopes` keys. It is **not an authorization
    /// decision**: the gateway only checks syntax. Tools must authorize the
    /// selected scope for `sub`/`roles` themselves (SEC-003). A malformed
    /// header is dropped entirely (all fields `null`).
    #[serde(default)]
    pub ctx: Scope,
    /// Per-session, per-tool pseudonym (HMAC, 16 B b64url); not linkable
    /// across tools and not the console session ref.
    pub sid: String,
    /// 128-bit random, b64url.
    pub jti: String,
    pub iat: u64,
    pub nbf: u64,
    pub exp: u64,
    /// Authentication context class from the console session, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acr: Option<String>,
}

fn valid_scope_value(v: &str) -> bool {
    (1..=MAX_SCOPE_VALUE_LEN).contains(&v.len())
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Render `X-Console-Scope` (`site=a;tenant=b`). Unset or invalid values
/// are omitted; `None` when nothing remains.
pub fn format_scope_header(scope: &Scope) -> Option<String> {
    let parts: Vec<String> = [
        ("site", &scope.site),
        ("tenant", &scope.tenant),
        ("environment", &scope.environment),
    ]
    .into_iter()
    .filter_map(|(k, v)| {
        v.as_deref()
            .filter(|v| valid_scope_value(v))
            .map(|v| format!("{k}={v}"))
    })
    .collect();
    (!parts.is_empty()).then(|| parts.join(";"))
}

/// Parse `X-Console-Scope`, keeping only keys in `allowed`. Any malformed
/// pair, duplicate, or bad value rejects the whole header (`None`); keys
/// that are well-formed but not allowed are skipped.
pub fn parse_scope_header(value: &str, allowed: &[impl AsRef<str>]) -> Option<Scope> {
    let mut scope = Scope::default();
    for pair in value.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=')?;
        if !valid_scope_value(v) {
            return None;
        }
        let slot = match k {
            "site" => &mut scope.site,
            "tenant" => &mut scope.tenant,
            "environment" => &mut scope.environment,
            _ => return None,
        };
        if slot.is_some() {
            return None;
        }
        if allowed.iter().any(|a| a.as_ref() == k) {
            *slot = Some(v.to_owned());
        }
    }
    Some(scope)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_header_round_trip_and_filtering() {
        let s = Scope {
            site: Some("dc1".into()),
            tenant: Some("t-2".into()),
            environment: None,
        };
        let h = format_scope_header(&s).unwrap();
        assert_eq!(h, "site=dc1;tenant=t-2");
        assert_eq!(parse_scope_header(&h, &["site", "tenant"]), Some(s));
        let only_site = parse_scope_header(&h, &["site"]).unwrap();
        assert_eq!(only_site.tenant, None);
        assert_eq!(format_scope_header(&Scope::default()), None);
    }

    #[test]
    fn scope_header_rejects_bad_input() {
        let allowed = ["site", "tenant", "environment"];
        for bad in [
            "site",
            "site=",
            "site=a b",
            "site=a;site=b",
            "role=admin",
            "site=a,b",
            &format!("site={}", "a".repeat(65)),
        ] {
            assert_eq!(parse_scope_header(bad, &allowed), None, "{bad}");
        }
    }

    #[test]
    fn routes() {
        assert_eq!(launch_route("hello"), "/api/tools/hello/launch");
        assert_eq!(tool_base_path("hello"), "/t/hello/");
    }
}
