//! Environment configuration. Fails closed: any missing or invalid required
//! key is a start-up error with a message naming the key (never its value
//! when the key is a secret).

use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

use ipnet::IpNet;
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use url::Url;

/// A string that must never be logged or echoed.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Binary key material that must never be logged or echoed.
#[derive(Clone)]
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    pub fn new(value: Vec<u8>) -> Self {
        Self(value)
    }
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretBytes(<redacted>)")
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("{0} is required")]
    Missing(&'static str),
    #[error("{key} is invalid: {why}")]
    Invalid { key: &'static str, why: String },
    #[error("{0}: could not read file")]
    File(&'static str),
}

fn invalid(key: &'static str, why: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        key,
        why: why.into(),
    }
}

// COMPLIANCE MAPPING:
// - NIST 800-53: IA-5 (Authenticator Management) — client secret / DB URLs from files, redacted in Debug
// - NIST 800-53: SI-10 (Information Input Validation) — every key validated, fail closed
#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub dist: String,
    /// Origin-only URL (`scheme://host[:port]`), no trailing slash.
    ///
    /// Deployment (SEC-010): put this origin and every registry tool origin
    /// under a dedicated registrable domain (e.g. `*.console.example.mil`
    /// on its own eTLD+1 / isolation domain) that hosts nothing else, so
    /// `Sec-Fetch-Site: same-site` on a launch means "console or a tool".
    pub public_url: Url,
    pub oidc_issuer: String,
    pub oidc_client_id: String,
    pub oidc_client_secret: Secret,
    pub oidc_redirect_path: String,
    pub oidc_discovery_retries: u32,
    pub database_url: Secret,
    pub db_max_connections: u32,
    pub audit_database_url: Secret,
    pub audit_db_max_connections: u32,
    pub session_idle: Duration,
    pub session_absolute: Duration,
    pub sweep_interval: Duration,
    pub trusted_proxies: Vec<IpNet>,
    pub required_role: String,
    /// Accepted id_token `acr` values (sent as `acr_values`). Empty only in
    /// dev mode, where the check is skipped.
    pub required_acr: Vec<String>,
    /// `CONSOLE_DEV_MODE=true`: relaxes the ACR and DB-TLS requirements.
    pub dev_mode: bool,
    /// Optional; when set, `serve` runs migrations with it at start-up.
    pub migrator_database_url: Option<Secret>,
    /// Total timeout of the keepalive refresh grant (PERF-008).
    pub oidc_refresh_timeout: Duration,
    /// `CONSOLE_TOOLS_REGISTRY_FILE`: (path, contents). `None` = no tools.
    pub tools_registry: Option<(String, String)>,
    /// `CONSOLE_ASSERTION_KEY_FILE` contents (PKCS#8 PEM, P-256).
    pub assertion_key: Option<Secret>,
    /// `CONSOLE_ASSERTION_PUBLISH_KEYS_FILE` contents (public PEM bundle).
    pub assertion_publish_keys: Option<String>,
    /// `CONSOLE_TOOL_SESSION_HMAC_KEY_FILE` key bytes (assertion `sid` key):
    /// base64 text is decoded, anything else is used as raw bytes. See
    /// [`decode_hmac_key`].
    pub tool_sid_key: Option<SecretBytes>,
    /// Non-fatal findings to log once the subscriber is up (never values).
    pub warnings: Vec<String>,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|k| std::env::var(k).ok(), |p| std::fs::read(p).ok())
    }

    /// `env` looks up a variable; `read_bytes` reads a `*_FILE` path. Text
    /// files must be UTF-8; the sid HMAC key file may be binary (F4).
    pub fn from_lookup(
        env: impl Fn(&str) -> Option<String>,
        read_bytes: impl Fn(&str) -> Option<Vec<u8>>,
    ) -> Result<Self, ConfigError> {
        let read_file = |p: &str| read_bytes(p).and_then(|b| String::from_utf8(b).ok());
        let get = |k: &str| env(k).filter(|v| !v.trim().is_empty());
        let mut warnings = Vec::new();
        let dev_mode = match get("CONSOLE_DEV_MODE").as_deref().map(str::trim) {
            None | Some("false" | "0") => false,
            Some("true" | "1") => true,
            Some(_) => return Err(invalid("CONSOLE_DEV_MODE", "expected true or false")),
        };
        if dev_mode {
            warnings.push(
                "CONSOLE_DEV_MODE=true: ACR and database TLS requirements are relaxed; never use in production"
                    .into(),
            );
        }

        let listen = get("CONSOLE_LISTEN")
            .unwrap_or_else(|| "0.0.0.0:8080".into())
            .parse()
            .map_err(|_| invalid("CONSOLE_LISTEN", "expected host:port"))?;
        let dist = get("CONSOLE_DIST").unwrap_or_else(|| "crates/console-shell/dist".into());

        let public_url = parse_origin(
            "CONSOLE_PUBLIC_URL",
            &get("CONSOLE_PUBLIC_URL").ok_or(ConfigError::Missing("CONSOLE_PUBLIC_URL"))?,
        )?;
        let issuer =
            get("CONSOLE_OIDC_ISSUER").ok_or(ConfigError::Missing("CONSOLE_OIDC_ISSUER"))?;
        let issuer_url =
            Url::parse(&issuer).map_err(|_| invalid("CONSOLE_OIDC_ISSUER", "not a URL"))?;
        require_https("CONSOLE_OIDC_ISSUER", &issuer_url)?;
        let oidc_client_id =
            get("CONSOLE_OIDC_CLIENT_ID").ok_or(ConfigError::Missing("CONSOLE_OIDC_CLIENT_ID"))?;

        let oidc_client_secret = match (
            get("CONSOLE_OIDC_CLIENT_SECRET_FILE"),
            get("CONSOLE_OIDC_CLIENT_SECRET"),
        ) {
            (Some(path), _) => read_secret(&read_file, "CONSOLE_OIDC_CLIENT_SECRET_FILE", &path)?,
            (None, Some(v)) => {
                warnings.push(
                    "CONSOLE_OIDC_CLIENT_SECRET set directly; use CONSOLE_OIDC_CLIENT_SECRET_FILE outside dev".into(),
                );
                Secret::new(v)
            }
            (None, None) => return Err(ConfigError::Missing("CONSOLE_OIDC_CLIENT_SECRET_FILE")),
        };

        let oidc_redirect_path =
            get("CONSOLE_OIDC_REDIRECT_PATH").unwrap_or_else(|| "/callback".into());
        if !oidc_redirect_path.starts_with('/') || oidc_redirect_path.starts_with("//") {
            return Err(invalid(
                "CONSOLE_OIDC_REDIRECT_PATH",
                "must be an absolute path",
            ));
        }
        let oidc_discovery_retries = num(&get, "CONSOLE_OIDC_DISCOVERY_RETRIES", 10, 0, 1000)?;

        let database_url = db_url(
            &get,
            &read_file,
            "CONSOLE_DATABASE_URL",
            "CONSOLE_DATABASE_URL_FILE",
            dev_mode,
            &mut warnings,
        )?
        .ok_or(ConfigError::Missing("CONSOLE_DATABASE_URL"))?;
        let audit_database_url = db_url(
            &get,
            &read_file,
            "CONSOLE_AUDIT_DATABASE_URL",
            "CONSOLE_AUDIT_DATABASE_URL_FILE",
            dev_mode,
            &mut warnings,
        )?
        .ok_or(ConfigError::Missing("CONSOLE_AUDIT_DATABASE_URL"))?;
        let migrator_database_url = db_url(
            &get,
            &read_file,
            "CONSOLE_MIGRATOR_DATABASE_URL",
            "CONSOLE_MIGRATOR_DATABASE_URL_FILE",
            dev_mode,
            &mut warnings,
        )?;
        if migrator_database_url.is_some() {
            warnings.push(
                "CONSOLE_MIGRATOR_DATABASE_URL set: serve runs migrations; outside dev run `console-gateway migrate` as a separate job"
                    .into(),
            );
        }
        // F3: default 32 (was 10). Every proxied tool request holds one app
        // connection for a single indexed lookup (no session cache, so logout
        // applies on every replica), so pool size caps proxy concurrency.
        // Budget per replica is 32 + audit 4 = 36; stock Postgres and CNPG
        // default `max_connections = 100` (3 reserved), which fits two
        // replicas plus migrator/sweeper headroom. For more replicas raise
        // `max_connections` or front Postgres with a pooler (CNPG Pooler /
        // PgBouncer, transaction mode) and size this per replica.
        let db_max_connections = num(
            &get,
            "CONSOLE_DB_MAX_CONNECTIONS",
            DEFAULT_DB_MAX_CONNECTIONS,
            1,
            200,
        )?;
        let audit_db_max_connections = num(&get, "CONSOLE_AUDIT_DB_MAX_CONNECTIONS", 4, 1, 100)?;

        let idle = num(&get, "CONSOLE_SESSION_IDLE_SECS", 900, 60, 3600)?;
        let absolute = num(&get, "CONSOLE_SESSION_ABSOLUTE_SECS", 43_200, 900, 86_400)?;
        if absolute <= idle {
            return Err(invalid(
                "CONSOLE_SESSION_ABSOLUTE_SECS",
                "must be greater than CONSOLE_SESSION_IDLE_SECS",
            ));
        }
        let sweep = num(&get, "CONSOLE_SWEEP_INTERVAL_SECS", 60, 5, 3600)?;

        let trusted_proxies = match get("CONSOLE_TRUSTED_PROXIES") {
            None => Vec::new(),
            Some(list) => list
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    s.parse::<IpNet>()
                        .map_err(|_| invalid("CONSOLE_TRUSTED_PROXIES", format!("bad CIDR {s:?}")))
                })
                .collect::<Result<_, _>>()?,
        };
        let required_role = get("CONSOLE_REQUIRED_ROLE").unwrap_or_else(|| "console-user".into());
        let required_acr: Vec<String> = get("CONSOLE_REQUIRED_ACR")
            .map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        if required_acr
            .iter()
            .any(|a| a.len() > 256 || a.chars().any(|c| c.is_whitespace() || c.is_control()))
        {
            return Err(invalid(
                "CONSOLE_REQUIRED_ACR",
                "values must be non-blank, at most 256 bytes, without spaces",
            ));
        }
        if required_acr.is_empty() {
            if !dev_mode {
                // NIST 800-53: IA-2(12) — the console refuses to run without an assurance requirement.
                return Err(ConfigError::Missing("CONSOLE_REQUIRED_ACR"));
            }
            warnings
                .push("CONSOLE_REQUIRED_ACR unset (dev mode): id_token acr is not checked".into());
        }

        let oidc_refresh_timeout = num(&get, "CONSOLE_OIDC_REFRESH_TIMEOUT_SECS", 4, 1, 10)?;

        // M4 tools. The registry path is not secret; its absence means "no
        // tools", but a configured file that cannot be read fails closed.
        let tools_registry = match get("CONSOLE_TOOLS_REGISTRY_FILE") {
            None => {
                warnings.push("CONSOLE_TOOLS_REGISTRY_FILE unset: no tools are registered".into());
                None
            }
            Some(path) => {
                let text =
                    read_file(&path).ok_or(ConfigError::File("CONSOLE_TOOLS_REGISTRY_FILE"))?;
                Some((path, text))
            }
        };
        let assertion_key = get("CONSOLE_ASSERTION_KEY_FILE")
            .map(|p| read_secret(&read_file, "CONSOLE_ASSERTION_KEY_FILE", &p))
            .transpose()?;
        let assertion_publish_keys = get("CONSOLE_ASSERTION_PUBLISH_KEYS_FILE")
            .map(|p| read_file(&p).ok_or(ConfigError::File("CONSOLE_ASSERTION_PUBLISH_KEYS_FILE")))
            .transpose()?;
        let tool_sid_key = get("CONSOLE_TOOL_SESSION_HMAC_KEY_FILE")
            .map(|p| {
                let raw = read_bytes(&p)
                    .ok_or(ConfigError::File("CONSOLE_TOOL_SESSION_HMAC_KEY_FILE"))?;
                decode_hmac_key(raw).ok_or_else(|| {
                    invalid(
                        "CONSOLE_TOOL_SESSION_HMAC_KEY_FILE",
                        format!("key must be at least {MIN_HMAC_KEY_BYTES} bytes"),
                    )
                })
            })
            .transpose()?;

        Ok(Self {
            listen,
            dist,
            public_url,
            oidc_issuer: issuer,
            oidc_client_id,
            oidc_client_secret,
            oidc_redirect_path,
            oidc_discovery_retries,
            database_url,
            db_max_connections,
            audit_database_url,
            audit_db_max_connections,
            session_idle: Duration::from_secs(idle.into()),
            session_absolute: Duration::from_secs(absolute.into()),
            sweep_interval: Duration::from_secs(sweep.into()),
            trusted_proxies,
            required_role,
            required_acr,
            dev_mode,
            migrator_database_url,
            oidc_refresh_timeout: Duration::from_secs(oidc_refresh_timeout.into()),
            tools_registry,
            assertion_key,
            assertion_publish_keys,
            tool_sid_key,
            warnings,
        })
    }

    /// `scheme://host[:port]` without a trailing slash.
    pub fn public_origin(&self) -> String {
        self.public_url.origin().ascii_serialization()
    }

    pub fn redirect_url(&self) -> String {
        format!("{}{}", self.public_origin(), self.oidc_redirect_path)
    }

    pub fn post_logout_url(&self) -> String {
        format!("{}/logged-out", self.public_origin())
    }

    /// `scheme://host[:port]` of the issuer (for CSP `form-action`).
    pub fn issuer_origin(&self) -> Option<String> {
        Url::parse(&self.oidc_issuer)
            .ok()
            .map(|u| u.origin().ascii_serialization())
            .filter(|o| o != "null")
    }
}

/// `console-gateway migrate`: only the migrator URL is needed.
pub fn migrator_url_from_env() -> Result<Secret, ConfigError> {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let dev_mode = matches!(get("CONSOLE_DEV_MODE").as_deref(), Some("true" | "1"));
    let mut warnings = Vec::new();
    let url = db_url(
        &get,
        &|p: &str| std::fs::read_to_string(p).ok(),
        "CONSOLE_MIGRATOR_DATABASE_URL",
        "CONSOLE_MIGRATOR_DATABASE_URL_FILE",
        dev_mode,
        &mut warnings,
    )?
    .ok_or(ConfigError::Missing("CONSOLE_MIGRATOR_DATABASE_URL"))?;
    for w in warnings {
        tracing::warn!(warning = %w, "configuration");
    }
    Ok(url)
}

pub(crate) fn is_loopback_host(url: &Url) -> bool {
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

fn require_https(key: &'static str, url: &Url) -> Result<(), ConfigError> {
    match url.scheme() {
        "https" => Ok(()),
        "http" if is_loopback_host(url) => Ok(()),
        _ => Err(invalid(key, "must be https (http only for localhost)")),
    }
}

fn parse_origin(key: &'static str, raw: &str) -> Result<Url, ConfigError> {
    let url = Url::parse(raw).map_err(|_| invalid(key, "not a URL"))?;
    require_https(key, &url)?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(invalid(
            key,
            "must be an origin like https://console.example.mil",
        ));
    }
    Ok(url)
}

fn num<G: Fn(&str) -> Option<String>>(
    get: &G,
    key: &'static str,
    default: u32,
    min: u32,
    max: u32,
) -> Result<u32, ConfigError> {
    let v = match get(key) {
        None => default,
        Some(s) => s
            .trim()
            .parse()
            .map_err(|_| invalid(key, "not an integer"))?,
    };
    if (min..=max).contains(&v) {
        Ok(v)
    } else {
        Err(invalid(key, format!("must be in {min}..={max}")))
    }
}

/// Default `CONSOLE_DB_MAX_CONNECTIONS` (app pool per replica; F3).
pub const DEFAULT_DB_MAX_CONNECTIONS: u32 = 32;

/// Minimum HMAC key length (256 bits, matching HMAC-SHA-256 output).
pub const MIN_HMAC_KEY_BYTES: usize = 32;

/// Decodes HMAC key material read from a file (F4). In order:
/// 1. UTF-8 text whose trimmed form is standard base64 (padded or not) and
///    decodes to at least [`MIN_HMAC_KEY_BYTES`] is decoded, e.g.
///    `openssl rand -base64 48`;
/// 2. other UTF-8 text is trimmed and used as its bytes (pre-M4 behaviour;
///    this also keeps a 32-char alphanumeric key, which is valid base64 of
///    only 24 bytes, working);
/// 3. non-UTF-8 content is raw binary, used as-is (`head -c 32 /dev/urandom`).
///
/// `None` when the resulting key is shorter than [`MIN_HMAC_KEY_BYTES`]
/// (fail closed).
// COMPLIANCE MAPPING:
// - NIST 800-53: SC-12 (Cryptographic Key Establishment and Management) — ≥256-bit sid HMAC key, binary or base64
pub fn decode_hmac_key(raw: Vec<u8>) -> Option<SecretBytes> {
    use base64::Engine;
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
    let key = match std::str::from_utf8(&raw) {
        Ok(text) => {
            let t = text.trim();
            STANDARD
                .decode(t)
                .or_else(|_| STANDARD_NO_PAD.decode(t))
                .ok()
                .filter(|k| k.len() >= MIN_HMAC_KEY_BYTES)
                .unwrap_or_else(|| t.as_bytes().to_vec())
        }
        Err(_) => raw,
    };
    (key.len() >= MIN_HMAC_KEY_BYTES).then(|| SecretBytes::new(key))
}

fn read_secret(
    read_file: &impl Fn(&str) -> Option<String>,
    key: &'static str,
    path: &str,
) -> Result<Secret, ConfigError> {
    let v = read_file(path).ok_or(ConfigError::File(key))?;
    let v = v.trim();
    if v.is_empty() {
        return Err(invalid(key, "file is empty"));
    }
    Ok(Secret::new(v.to_owned()))
}

/// `None` when neither key is set. Non-loopback hosts must verify the
/// server certificate unless `dev_mode`.
// COMPLIANCE MAPPING:
// - NIST 800-53: SC-8 (Transmission Confidentiality and Integrity) — DB TLS with certificate verification off-host
fn db_url<G: Fn(&str) -> Option<String>>(
    get: &G,
    read_file: &impl Fn(&str) -> Option<String>,
    key: &'static str,
    file_key: &'static str,
    dev_mode: bool,
    warnings: &mut Vec<String>,
) -> Result<Option<Secret>, ConfigError> {
    let url = match (get(file_key), get(key)) {
        (Some(path), _) => read_secret(read_file, file_key, &path)?,
        (None, Some(v)) => {
            warnings.push(format!(
                "{key} set directly (it may carry a password); use {file_key} outside dev"
            ));
            Secret::new(v)
        }
        (None, None) => return Ok(None),
    };
    if !(url.expose().starts_with("postgres://") || url.expose().starts_with("postgresql://")) {
        return Err(invalid(key, "must be a postgres:// URL"));
    }
    let opts: PgConnectOptions = url
        .expose()
        .parse()
        .map_err(|_| invalid(key, "not a valid postgres URL"))?;
    let local = opts.get_socket().is_some()
        || opts.get_host().starts_with('/')
        || matches!(opts.get_host(), "localhost" | "127.0.0.1" | "::1" | "[::1]");
    let verified = matches!(
        opts.get_ssl_mode(),
        PgSslMode::VerifyFull | PgSslMode::VerifyCa
    );
    if !local && !verified {
        if !dev_mode {
            return Err(invalid(
                key,
                "non-loopback database host requires sslmode=verify-full (or verify-ca)",
            ));
        }
        warnings.push(format!(
            "{key}: non-loopback host without sslmode=verify-full (allowed by CONSOLE_DEV_MODE)"
        ));
    }
    Ok(Some(url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn base() -> HashMap<&'static str, &'static str> {
        HashMap::from([
            ("CONSOLE_PUBLIC_URL", "http://localhost:8080"),
            ("CONSOLE_OIDC_ISSUER", "http://localhost:8180/realms/icam"),
            ("CONSOLE_OIDC_CLIENT_ID", "usg-ops-console"),
            ("CONSOLE_OIDC_CLIENT_SECRET_FILE", "/run/secrets/oidc"),
            (
                "CONSOLE_DATABASE_URL",
                "postgres://console:pw@localhost/console",
            ),
            (
                "CONSOLE_AUDIT_DATABASE_URL",
                "postgres://console_audit:pw@localhost/console",
            ),
            ("CONSOLE_REQUIRED_ACR", "urn:test:cac"),
        ])
    }

    fn load(env: &HashMap<&'static str, &'static str>) -> Result<Config, ConfigError> {
        Config::from_lookup(
            |k| env.get(k).map(|v| (*v).to_owned()),
            |p| (p == "/run/secrets/oidc").then(|| b"s3cr3t-value\n".to_vec()),
        )
    }

    #[test]
    fn defaults_and_secret_redaction() {
        let cfg = load(&base()).unwrap();
        assert_eq!(cfg.session_idle, Duration::from_secs(900));
        assert_eq!(cfg.session_absolute, Duration::from_secs(43_200));
        assert_eq!(cfg.db_max_connections, 32, "F3 default app pool");
        assert_eq!(cfg.required_role, "console-user");
        assert_eq!(cfg.oidc_client_secret.expose(), "s3cr3t-value");
        assert_eq!(cfg.redirect_url(), "http://localhost:8080/callback");
        assert_eq!(cfg.post_logout_url(), "http://localhost:8080/logged-out");
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("s3cr3t"));
        assert!(!dbg.contains(":pw@"));
    }

    #[test]
    fn missing_required_keys_fail_closed() {
        for key in [
            "CONSOLE_PUBLIC_URL",
            "CONSOLE_OIDC_ISSUER",
            "CONSOLE_OIDC_CLIENT_ID",
            "CONSOLE_DATABASE_URL",
            "CONSOLE_AUDIT_DATABASE_URL",
        ] {
            let mut env = base();
            env.remove(key);
            assert_eq!(load(&env).unwrap_err(), ConfigError::Missing(key), "{key}");
        }
        let mut env = base();
        env.remove("CONSOLE_OIDC_CLIENT_SECRET_FILE");
        assert!(matches!(load(&env), Err(ConfigError::Missing(_))));
        env.insert("CONSOLE_OIDC_CLIENT_SECRET_FILE", "/nope");
        assert_eq!(
            load(&env).unwrap_err(),
            ConfigError::File("CONSOLE_OIDC_CLIENT_SECRET_FILE")
        );
    }

    #[test]
    fn direct_secret_warns() {
        let mut env = base();
        env.remove("CONSOLE_OIDC_CLIENT_SECRET_FILE");
        env.insert("CONSOLE_OIDC_CLIENT_SECRET", "dev");
        let cfg = load(&env).unwrap();
        assert!(cfg.warnings.iter().any(|w| w.contains("CLIENT_SECRET")));
        assert!(!cfg.warnings.iter().any(|w| w.contains("dev\"")));
    }

    #[test]
    fn invalid_values_rejected() {
        let cases: &[(&'static str, &'static str)] = &[
            ("CONSOLE_PUBLIC_URL", "http://console.example.mil"),
            ("CONSOLE_PUBLIC_URL", "https://console.example.mil/sub"),
            ("CONSOLE_PUBLIC_URL", "https://u:p@console.example.mil"),
            ("CONSOLE_OIDC_ISSUER", "http://kc.example.mil/realms/icam"),
            ("CONSOLE_DATABASE_URL", "mysql://x"),
            ("CONSOLE_SESSION_IDLE_SECS", "30"),
            ("CONSOLE_SESSION_IDLE_SECS", "abc"),
            ("CONSOLE_SESSION_ABSOLUTE_SECS", "100000"),
            ("CONSOLE_TRUSTED_PROXIES", "10.0.0.0/8,nope"),
            ("CONSOLE_OIDC_REDIRECT_PATH", "//evil/cb"),
            ("CONSOLE_OIDC_REFRESH_TIMEOUT_SECS", "0"),
            ("CONSOLE_OIDC_REFRESH_TIMEOUT_SECS", "11"),
        ];
        for (k, v) in cases {
            let mut env = base();
            env.insert(k, v);
            assert!(
                matches!(load(&env), Err(ConfigError::Invalid { .. })),
                "{k}={v}"
            );
        }
        let mut env = base();
        env.insert("CONSOLE_SESSION_IDLE_SECS", "1800");
        env.insert("CONSOLE_SESSION_ABSOLUTE_SECS", "1200");
        assert!(load(&env).is_err(), "absolute must exceed idle");
    }

    #[test]
    fn acr_required_outside_dev_mode() {
        let mut env = base();
        assert_eq!(load(&env).unwrap().required_acr, ["urn:test:cac"]);
        env.insert("CONSOLE_REQUIRED_ACR", "a, b");
        assert_eq!(load(&env).unwrap().required_acr, ["a", "b"]);
        env.remove("CONSOLE_REQUIRED_ACR");
        assert_eq!(
            load(&env).unwrap_err(),
            ConfigError::Missing("CONSOLE_REQUIRED_ACR")
        );
        env.insert("CONSOLE_DEV_MODE", "true");
        let cfg = load(&env).unwrap();
        assert!(cfg.dev_mode && cfg.required_acr.is_empty());
        assert!(cfg.warnings.iter().any(|w| w.contains("CONSOLE_DEV_MODE")));
        env.insert("CONSOLE_DEV_MODE", "yes");
        assert!(matches!(load(&env), Err(ConfigError::Invalid { .. })));
    }

    #[test]
    fn remote_db_requires_verified_tls() {
        for (url, ok) in [
            ("postgres://c:pw@db.example.mil/console", false),
            (
                "postgres://c:pw@db.example.mil/console?sslmode=require",
                false,
            ),
            (
                "postgres://c:pw@db.example.mil/console?sslmode=verify-full",
                true,
            ),
            (
                "postgres://c:pw@db.example.mil/console?sslmode=verify-ca",
                true,
            ),
            (
                "postgres://c:pw@127.0.0.1:5432/console?sslmode=disable",
                true,
            ),
            ("postgres://c:pw@[::1]/console", true),
        ] {
            let mut env = base();
            env.insert("CONSOLE_DATABASE_URL", url);
            assert_eq!(load(&env).is_ok(), ok, "{url}");
            env.insert("CONSOLE_DEV_MODE", "true");
            let cfg = load(&env).unwrap();
            assert_eq!(
                cfg.warnings.iter().any(|w| w.contains("non-loopback")),
                !ok,
                "{url}"
            );
        }
        let mut env = base();
        env.insert(
            "CONSOLE_MIGRATOR_DATABASE_URL",
            "postgres://m:pw@db.example.mil/console",
        );
        assert!(load(&env).is_err(), "migrator URL is checked too");
    }

    #[test]
    fn direct_db_url_warns() {
        let cfg = load(&base()).unwrap();
        assert!(
            cfg.warnings
                .iter()
                .any(|w| w.contains("CONSOLE_DATABASE_URL set directly"))
        );
        assert!(!cfg.warnings.iter().any(|w| w.contains(":pw@")));
        assert!(cfg.migrator_database_url.is_none());
    }

    #[test]
    fn tool_keys_and_registry_files() {
        let cfg = load(&base()).unwrap();
        assert_eq!(cfg.oidc_refresh_timeout, Duration::from_secs(4));
        assert!(cfg.tools_registry.is_none() && cfg.assertion_key.is_none());
        assert!(cfg.warnings.iter().any(|w| w.contains("no tools")));
        let files = |p: &str| match p {
            "/reg.toml" => Some(b"[[tool]]\n".to_vec()),
            "/key.pem" => Some(b"-----BEGIN PRIVATE KEY-----\n".to_vec()),
            "/short" => Some(b"too-short".to_vec()),
            "/short-bin" => Some(vec![0xff; 31]),
            "/short-b64" => Some(b"QkJCQkJCQkJC\n".to_vec()),
            "/bin" => Some((0u8..32).map(|i| i.wrapping_mul(97) | 0x80).collect()),
            "/b64" => {
                Some(b"QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFB\n".to_vec())
            }
            "/run/secrets/oidc" => Some(b"s3cr3t-value".to_vec()),
            _ => None,
        };
        let mut env = base();
        env.insert("CONSOLE_TOOLS_REGISTRY_FILE", "/reg.toml");
        env.insert("CONSOLE_ASSERTION_KEY_FILE", "/key.pem");
        let lookup = |env: &HashMap<&'static str, &'static str>| {
            Config::from_lookup(|k| env.get(k).map(|v| (*v).to_owned()), files)
        };
        let cfg = lookup(&env).unwrap();
        assert_eq!(cfg.tools_registry.as_ref().unwrap().0, "/reg.toml");
        assert!(
            !format!("{cfg:?}").contains("BEGIN PRIVATE"),
            "key redacted"
        );
        env.insert("CONSOLE_TOOLS_REGISTRY_FILE", "/missing.toml");
        assert_eq!(
            lookup(&env).unwrap_err(),
            ConfigError::File("CONSOLE_TOOLS_REGISTRY_FILE"),
            "a configured but unreadable registry fails closed"
        );
        env.insert("CONSOLE_TOOLS_REGISTRY_FILE", "/reg.toml");
        env.insert("CONSOLE_TOOL_SESSION_HMAC_KEY_FILE", "/short");
        assert!(matches!(lookup(&env), Err(ConfigError::Invalid { .. })));
        // F4: binary and base64 key files; short keys fail closed either way.
        env.insert("CONSOLE_TOOL_SESSION_HMAC_KEY_FILE", "/short-bin");
        assert!(matches!(lookup(&env), Err(ConfigError::Invalid { .. })));
        env.insert("CONSOLE_TOOL_SESSION_HMAC_KEY_FILE", "/short-b64");
        assert!(matches!(lookup(&env), Err(ConfigError::Invalid { .. })));
        env.insert("CONSOLE_TOOL_SESSION_HMAC_KEY_FILE", "/bin");
        let cfg = lookup(&env).unwrap();
        let key = cfg.tool_sid_key.as_ref().unwrap().expose();
        assert_eq!(key, files("/bin").unwrap().as_slice(), "raw bytes kept");
        assert!(
            !format!("{cfg:?}").contains(&format!("{key:?}")),
            "key redacted"
        );
        env.insert("CONSOLE_TOOL_SESSION_HMAC_KEY_FILE", "/b64");
        let cfg = lookup(&env).unwrap();
        assert_eq!(
            cfg.tool_sid_key.unwrap().expose(),
            &[b'A'; 48][..],
            "base64 decoded"
        );
    }

    #[test]
    fn https_public_url_and_proxies() {
        let mut env = base();
        env.insert("CONSOLE_PUBLIC_URL", "https://console.example.mil/");
        env.insert("CONSOLE_TRUSTED_PROXIES", "10.0.0.0/8, 192.168.1.1/32");
        let cfg = load(&env).unwrap();
        assert_eq!(cfg.public_origin(), "https://console.example.mil");
        assert_eq!(
            cfg.issuer_origin().as_deref(),
            Some("http://localhost:8180")
        );
        assert_eq!(cfg.trusted_proxies.len(), 2);
    }
}
