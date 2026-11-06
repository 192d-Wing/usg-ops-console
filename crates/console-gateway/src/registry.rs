//! Tool registry (`CONSOLE_TOOLS_REGISTRY_FILE`, TOML): the gateway's
//! allow-list of tools. It is authoritative: upstreams, origins, roles and
//! limits come only from here, never from a tool's manifest (PLAN section 2).
//! Validation is fail-closed; every error names the file, the tool and the
//! field. Contract: plan.md section 4.1 plus the WU2 CONTRACT DELTA.
//!
//! Schema stability (INT-010): the TOML layout is a versioned operator
//! interface. Optional top-level `schema = 1` pins the version this gateway
//! understands; any other value fails start-up. Tables keep
//! `deny_unknown_fields` (a typo must not silently drop a control), so a new
//! key may be added to a deployed registry only after every gateway replica
//! (including the rollback target) understands it. Keys added after plan 4.1:
//! `insecure_http_upstream`, `insecure_http_upstream_reason`, nav `label`
//! cap (64), upstream path must be `/`, `forward_roles` are prefixes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderName;
use console_protocol::{ToolNavItem, ToolSummary, valid_id, valid_path};
use serde::Deserialize;
use url::Url;

use crate::assertion::AssertionKeys;
use crate::config::{Config, is_loopback_host};
use crate::proxy::Upstream;
use crate::rate_limit::RateLimiter;

pub const MAX_TOOLS: usize = 64;
pub const DEFAULT_MAX_BODY: u64 = 10 * 1024 * 1024;
pub const MAX_MAX_BODY: u64 = 100 * 1024 * 1024;
pub const DEFAULT_HEADER_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_MAX_CONCURRENT: u32 = 256;
const MAX_CONCURRENT_LIMIT: u32 = 4096;
const MAX_ROLES: usize = 32;
const MAX_ROLE_LEN: usize = 128;
const MAX_NAV: usize = 32;
const MAX_LABEL: usize = 64;
const MAX_TITLE: usize = 128;
const MAX_FORWARD_HEADERS: usize = 16;
const SCOPE_KEYS: [&str; 3] = ["site", "tenant", "environment"];
/// The registry schema version this gateway understands (`schema = 1`).
pub const REGISTRY_SCHEMA: u32 = 1;
const MAX_REASON: usize = 200;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRegistry {
    /// Registry schema version; only [`REGISTRY_SCHEMA`] is accepted.
    #[serde(default)]
    schema: Option<u32>,
    #[serde(default)]
    tool: Vec<RawTool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTool {
    id: String,
    title: String,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    origin: String,
    upstream: String,
    #[serde(default)]
    required_roles: Vec<String>,
    #[serde(default)]
    forward_roles: Option<Vec<String>>,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    entry: Option<String>,
    #[serde(default)]
    upstream_ca_file: Option<String>,
    #[serde(default)]
    max_body_bytes: Option<u64>,
    #[serde(default)]
    response_header_timeout_secs: Option<u64>,
    #[serde(default)]
    max_concurrent: Option<u32>,
    #[serde(default)]
    forward_headers: Vec<String>,
    #[serde(default)]
    audit_writes: bool,
    #[serde(default)]
    insecure_http_upstream: bool,
    /// Required with `insecure_http_upstream = true` (NIST-003): why
    /// cleartext is acceptable, e.g. the compensating mesh mTLS control.
    #[serde(default)]
    insecure_http_upstream_reason: Option<String>,
    #[serde(default)]
    nav: Vec<RawNav>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNav {
    label: String,
    path: String,
}

/// One validated registry entry.
#[derive(Debug, Clone)]
pub struct ToolConfig {
    pub id: String,
    pub title: String,
    pub category: Option<String>,
    pub icon: Option<String>,
    /// Canonical browser origin, `https://host[:port]`.
    pub origin: String,
    /// Lower-case `host[:port]` (port omitted when 443): the `Host` match key.
    pub host: String,
    /// Scheme + authority only; the request path is appended verbatim.
    pub upstream: Url,
    pub upstream_ca_file: Option<String>,
    /// Any-of; empty means any console user.
    pub required_roles: Vec<String>,
    /// Prefixes of session roles copied into the assertion and `Init`.
    pub forward_roles: Vec<String>,
    pub scopes: Vec<String>,
    pub entry: String,
    pub nav: Vec<ToolNavItem>,
    pub max_body_bytes: u64,
    pub response_header_timeout: Duration,
    pub max_concurrent: u32,
    pub forward_headers: Vec<HeaderName>,
    pub audit_writes: bool,
    /// Set when a plain-http upstream to a non-loopback host is allowed by
    /// `insecure_http_upstream` (the operator's reason; audited at start-up).
    pub insecure_upstream_reason: Option<String>,
}

impl ToolConfig {
    // COMPLIANCE MAPPING:
    // - NIST 800-53: AC-3 (Access Enforcement) — registry required_roles, any-of, checked on list/launch/proxy
    pub fn permits(&self, roles: &[String]) -> bool {
        self.required_roles.is_empty() || self.required_roles.iter().any(|r| roles.contains(r))
    }

    /// Session roles a tool may see (AC-6: minimisation).
    pub fn forwarded_roles(&self, roles: &[String]) -> Vec<String> {
        roles
            .iter()
            .filter(|r| self.forward_roles.iter().any(|p| r.starts_with(p.as_str())))
            .cloned()
            .collect()
    }

    /// Whether a session should keep `role` because this tool uses it.
    pub fn uses_role(&self, role: &str) -> bool {
        self.required_roles.iter().any(|r| r == role)
            || self
                .forward_roles
                .iter()
                .any(|p| role.starts_with(p.as_str()))
    }

    /// Browser-safe view (no upstream, no limits).
    pub fn summary(&self) -> ToolSummary {
        ToolSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            category: self.category.clone(),
            icon: self.icon.clone(),
            origin: self.origin.clone(),
            entry: self.entry.clone(),
            nav: self.nav.clone(),
            scopes: self.scopes.clone(),
        }
    }

    /// `scheme://host[:port]` of the upstream (for `Location` rewriting).
    pub fn upstream_origin(&self) -> String {
        self.upstream.origin().ascii_serialization()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("{file}: not a valid tool registry: {msg}")]
    Parse { file: String, msg: String },
    #[error("{file}: tool {tool:?}: {field}: {why}")]
    Invalid {
        file: String,
        tool: String,
        field: &'static str,
        why: String,
    },
    #[error("{file}: at most {MAX_TOOLS} tools may be registered")]
    TooMany { file: String },
}

/// Lower-case `host[:port]` with the default https port removed; used for
/// both registry origins and request `Host` headers.
pub fn host_key(host: &str) -> String {
    let h = host.trim().to_ascii_lowercase();
    match h.strip_suffix(":443") {
        Some(rest) => rest.to_owned(),
        None => h,
    }
}

fn plain_text(v: &str, max: usize) -> bool {
    !v.trim().is_empty() && v.len() <= max && !v.chars().any(char::is_control)
}

fn role_ok(r: &str) -> bool {
    !r.is_empty()
        && r.len() <= MAX_ROLE_LEN
        && !r.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Request headers a registry entry may never ask to forward.
fn reserved_header(name: &str) -> bool {
    const EXACT: [&str; 17] = [
        "authorization",
        "cookie",
        "host",
        "forwarded",
        "connection",
        "keep-alive",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "x-real-ip",
        "x-request-id",
        "remote-user",
        "x-remote-user",
        "x-user",
        "set-cookie",
        "content-length",
    ];
    EXACT.contains(&name)
        || name.starts_with("proxy-")
        || name.starts_with("x-forwarded-")
        || name.starts_with("x-console-")
}

// COMPLIANCE MAPPING:
// - NIST 800-53: CM-7 (Least Functionality), CM-3 (Configuration Change Control) — fail-closed allow-list from reviewed config
// - NIST 800-53: SI-10 (Information Input Validation) — every field validated at start-up
// - NIST 800-53: SC-8 (Transmission Confidentiality) — https upstreams with verification off-loopback, except
//   (CM-6 deviation) `insecure_http_upstream` with a mandatory reason, start-up warning and audit event
// POAM: SC-8 - insecure_http_upstream relies on a compensating control outside the gateway (e.g. mesh mTLS) that is not verified here.
/// Parses and validates the registry. `console` is `CONSOLE_PUBLIC_URL`.
/// Non-fatal findings (empty registry, plain-http upstreams) go to
/// `warnings`.
pub fn parse(
    file: &str,
    text: &str,
    console: &Url,
    dev_mode: bool,
    warnings: &mut Vec<String>,
) -> Result<Vec<ToolConfig>, RegistryError> {
    let raw: RawRegistry = toml::from_str(text).map_err(|e| RegistryError::Parse {
        file: file.to_owned(),
        msg: e.message().to_owned(),
    })?;
    if raw.schema.is_some_and(|v| v != REGISTRY_SCHEMA) {
        return Err(RegistryError::Parse {
            file: file.to_owned(),
            msg: format!(
                "unsupported schema (this gateway understands schema = {REGISTRY_SCHEMA})"
            ),
        });
    }
    if raw.tool.is_empty() {
        warnings.push(format!("{file}: registry has no tools"));
    }
    if raw.tool.len() > MAX_TOOLS {
        return Err(RegistryError::TooMany {
            file: file.to_owned(),
        });
    }
    let console_origin = console.origin().ascii_serialization();
    let console_host = console.host_str().unwrap_or_default().to_ascii_lowercase();
    let mut ids = HashSet::new();
    let mut hosts = HashSet::new();
    let mut out = Vec::with_capacity(raw.tool.len());
    for t in raw.tool {
        let tool_name = t.id.clone();
        let bad = |field: &'static str, why: &str| RegistryError::Invalid {
            file: file.to_owned(),
            tool: tool_name.clone(),
            field,
            why: why.to_owned(),
        };
        if !valid_id(&t.id) {
            return Err(bad(
                "id",
                "must be 1-32 chars [a-z0-9-] starting with a letter",
            ));
        }
        if !ids.insert(t.id.clone()) {
            return Err(bad("id", "duplicate id"));
        }
        if !plain_text(&t.title, MAX_TITLE) {
            return Err(bad("title", "must be 1-128 chars of plain text"));
        }
        for (field, v) in [("category", &t.category), ("icon", &t.icon)] {
            if v.as_deref().is_some_and(|v| !plain_text(v, MAX_LABEL)) {
                return Err(bad(field, "must be 1-64 chars of plain text"));
            }
        }

        // Origin: canonical https://host[:port], unique, outside the console host.
        let origin_url = Url::parse(&t.origin).map_err(|_| bad("origin", "not a URL"))?;
        let canonical = origin_url.origin().ascii_serialization();
        if origin_url.scheme() != "https"
            || origin_url.host_str().is_none()
            || t.origin.trim_end_matches('/') != canonical
        {
            return Err(bad(
                "origin",
                "must be a canonical https://host[:port] origin without path",
            ));
        }
        let tool_host = origin_url
            .host_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if canonical == console_origin {
            return Err(bad("origin", "must not be the console origin"));
        }
        if tool_host.ends_with(&format!(".{console_host}")) {
            return Err(bad(
                "origin",
                "must not be a subdomain of the console host (use a sibling such as <id>.tools.<domain>)",
            ));
        }
        let host = host_key(&match origin_url.port() {
            Some(p) => format!("{tool_host}:{p}"),
            None => tool_host.clone(),
        });
        if !hosts.insert(host.clone()) {
            return Err(bad("origin", "duplicate origin"));
        }

        // Upstream: http(s), authority only, verified TLS off-loopback.
        let upstream = Url::parse(&t.upstream).map_err(|_| bad("upstream", "not a URL"))?;
        if upstream.host_str().is_none()
            || !upstream.username().is_empty()
            || upstream.password().is_some()
            || upstream.query().is_some()
            || upstream.fragment().is_some()
            || upstream.path() != "/"
        {
            return Err(bad(
                "upstream",
                "must be scheme://host[:port] without credentials, path or query",
            ));
        }
        let reason = match (t.insecure_http_upstream, t.insecure_http_upstream_reason) {
            (false, None) => None,
            (false, Some(_)) => {
                return Err(bad(
                    "insecure_http_upstream_reason",
                    "only valid with insecure_http_upstream = true",
                ));
            }
            (true, Some(r)) if plain_text(&r, MAX_REASON) => Some(r),
            (true, _) => {
                return Err(bad(
                    "insecure_http_upstream_reason",
                    "required with insecure_http_upstream = true: 1-200 chars naming the \
                     compensating control (e.g. mesh mTLS)",
                ));
            }
        };
        let mut insecure_upstream_reason = None;
        match upstream.scheme() {
            "https" => {}
            "http" if is_loopback_host(&upstream) => {}
            "http" if dev_mode && reason.is_none() => warnings.push(format!(
                "{file}: tool {:?}: plain-http upstream to a non-loopback host (no TLS); allowed by CONSOLE_DEV_MODE",
                t.id
            )),
            "http" if reason.is_some() => {
                warnings.push(format!(
                    "{file}: tool {:?}: plain-http upstream to a non-loopback host (no TLS); allowed by \
                     insecure_http_upstream (CM-6 deviation; compensating control required)",
                    t.id
                ));
                insecure_upstream_reason = reason;
            }
            "http" => {
                return Err(bad(
                    "upstream",
                    "non-loopback upstream must be https (or set insecure_http_upstream = true with \
                     insecure_http_upstream_reason for a cluster-internal name)",
                ));
            }
            _ => return Err(bad("upstream", "scheme must be http or https")),
        }
        if t.upstream_ca_file.as_deref().is_some_and(str::is_empty) {
            return Err(bad("upstream_ca_file", "must not be empty"));
        }

        if t.required_roles.len() > MAX_ROLES || !t.required_roles.iter().all(|r| role_ok(r)) {
            return Err(bad(
                "required_roles",
                "at most 32 roles, each 1-128 chars without whitespace",
            ));
        }
        let forward_roles = t.forward_roles.unwrap_or_else(|| t.required_roles.clone());
        if forward_roles.len() > MAX_ROLES || !forward_roles.iter().all(|r| role_ok(r)) {
            return Err(bad(
                "forward_roles",
                "at most 32 prefixes, each 1-128 chars without whitespace",
            ));
        }
        let mut seen = HashSet::new();
        if !t
            .scopes
            .iter()
            .all(|k| SCOPE_KEYS.contains(&k.as_str()) && seen.insert(k.as_str()))
        {
            return Err(bad(
                "scopes",
                "keys must be unique values of site, tenant, environment",
            ));
        }
        let entry = t.entry.unwrap_or_else(|| "/".into());
        if !valid_path(&entry) {
            return Err(bad(
                "entry",
                "must be a path starting with / (no .., //, :)",
            ));
        }
        if t.nav.len() > MAX_NAV {
            return Err(bad("nav", "at most 32 entries"));
        }
        let mut nav = Vec::with_capacity(t.nav.len());
        for n in t.nav {
            if !plain_text(&n.label, MAX_LABEL) || !valid_path(&n.path) {
                return Err(bad(
                    "nav",
                    "label must be 1-64 chars; path must start with / (no .., //, :)",
                ));
            }
            nav.push(ToolNavItem {
                label: n.label,
                path: n.path,
            });
        }

        let max_body_bytes = t.max_body_bytes.unwrap_or(DEFAULT_MAX_BODY);
        if !(1..=MAX_MAX_BODY).contains(&max_body_bytes) {
            return Err(bad("max_body_bytes", "must be in 1..=104857600"));
        }
        let timeout = t
            .response_header_timeout_secs
            .unwrap_or(DEFAULT_HEADER_TIMEOUT_SECS);
        if !(1..=300).contains(&timeout) {
            return Err(bad("response_header_timeout_secs", "must be in 1..=300"));
        }
        let max_concurrent = t.max_concurrent.unwrap_or(DEFAULT_MAX_CONCURRENT);
        if !(1..=MAX_CONCURRENT_LIMIT).contains(&max_concurrent) {
            return Err(bad("max_concurrent", "must be in 1..=4096"));
        }
        if t.forward_headers.len() > MAX_FORWARD_HEADERS {
            return Err(bad("forward_headers", "at most 16 headers"));
        }
        let mut forward_headers = Vec::with_capacity(t.forward_headers.len());
        for h in &t.forward_headers {
            let token = !h.is_empty()
                && h.len() <= 64
                && h.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !token || reserved_header(h) {
                return Err(bad(
                    "forward_headers",
                    "lower-case header names only; identity, hop-by-hop, cookie, authorization, \
                     host, forwarded, x-forwarded-* and x-console-* are reserved",
                ));
            }
            forward_headers.push(
                HeaderName::from_bytes(h.as_bytes())
                    .map_err(|_| bad("forward_headers", "not a header name"))?,
            );
        }

        out.push(ToolConfig {
            id: t.id,
            title: t.title,
            category: t.category,
            icon: t.icon,
            origin: canonical,
            host,
            upstream,
            upstream_ca_file: t.upstream_ca_file,
            required_roles: t.required_roles,
            forward_roles,
            scopes: t.scopes,
            entry,
            nav,
            max_body_bytes,
            response_header_timeout: Duration::from_secs(timeout),
            max_concurrent,
            forward_headers,
            audit_writes: t.audit_writes,
            insecure_upstream_reason,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Runtime registry
// ---------------------------------------------------------------------------

/// A registered tool plus its upstream client.
#[derive(Debug)]
pub struct Tool {
    pub cfg: ToolConfig,
    pub upstream: Upstream,
}

/// Launches per console session per minute (SC-5).
pub const LAUNCH_PER_MINUTE: u32 = 60;
/// Unauthenticated tool-origin denials audited per client IP per minute
/// (bounds audit amplification from bad codes / misdirected requests).
pub const DENIAL_AUDIT_PER_MINUTE: u32 = 30;

/// Everything M4 adds to the gateway state: tools by id and host, the
/// assertion keys, and the tool-related rate limits.
pub struct ToolsState {
    tools: Vec<Arc<Tool>>,
    by_host: HashMap<String, Arc<Tool>>,
    keys: Option<AssertionKeys>,
    pub launch_limiter: RateLimiter<Vec<u8>>,
    /// Spoofed-identity audit: one record per tool session per minute.
    pub spoof_audit: RateLimiter<Vec<u8>>,
    /// `/_console/launch` attempts per IP, and unauthenticated denial audits.
    pub code_limiter: RateLimiter,
    pub denial_audit: RateLimiter,
}

impl std::fmt::Debug for ToolsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolsState")
            .field(
                "tools",
                &self.tools.iter().map(|t| &t.cfg.id).collect::<Vec<_>>(),
            )
            .field("keys", &self.keys)
            .finish()
    }
}

impl ToolsState {
    /// No tools (and no keys): the M3 surface only.
    pub fn empty() -> Self {
        Self {
            tools: Vec::new(),
            by_host: HashMap::new(),
            keys: None,
            launch_limiter: RateLimiter::per_minute(LAUNCH_PER_MINUTE),
            spoof_audit: RateLimiter::per_minute(1),
            code_limiter: RateLimiter::per_minute(LAUNCH_PER_MINUTE),
            denial_audit: RateLimiter::per_minute(DENIAL_AUDIT_PER_MINUTE),
        }
    }

    /// Builds clients for `tools`. Keys are required when any tool exists
    /// (fail closed: no unsigned identity ever reaches an upstream).
    pub fn new(
        tools: Vec<ToolConfig>,
        keys: Option<AssertionKeys>,
        read_file: &dyn Fn(&str) -> Option<Vec<u8>>,
    ) -> Result<Self, String> {
        if !tools.is_empty() && keys.is_none() {
            return Err(
                "CONSOLE_ASSERTION_KEY_FILE is required when the registry has tools".into(),
            );
        }
        let roots = if tools.iter().any(|t| t.upstream.scheme() == "https") {
            crate::proxy::base_roots()
        } else {
            rustls::RootCertStore::empty()
        };
        let mut state = Self::empty();
        state.keys = keys;
        for cfg in tools {
            let upstream = crate::proxy::build_upstream(&cfg, &roots, read_file)?;
            let tool = Arc::new(Tool { cfg, upstream });
            state.by_host.insert(tool.cfg.host.clone(), tool.clone());
            state.tools.push(tool);
        }
        Ok(state)
    }

    /// From `Config`: parse the registry file and load the keys. Warnings
    /// are appended (never secret values).
    // COMPLIANCE MAPPING:
    // - NIST 800-53: SC-12 (Cryptographic Key Establishment and Management) — separate sid HMAC key required outside dev mode (fail closed)
    pub fn from_config(cfg: &Config, warnings: &mut Vec<String>) -> Result<Self, String> {
        let tools = match &cfg.tools_registry {
            None => Vec::new(),
            Some((path, text)) => parse(path, text, &cfg.public_url, cfg.dev_mode, warnings)
                .map_err(|e| e.to_string())?,
        };
        // SEC-005: outside dev mode the sid pseudonym key must be its own
        // secret, so rotating the signing key does not re-key every `sid`
        // and one leaked key does not yield the other.
        if !tools.is_empty() && !cfg.dev_mode && cfg.tool_sid_key.is_none() {
            return Err(
                "CONSOLE_TOOL_SESSION_HMAC_KEY_FILE is required when the registry has tools \
                 (only CONSOLE_DEV_MODE may derive it from the assertion key)"
                    .into(),
            );
        }
        if !tools.is_empty() && cfg.dev_mode && cfg.tool_sid_key.is_none() {
            warnings.push(
                "CONSOLE_TOOL_SESSION_HMAC_KEY_FILE unset (dev mode): sid pseudonyms are derived \
                 from the assertion signing key"
                    .into(),
            );
        }
        let keys = match &cfg.assertion_key {
            None => None,
            Some(pem) => Some(
                AssertionKeys::from_pem(
                    pem.expose(),
                    cfg.assertion_publish_keys.as_deref(),
                    cfg.tool_sid_key.as_ref().map(|k| k.expose()),
                )
                .map_err(|e| e.to_string())?,
            ),
        };
        Self::new(tools, keys, &|p| std::fs::read(p).ok())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<Tool>> {
        self.tools.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn by_id(&self, id: &str) -> Option<&Arc<Tool>> {
        self.tools.iter().find(|t| t.cfg.id == id)
    }

    /// Tool served on request host `host` (`Host` header / authority).
    pub fn by_host(&self, host: &str) -> Option<&Arc<Tool>> {
        self.by_host.get(&host_key(host))
    }

    /// Registry origins (console CSP `frame-src`).
    pub fn origins(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.cfg.origin.clone()).collect()
    }

    /// Whether a session keeps `role` because some tool uses it.
    pub fn keeps_role(&self, role: &str) -> bool {
        self.tools.iter().any(|t| t.cfg.uses_role(role))
    }

    pub fn keys(&self) -> Option<&AssertionKeys> {
        self.keys.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
[[tool]]
id = "hello"
title = "Hello Tool"
category = "Examples"
icon = "puzzle"
origin = "https://hello.tools.ops.example.mil"
upstream = "http://127.0.0.1:8090"
required_roles = ["hello:user"]
forward_roles = ["hello:"]
scopes = ["site", "tenant"]
forward_headers = ["x-hello-csrf"]
audit_writes = true
[[tool.nav]]
label = "Overview"
path = "/"

[[tool]]
id = "evil"
title = "Evil"
origin = "https://localhost:9448"
upstream = "https://evil.internal:8443"
"#;

    fn console() -> Url {
        Url::parse("https://console.ops.example.mil").unwrap()
    }

    fn try_parse(text: &str, dev: bool) -> Result<Vec<ToolConfig>, RegistryError> {
        parse("reg.toml", text, &console(), dev, &mut Vec::new())
    }

    /// `${CONSOLE_*}` rendering as `make tools-registries` does, from ports.env.
    fn render_dev_registry(text: &str) -> String {
        let ports: std::collections::HashMap<&str, &str> =
            include_str!("../../../deploy/compose/ports.env")
                .lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .filter_map(|l| l.split_once('='))
                .collect();
        let mut out = text.to_owned();
        let mut from = 0;
        while let Some(rel) = out[from..].find("${CONSOLE_") {
            let start = from + rel;
            let end = start + out[start..].find('}').unwrap();
            let key = &out[start + 2..end];
            if !key
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            {
                from = end; // e.g. the `${CONSOLE_*}` in header comments
                continue;
            }
            let val = *ports.get(key).unwrap_or_else(|| panic!("unset {key}"));
            out.replace_range(start..=end, val);
            from = start;
        }
        out
    }

    #[test]
    fn dev_registries_parse_and_include_dcim() {
        for (name, text, dcim_port_key) in [
            (
                "registry.main.toml",
                include_str!("../../../deploy/compose/tools/registry.main.toml"),
                "CONSOLE_TLS_PORT_TOOL_DCIM=",
            ),
            (
                "registry.run.toml",
                include_str!("../../../deploy/compose/tools/registry.run.toml"),
                "CONSOLE_TLS_PORT_TOOL_DCIM_RUN=",
            ),
        ] {
            let tools = parse(
                name,
                &render_dev_registry(text),
                &Url::parse("https://localhost:9443").unwrap(),
                true,
                &mut Vec::new(),
            )
            .unwrap_or_else(|e| panic!("{name}: {e}"));
            let d = tools.iter().find(|t| t.id == "dcim").expect(name);
            assert_eq!(d.required_roles, ["dcim:viewer"]);
            assert_eq!(d.forward_roles, ["dcim:"]);
            assert_eq!(d.scopes, ["site"]);
            assert_eq!(d.forward_headers, ["x-dcim-csrf"]);
            let port = include_str!("../../../deploy/compose/ports.env")
                .lines()
                .find_map(|l| l.strip_prefix(dcim_port_key))
                .unwrap();
            assert_eq!(d.host, format!("localhost:{port}"), "{name}: own origin");
            // hello is still present and keeps its own origin.
            assert!(tools.iter().any(|t| t.id == "hello" && t.host != d.host));
            assert_eq!(
                d.forwarded_roles(&["dcim:operator".into(), "hello:user".into()]),
                ["dcim:operator"]
            );
        }
    }

    #[test]
    fn parses_good_registry_with_defaults() {
        let tools = try_parse(GOOD, false).unwrap();
        assert_eq!(tools.len(), 2);
        let h = &tools[0];
        assert_eq!(h.host, "hello.tools.ops.example.mil");
        assert_eq!(h.entry, "/");
        assert_eq!(h.max_body_bytes, DEFAULT_MAX_BODY);
        assert_eq!(h.response_header_timeout, Duration::from_secs(30));
        assert_eq!(h.max_concurrent, 256);
        assert!(h.audit_writes);
        assert_eq!(h.upstream_origin(), "http://127.0.0.1:8090");
        let roles = vec![
            "hello:user".to_owned(),
            "hello:admin".into(),
            "dcim:x".into(),
        ];
        assert!(h.permits(&roles));
        assert!(!h.permits(&["console-user".to_owned()]));
        assert_eq!(h.forwarded_roles(&roles), vec!["hello:user", "hello:admin"]);
        assert!(h.uses_role("hello:admin") && !h.uses_role("dcim:x"));
        let e = &tools[1];
        assert_eq!(e.host, "localhost:9448");
        assert!(e.permits(&[]), "no required roles: any console user");
        assert!(e.forward_roles.is_empty());
        let s = h.summary();
        assert_eq!(s.origin, "https://hello.tools.ops.example.mil");
        assert_eq!(s.nav.len(), 1);
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("8090"), "no upstream in the browser view");
    }

    #[test]
    fn forward_roles_default_to_required_roles() {
        let t = GOOD.replace("forward_roles = [\"hello:\"]\n", "");
        let tools = try_parse(&t, false).unwrap();
        assert_eq!(tools[0].forward_roles, vec!["hello:user"]);
    }

    #[test]
    fn empty_registry_warns() {
        let mut w = Vec::new();
        assert!(
            parse("r.toml", "", &console(), false, &mut w)
                .unwrap()
                .is_empty()
        );
        assert!(w.iter().any(|w| w.contains("no tools")));
    }

    /// Each rule fails closed; the error names file, tool and field.
    #[test]
    fn validation_table() {
        let cases: &[(&str, &str, &str)] = &[
            ("id = \"hello\"", "id = \"Hello\"", "id"),
            ("id = \"evil\"", "id = \"hello\"", "id"),
            ("title = \"Hello Tool\"", "title = \"\"", "title"),
            ("icon = \"puzzle\"", "icon = \"a\\u0007b\"", "icon"),
            (
                "origin = \"https://hello.tools.ops.example.mil\"",
                "origin = \"http://hello.tools.ops.example.mil\"",
                "origin",
            ),
            (
                "origin = \"https://hello.tools.ops.example.mil\"",
                "origin = \"https://hello.tools.ops.example.mil/x\"",
                "origin",
            ),
            (
                "origin = \"https://hello.tools.ops.example.mil\"",
                "origin = \"https://console.ops.example.mil\"",
                "origin",
            ),
            (
                "origin = \"https://hello.tools.ops.example.mil\"",
                "origin = \"https://hello.console.ops.example.mil\"",
                "origin",
            ),
            (
                "origin = \"https://hello.tools.ops.example.mil\"",
                "origin = \"https://localhost:9448\"",
                "origin",
            ),
            (
                "upstream = \"http://127.0.0.1:8090\"",
                "upstream = \"http://hello.internal:8090\"",
                "upstream",
            ),
            (
                "upstream = \"http://127.0.0.1:8090\"",
                "upstream = \"http://u:p@127.0.0.1:8090\"",
                "upstream",
            ),
            (
                "upstream = \"http://127.0.0.1:8090\"",
                "upstream = \"http://127.0.0.1:8090/base\"",
                "upstream",
            ),
            (
                "upstream = \"http://127.0.0.1:8090\"",
                "upstream = \"http://127.0.0.1:8090/?q=1\"",
                "upstream",
            ),
            (
                "upstream = \"http://127.0.0.1:8090\"",
                "upstream = \"ftp://127.0.0.1\"",
                "upstream",
            ),
            (
                "required_roles = [\"hello:user\"]",
                "required_roles = [\"a b\"]",
                "required_roles",
            ),
            (
                "forward_roles = [\"hello:\"]",
                "forward_roles = [\"\"]",
                "forward_roles",
            ),
            (
                "scopes = [\"site\", \"tenant\"]",
                "scopes = [\"site\", \"site\"]",
                "scopes",
            ),
            (
                "scopes = [\"site\", \"tenant\"]",
                "scopes = [\"role\"]",
                "scopes",
            ),
            ("path = \"/\"", "path = \"/../x\"", "nav"),
            ("path = \"/\"", "path = \"//evil\"", "nav"),
            (
                "forward_headers = [\"x-hello-csrf\"]",
                "forward_headers = [\"x-console-user\"]",
                "forward_headers",
            ),
            (
                "forward_headers = [\"x-hello-csrf\"]",
                "forward_headers = [\"cookie\"]",
                "forward_headers",
            ),
            (
                "forward_headers = [\"x-hello-csrf\"]",
                "forward_headers = [\"X-Hello\"]",
                "forward_headers",
            ),
            (
                "forward_headers = [\"x-hello-csrf\"]",
                "forward_headers = [\"x-forwarded-user\"]",
                "forward_headers",
            ),
            (
                "audit_writes = true",
                "audit_writes = true\nmax_body_bytes = 104857601",
                "max_body_bytes",
            ),
            (
                "audit_writes = true",
                "audit_writes = true\nresponse_header_timeout_secs = 0",
                "response_header_timeout_secs",
            ),
            (
                "audit_writes = true",
                "audit_writes = true\nmax_concurrent = 0",
                "max_concurrent",
            ),
            (
                "audit_writes = true",
                "audit_writes = true\nentry = \"x\"",
                "entry",
            ),
        ];
        for (from, to, field) in cases {
            assert!(GOOD.contains(from), "fixture has {from}");
            let text = GOOD.replacen(from, to, 1);
            match try_parse(&text, false) {
                Err(RegistryError::Invalid { file, field: f, .. }) => {
                    assert_eq!(file, "reg.toml");
                    assert_eq!(f, *field, "{to}");
                }
                other => panic!("{to}: expected Invalid({field}), got {other:?}"),
            }
        }
    }

    #[test]
    fn unknown_fields_and_bad_toml_fail() {
        let t = GOOD.replacen(
            "audit_writes = true",
            "audit_writes = true\nupstream_url = \"x\"",
            1,
        );
        assert!(matches!(
            try_parse(&t, false),
            Err(RegistryError::Parse { .. })
        ));
        assert!(try_parse(&format!("schema = 1\n{GOOD}"), false).is_ok());
        assert!(matches!(
            try_parse(&format!("schema = 2\n{GOOD}"), false),
            Err(RegistryError::Parse { .. })
        ));
        assert!(matches!(
            try_parse("[[tool]\n", false),
            Err(RegistryError::Parse { .. })
        ));
        assert!(matches!(
            try_parse("[[tool]]\nid = \"x\"\n", false),
            Err(RegistryError::Parse { .. })
        ));
    }

    #[test]
    fn too_many_tools() {
        let mut t = String::new();
        for i in 0..=MAX_TOOLS {
            t.push_str(&format!(
                "[[tool]]\nid = \"t{i}\"\ntitle = \"T\"\norigin = \"https://t{i}.tools.example.mil\"\nupstream = \"http://127.0.0.1:1\"\n"
            ));
        }
        assert!(matches!(
            try_parse(&t, false),
            Err(RegistryError::TooMany { .. })
        ));
    }

    #[test]
    fn plain_http_upstream_needs_explicit_opt_in() {
        let t = GOOD.replacen("http://127.0.0.1:8090", "http://hello.internal:8090", 1);
        assert!(try_parse(&t, false).is_err());
        let mut w = Vec::new();
        assert!(parse("r", &t, &console(), true, &mut w).is_ok());
        assert!(w.iter().any(|w| w.contains("CONSOLE_DEV_MODE")));
        let opt_in = t.replacen(
            "audit_writes = true",
            "audit_writes = true\ninsecure_http_upstream = true",
            1,
        );
        assert!(
            try_parse(&opt_in, false).is_err(),
            "a reason is required (NIST-003)"
        );
        let with_reason = opt_in.replacen(
            "insecure_http_upstream = true",
            "insecure_http_upstream = true\ninsecure_http_upstream_reason = \"mesh mTLS (istio STRICT)\"",
            1,
        );
        let mut w = Vec::new();
        let tools = parse("r", &with_reason, &console(), false, &mut w).unwrap();
        assert!(w.iter().any(|w| w.contains("insecure_http_upstream")));
        assert_eq!(
            tools[0].insecure_upstream_reason.as_deref(),
            Some("mesh mTLS (istio STRICT)")
        );
        let empty = opt_in.replacen(
            "insecure_http_upstream = true",
            "insecure_http_upstream = true\ninsecure_http_upstream_reason = \" \"",
            1,
        );
        assert!(try_parse(&empty, false).is_err());
        let stray = GOOD.replacen(
            "audit_writes = true",
            "audit_writes = true\ninsecure_http_upstream_reason = \"x\"",
            1,
        );
        assert!(try_parse(&stray, false).is_err(), "reason without opt-in");
    }

    /// SEC-005/NIST-004: tools outside dev mode need a dedicated sid key.
    #[test]
    fn sid_key_required_outside_dev_mode() {
        let files = |p: &str| match p {
            "/reg.toml" => Some(GOOD.as_bytes().to_vec()),
            "/key.pem" => Some(crate::assertion::tests::TEST_KEY.as_bytes().to_vec()),
            "/sid.key" => Some(b"k".repeat(32)),
            "/oidc" => Some(b"s3cr3t".to_vec()),
            _ => None,
        };
        let cfg = |extra: &[(&str, &str)]| {
            let mut env: HashMap<&str, &str> = HashMap::from([
                ("CONSOLE_PUBLIC_URL", "https://console.ops.example.mil"),
                ("CONSOLE_OIDC_ISSUER", "https://kc.example.mil/realms/icam"),
                ("CONSOLE_OIDC_CLIENT_ID", "usg-ops-console"),
                ("CONSOLE_OIDC_CLIENT_SECRET_FILE", "/oidc"),
                ("CONSOLE_DATABASE_URL", "postgres://c:pw@localhost/console"),
                (
                    "CONSOLE_AUDIT_DATABASE_URL",
                    "postgres://a:pw@localhost/console",
                ),
                ("CONSOLE_REQUIRED_ACR", "urn:test:cac"),
                ("CONSOLE_TOOLS_REGISTRY_FILE", "/reg.toml"),
                ("CONSOLE_ASSERTION_KEY_FILE", "/key.pem"),
            ]);
            env.extend(extra.iter().copied());
            Config::from_lookup(|k| env.get(k).map(|v| (*v).to_owned()), files).unwrap()
        };
        let mut w = Vec::new();
        let err = ToolsState::from_config(&cfg(&[]), &mut w).unwrap_err();
        assert!(err.contains("CONSOLE_TOOL_SESSION_HMAC_KEY_FILE"), "{err}");
        assert!(
            ToolsState::from_config(
                &cfg(&[("CONSOLE_TOOL_SESSION_HMAC_KEY_FILE", "/sid.key")]),
                &mut w
            )
            .is_ok()
        );
        let mut w = Vec::new();
        assert!(ToolsState::from_config(&cfg(&[("CONSOLE_DEV_MODE", "true")]), &mut w).is_ok());
        assert!(w.iter().any(|w| w.contains("derived")));
    }

    #[test]
    fn host_keys() {
        assert_eq!(host_key("Hello.Tools.X:443"), "hello.tools.x");
        assert_eq!(host_key("localhost:9447"), "localhost:9447");
    }
}
