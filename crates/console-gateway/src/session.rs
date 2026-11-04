//! Server-side sessions: opaque ids, cookie flags, expiry math, Postgres store.

use std::net::IpAddr;
use std::time::Duration;

use axum::http::{HeaderMap, header};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use chrono::{DateTime, Utc};
use cookie::{Cookie, SameSite};
use rand::TryRngCore;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use tracing::Instrument;

pub const SID_COOKIE: &str = "__Host-console_sid";
pub const AUTH_COOKIE: &str = "__Host-console_auth";
/// Set when a session ends in this browser (logout, expiry, revocation);
/// the next `/login` then forces fresh authentication (SEC-001).
pub const REAUTH_COOKIE: &str = "__Host-console_reauth";
/// Global cap on live pending authorization requests (SC-5).
pub const MAX_PENDING_AUTH: i64 = 10_000;
/// Lifetime of a pending authorization request (and its cookie).
pub const AUTH_REQUEST_TTL: Duration = Duration::from_secs(600);
const ID_BYTES: usize = 32;

/// Injectable time source so expiry is testable at exact boundaries.
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> DateTime<Utc>;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("operating system RNG unavailable")]
pub struct RngError;

/// `N` bytes from the OS CSPRNG.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], RngError> {
    let mut buf = [0u8; N];
    OsRng.try_fill_bytes(&mut buf).map_err(|_| RngError)?;
    Ok(buf)
}

/// A fresh opaque id: (cookie value, sha256 stored in the DB).
// COMPLIANCE MAPPING:
// - NIST 800-53: SC-23 (Session Authenticity) — 256-bit random ids; DB holds only the hash
pub fn new_opaque_id() -> Result<(String, Vec<u8>), RngError> {
    let raw = random_bytes::<ID_BYTES>()?;
    Ok((B64URL.encode(raw), Sha256::digest(raw).to_vec()))
}

/// Hash of a presented cookie value; `None` unless it decodes to exactly 32
/// bytes (the server never adopts a client-chosen id format).
pub fn hash_cookie_value(value: &str) -> Option<Vec<u8>> {
    let raw = B64URL.decode(value).ok()?;
    (raw.len() == ID_BYTES).then(|| Sha256::digest(&raw).to_vec())
}

/// First 8 hex chars of the id hash: a correlation handle, not the secret.
pub fn session_ref(id_hash: &[u8]) -> String {
    id_hash.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// Value of the first cookie named `name` across all `Cookie` headers.
pub fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(Cookie::split_parse)
        .filter_map(Result::ok)
        .find(|c| c.name() == name)
        .map(|c| c.value().to_owned())
}

// COMPLIANCE MAPPING:
// - NIST 800-53: SC-23 (Session Authenticity) — __Host- prefix, Secure, HttpOnly, SameSite=Lax, no Domain
// - NIST 800-53: SC-8 (Transmission Confidentiality) — Secure cookie only
/// `Set-Cookie` value for a session-lifetime cookie (no `Max-Age`; the
/// server-side expiry is authoritative) or, with `max_age`, a bounded one.
pub fn set_cookie(name: &'static str, value: String, max_age: Option<Duration>) -> String {
    build_cookie(name, value, max_age, SameSite::Lax)
}

/// Same flags as [`set_cookie`] but `SameSite=Strict` (tool-origin cookies:
/// only ever needed on same-site requests from the console's iframe).
// COMPLIANCE MAPPING:
// - NIST 800-53: SC-23 (Session Authenticity) — __Host-tool_sid: Secure, HttpOnly, SameSite=Strict, host-only
pub fn set_cookie_strict(name: &'static str, value: String, max_age: Option<Duration>) -> String {
    build_cookie(name, value, max_age, SameSite::Strict)
}

fn build_cookie(
    name: &'static str,
    value: String,
    max_age: Option<Duration>,
    same_site: SameSite,
) -> String {
    let mut c = Cookie::build((name, value))
        .secure(true)
        .http_only(true)
        .same_site(same_site)
        .path("/");
    if let Some(age) = max_age {
        c = c.max_age(cookie::time::Duration::seconds(
            i64::try_from(age.as_secs()).unwrap_or(i64::MAX),
        ));
    }
    c.build().to_string()
}

/// The reauth marker lives as long as a session could (so it outlives any
/// IdP SSO session bounded to the console lifetimes).
pub fn reauth_cookie(lifetime: Duration) -> String {
    set_cookie(REAUTH_COOKIE, "1".into(), Some(lifetime))
}

pub fn clear_cookie(name: &'static str) -> String {
    set_cookie(name, String::new(), Some(Duration::ZERO))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Session {
    pub id_hash: Vec<u8>,
    pub user_sub: String,
    pub username: String,
    pub display_name: String,
    pub roles: Vec<String>,
    pub csrf_token: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub absolute_expires_at: DateTime<Utc>,
    /// Idle limit fixed at creation (POAM-031); `None` on pre-M4 rows.
    pub idle_timeout_secs: Option<i32>,
    /// id_token `acr` at sign-in (carried in tool assertions).
    pub acr: Option<String>,
}

impl Session {
    /// The row's idle limit, or `fallback` (this gateway's config) for
    /// legacy rows.
    pub fn idle(&self, fallback: Duration) -> Duration {
        idle_or(self.idle_timeout_secs, fallback)
    }
}

/// Row idle seconds (if positive) or `fallback`.
pub fn idle_or(secs: Option<i32>, fallback: Duration) -> Duration {
    secs.and_then(|v| u64::try_from(v).ok())
        .filter(|v| *v > 0)
        .map(Duration::from_secs)
        .unwrap_or(fallback)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expiry {
    Live,
    Idle,
    Absolute,
}

// COMPLIANCE MAPPING:
// - NIST 800-53: AC-11 (Device Lock) — idle deadline; only keepalive moves last_seen
// - NIST 800-53: AC-12 (Session Termination) — absolute deadline wins when both pass
/// Deadlines are exclusive: at exactly the deadline the session is expired.
pub fn expiry(
    last_seen: DateTime<Utc>,
    absolute: DateTime<Utc>,
    idle: Duration,
    now: DateTime<Utc>,
) -> Expiry {
    if now >= absolute {
        Expiry::Absolute
    } else if now >= add(last_seen, idle) {
        Expiry::Idle
    } else {
        Expiry::Live
    }
}

/// `min(last_seen + idle, absolute)`.
pub fn idle_deadline(
    last_seen: DateTime<Utc>,
    absolute: DateTime<Utc>,
    idle: Duration,
) -> DateTime<Utc> {
    add(last_seen, idle).min(absolute)
}

pub fn add(t: DateTime<Utc>, d: Duration) -> DateTime<Utc> {
    chrono::Duration::from_std(d)
        .ok()
        .and_then(|d| t.checked_add_signed(d))
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

pub fn sub(t: DateTime<Utc>, d: Duration) -> DateTime<Utc> {
    chrono::Duration::from_std(d)
        .ok()
        .and_then(|d| t.checked_sub_signed(d))
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}

/// Truncate to at most `max` bytes on a char boundary.
pub fn truncate(s: &str, max: usize) -> String {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AuthRequest {
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
    pub return_to: String,
    pub expires_at: DateTime<Utc>,
    /// Fresh authentication required (`prompt=login` was sent).
    pub reauth: bool,
    /// When `/login` issued this request (the `auth_time` floor).
    pub issued_at: Option<DateTime<Utc>>,
}

pub struct NewSession<'a> {
    pub id_hash: &'a [u8],
    pub user_sub: &'a str,
    pub username: &'a str,
    pub display_name: &'a str,
    pub roles: &'a [String],
    pub csrf_token: &'a [u8],
    pub id_token: &'a str,
    pub refresh_token: Option<&'a str>,
    pub now: DateTime<Utc>,
    pub absolute_expires_at: DateTime<Utc>,
    /// This gateway's idle limit, stored on the row (POAM-031).
    pub idle: Duration,
    pub acr: Option<&'a str>,
    pub client_ip: Option<IpAddr>,
    pub user_agent: Option<&'a str>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct DeletedSession {
    pub user_sub: String,
    pub username: String,
    pub id_token: String,
}

/// Values a successful refresh grant writes back.
pub struct Refreshed<'a> {
    pub roles: &'a [String],
    pub id_token: &'a str,
    pub refresh_token: Option<&'a str>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct SweptSession {
    pub id_hash: Vec<u8>,
    pub user_sub: String,
    pub username: String,
    pub absolute: bool,
    pub client_ip: Option<String>,
}

/// Queries are runtime-checked (`sqlx::query*`), not `query!`: see impl notes
/// (no database was reachable to produce `.sqlx/` offline data). Every query
/// binds parameters; none concatenates input.
#[derive(Clone)]
pub struct SessionStore {
    pool: PgPool,
}

pub(crate) const SESSION_COLS: &str = "id_hash, user_sub, username, display_name, roles, \
                            csrf_token, created_at, last_seen_at, absolute_expires_at, \
                            idle_timeout_secs, acr";

/// SQL for "the row's idle limit as an interval" with `$fallback` (an
/// integer seconds parameter) for legacy rows.
fn idle_interval(fallback_param: &str) -> String {
    format!("make_interval(secs => COALESCE(idle_timeout_secs, {fallback_param}))")
}

fn secs_i32(d: Duration) -> i32 {
    i32::try_from(d.as_secs()).unwrap_or(i32::MAX)
}

impl SessionStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// `Ok(false)` when [`MAX_PENDING_AUTH`] live requests already exist
    /// (the count is bounded by a `LIMIT`, so the check is O(cap) at worst).
    // COMPLIANCE MAPPING:
    // - NIST 800-53: SC-5 (Denial of Service Protection) — global bound on pending sign-ins
    pub async fn insert_auth_request(
        &self,
        id_hash: &[u8],
        req: &AuthRequest,
        now: DateTime<Utc>,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "INSERT INTO auth_requests \
               (id_hash, state, nonce, pkce_verifier, return_to, expires_at, reauth, issued_at) \
             SELECT $1, $2, $3, $4, $5, $6, $7, $8 \
             WHERE (SELECT count(*) FROM (SELECT 1 FROM auth_requests \
                    WHERE expires_at > $9 LIMIT $10) live) < $10",
        )
        .bind(id_hash)
        .bind(&req.state)
        .bind(&req.nonce)
        .bind(&req.pkce_verifier)
        .bind(&req.return_to)
        .bind(req.expires_at)
        .bind(req.reauth)
        .bind(req.issued_at)
        .bind(now)
        .bind(MAX_PENDING_AUTH)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Single use: the row is deleted whether or not it is still valid.
    pub async fn take_auth_request(
        &self,
        id_hash: &[u8],
        now: DateTime<Utc>,
    ) -> Result<Option<AuthRequest>, sqlx::Error> {
        let row: Option<AuthRequest> = sqlx::query_as(
            "DELETE FROM auth_requests WHERE id_hash = $1 \
             RETURNING state, nonce, pkce_verifier, return_to, expires_at, reauth, issued_at",
        )
        .bind(id_hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.filter(|r| r.expires_at > now))
    }

    /// Per-request lookup. Pool acquire and query are timed separately in
    /// `debug` events under the `session.lookup` span (PERF-005).
    pub async fn load(&self, id_hash: &[u8]) -> Result<Option<Session>, sqlx::Error> {
        // QA F3 / perf MINOR (c): read-only, so retried once on a dead
        // pooled connection (pool runs with `test_before_acquire(false)`).
        match self.load_once(id_hash).await {
            Err(e) if is_connection_error(&e) => {
                tracing::warn!(error = %e, "session lookup hit a dead connection; retrying once");
                self.load_once(id_hash).await
            }
            r => r,
        }
    }

    async fn load_once(&self, id_hash: &[u8]) -> Result<Option<Session>, sqlx::Error> {
        async {
            let t0 = std::time::Instant::now();
            let mut conn = self.pool.acquire().await?;
            let acquired = t0.elapsed();
            let row = sqlx::query_as(&format!(
                "SELECT {SESSION_COLS} FROM sessions WHERE id_hash = $1"
            ))
            .bind(id_hash)
            .fetch_optional(&mut *conn)
            .await;
            tracing::debug!(
                acquire_us = u64::try_from(acquired.as_micros()).unwrap_or(u64::MAX),
                total_us = u64::try_from(t0.elapsed().as_micros()).unwrap_or(u64::MAX),
                "session lookup"
            );
            row
        }
        .instrument(tracing::debug_span!("session.lookup"))
        .await
    }

    /// The stored refresh token (kept out of [`Session`] so it never sits in
    /// request extensions or `Debug` output).
    pub async fn refresh_token(&self, id_hash: &[u8]) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT refresh_token FROM sessions WHERE id_hash = $1")
                .bind(id_hash)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|r| r.0))
    }

    /// Start-up check that the schema includes the latest migration.
    pub async fn schema_ready(&self) -> bool {
        sqlx::query("SELECT refresh_token, id_token FROM sessions LIMIT 0")
            .execute(&self.pool)
            .await
            .is_ok()
            && sqlx::query("SELECT reauth, issued_at FROM auth_requests LIMIT 0")
                .execute(&self.pool)
                .await
                .is_ok()
            && sqlx::query("SELECT idle_timeout_secs, idle_expires_at, acr FROM sessions LIMIT 0")
                .execute(&self.pool)
                .await
                .is_ok()
            && sqlx::query("SELECT 1 FROM tool_sessions, tool_launch_codes LIMIT 0")
                .execute(&self.pool)
                .await
                .is_ok()
    }

    /// Rotation: deletes any session for the presented (pre-login) id and
    /// inserts the new one, in a transaction the caller commits only after
    /// the audit record is written (AU-5 fail closed).
    // COMPLIANCE MAPPING:
    // - NIST 800-53: SC-23 (Session Authenticity) — no session fixation: fresh id on every login
    pub async fn begin_login(
        &self,
        previous: Option<&[u8]>,
        new: &NewSession<'_>,
    ) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        if let Some(prev) = previous {
            sqlx::query("DELETE FROM sessions WHERE id_hash = $1")
                .bind(prev)
                .execute(&mut *tx)
                .await?;
        }
        // POAM: SC-28 - id_token and refresh_token are stored in plaintext at rest (accepted for M3; M7 encrypts).
        sqlx::query(
            "INSERT INTO sessions (id_hash, user_sub, username, display_name, roles, csrf_token, \
             id_token, created_at, last_seen_at, absolute_expires_at, client_ip, user_agent, \
             refresh_token, idle_timeout_secs, idle_expires_at, acr) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8, $9, $10::inet, $11, $12, $13, $14, $15)",
        )
        .bind(new.id_hash)
        .bind(new.user_sub)
        .bind(new.username)
        .bind(new.display_name)
        .bind(new.roles)
        .bind(new.csrf_token)
        .bind(new.id_token)
        .bind(new.now)
        .bind(new.absolute_expires_at)
        .bind(new.client_ip.map(|ip| ip.to_string()))
        .bind(new.user_agent)
        .bind(new.refresh_token)
        .bind(secs_i32(new.idle))
        .bind(idle_deadline(new.now, DateTime::<Utc>::MAX_UTC, new.idle))
        .bind(new.acr)
        .execute(&mut *tx)
        .await?;
        Ok(tx)
    }

    /// Deletes the session; `None` if it was already gone (e.g. swept).
    pub async fn delete(&self, id_hash: &[u8]) -> Result<Option<DeletedSession>, sqlx::Error> {
        sqlx::query_as(
            "DELETE FROM sessions WHERE id_hash = $1 RETURNING user_sub, username, id_token",
        )
        .bind(id_hash)
        .fetch_optional(&self.pool)
        .await
    }

    /// Keepalive: moves `last_seen_at` (and the row's idle deadline) only
    /// while the session is still live under the row's own idle limit.
    /// `idle` is the fallback for legacy rows, which adopt it.
    pub async fn touch(
        &self,
        id_hash: &[u8],
        now: DateTime<Utc>,
        idle: Duration,
    ) -> Result<bool, sqlx::Error> {
        let iv = idle_interval("$3");
        let res = sqlx::query(&format!(
            "UPDATE sessions SET last_seen_at = $2, idle_expires_at = $2 + {iv}, \
                    idle_timeout_secs = COALESCE(idle_timeout_secs, $3) \
             WHERE id_hash = $1 AND absolute_expires_at > $2 AND last_seen_at + {iv} > $2"
        ))
        .bind(id_hash)
        .bind(now)
        .bind(secs_i32(idle))
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Keepalive after a successful IdP refresh: moves `last_seen_at` and
    /// stores the refreshed roles and tokens, only while still live.
    pub async fn touch_refreshed(
        &self,
        id_hash: &[u8],
        now: DateTime<Utc>,
        idle: Duration,
        refreshed: &Refreshed<'_>,
    ) -> Result<bool, sqlx::Error> {
        let iv = idle_interval("$3");
        let res = sqlx::query(&format!(
            "UPDATE sessions SET last_seen_at = $2, roles = $4, id_token = $5, \
                    refresh_token = COALESCE($6, refresh_token), \
                    idle_expires_at = $2 + {iv}, \
                    idle_timeout_secs = COALESCE(idle_timeout_secs, $3) \
             WHERE id_hash = $1 AND absolute_expires_at > $2 AND last_seen_at + {iv} > $2"
        ))
        .bind(id_hash)
        .bind(now)
        .bind(secs_i32(idle))
        .bind(refreshed.roles)
        .bind(refreshed.id_token)
        .bind(refreshed.refresh_token)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Deletes up to `limit` expired sessions (tool sessions and launch
    /// codes cascade), all expired auth requests and launch codes.
    /// Predicate (POAM-031): `absolute_expires_at <= now OR
    /// COALESCE(idle_expires_at, last_seen_at + idle) <= now`, as bounded
    /// index range scans (`UNION ALL`) instead of an `OR` across columns
    /// (PERF-003); `IN` de-duplicates. `idle` applies to legacy rows only.
    // COMPLIANCE MAPPING:
    // - NIST 800-53: AC-12 (Session Termination) — each row expires under its own idle limit
    pub async fn sweep(
        &self,
        now: DateTime<Utc>,
        idle: Duration,
        limit: i64,
    ) -> Result<Vec<SweptSession>, sqlx::Error> {
        let swept = sqlx::query_as(
            "DELETE FROM sessions WHERE id_hash IN ( \
                 (SELECT id_hash FROM sessions WHERE idle_expires_at <= $2 LIMIT $3) \
                 UNION ALL \
                 (SELECT id_hash FROM sessions \
                  WHERE idle_expires_at IS NULL AND last_seen_at <= $1 LIMIT $3) \
                 UNION ALL \
                 (SELECT id_hash FROM sessions WHERE absolute_expires_at <= $2 LIMIT $3)) \
             RETURNING id_hash, user_sub, username, absolute_expires_at <= $2 AS absolute, \
                       host(client_ip) AS client_ip",
        )
        .bind(sub(now, idle))
        .bind(now)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        sqlx::query("DELETE FROM auth_requests WHERE expires_at <= $1")
            .bind(now)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM tool_launch_codes WHERE expires_at <= $1")
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(swept)
    }
}

/// Connection-class failures (socket closed/reset, TLS, wire desync) that
/// a fresh pooled connection may not repeat. Only read-only lookups retry
/// on these; writes never do (they may have been applied).
pub(crate) fn is_connection_error(e: &sqlx::Error) -> bool {
    matches!(
        e,
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) | sqlx::Error::Protocol(_)
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn connection_errors_are_retry_class_only() {
        let io = sqlx::Error::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset));
        assert!(super::is_connection_error(&io));
        assert!(super::is_connection_error(&sqlx::Error::Protocol(
            "desync".into()
        )));
        assert!(!super::is_connection_error(&sqlx::Error::RowNotFound));
        assert!(!super::is_connection_error(&sqlx::Error::PoolTimedOut));
        assert!(!super::is_connection_error(&sqlx::Error::PoolClosed));
    }

    use super::*;
    use chrono::TimeZone;

    fn t(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_800_000_000 + secs, 0).unwrap()
    }

    #[test]
    fn cookie_flags() {
        let c = set_cookie(SID_COOKIE, "abc".into(), None);
        assert!(c.starts_with("__Host-console_sid=abc"));
        for flag in ["Secure", "HttpOnly", "SameSite=Lax", "Path=/"] {
            assert!(c.contains(flag), "{c} missing {flag}");
        }
        assert!(!c.contains("Domain"));
        assert!(!c.contains("Max-Age"));
        let a = set_cookie(AUTH_COOKIE, "x".into(), Some(AUTH_REQUEST_TTL));
        assert!(a.contains("Max-Age=600"));
        let r = reauth_cookie(Duration::from_secs(43_200));
        for flag in [
            "__Host-console_reauth=1",
            "Secure",
            "HttpOnly",
            "Max-Age=43200",
        ] {
            assert!(r.contains(flag), "{r} missing {flag}");
        }
        let strict = set_cookie_strict("__Host-tool_sid", "t".into(), None);
        assert!(strict.contains("SameSite=Strict") && strict.contains("HttpOnly"));
        assert!(!strict.contains("Domain") && !strict.contains("Max-Age"));
        assert_eq!(idle_or(Some(150), Duration::from_secs(900)).as_secs(), 150);
        assert_eq!(idle_or(None, Duration::from_secs(900)).as_secs(), 900);
        assert_eq!(idle_or(Some(0), Duration::from_secs(900)).as_secs(), 900);
        let cleared = clear_cookie(SID_COOKIE);
        assert!(cleared.contains("Max-Age=0") && cleared.contains("Secure"));
    }

    #[test]
    fn opaque_ids() {
        let (v1, h1) = new_opaque_id().unwrap();
        let (v2, _) = new_opaque_id().unwrap();
        assert_ne!(v1, v2);
        assert_eq!(v1.len(), 43);
        assert_eq!(hash_cookie_value(&v1), Some(h1.clone()));
        assert_eq!(hash_cookie_value("short"), None);
        assert_eq!(hash_cookie_value(&B64URL.encode([0u8; 31])), None);
        assert_eq!(session_ref(&h1).len(), 8);
    }

    #[test]
    fn reads_cookie_among_many() {
        let mut h = HeaderMap::new();
        h.append(
            header::COOKIE,
            "a=1; __Host-console_sid=xyz".parse().unwrap(),
        );
        h.append(header::COOKIE, "__Host-console_auth=q".parse().unwrap());
        assert_eq!(read_cookie(&h, SID_COOKIE).as_deref(), Some("xyz"));
        assert_eq!(read_cookie(&h, AUTH_COOKIE).as_deref(), Some("q"));
        assert_eq!(read_cookie(&h, "missing"), None);
    }

    #[test]
    fn expiry_boundaries() {
        let idle = Duration::from_secs(900);
        let abs = t(43_200);
        assert_eq!(expiry(t(0), abs, idle, t(899)), Expiry::Live);
        assert_eq!(
            expiry(t(0), abs, idle, t(900)),
            Expiry::Idle,
            "exactly at idle deadline"
        );
        assert_eq!(expiry(t(43_000), abs, idle, t(43_199)), Expiry::Live);
        assert_eq!(expiry(t(43_000), abs, idle, t(43_200)), Expiry::Absolute);
        assert_eq!(
            expiry(t(0), abs, idle, t(50_000)),
            Expiry::Absolute,
            "absolute wins when both passed"
        );
        assert_eq!(idle_deadline(t(0), abs, idle), t(900));
        assert_eq!(
            idle_deadline(t(42_800), abs, idle),
            abs,
            "capped by absolute"
        );
    }

    #[test]
    fn truncates_on_char_boundary() {
        assert_eq!(truncate("héllo", 2), "h");
        assert_eq!(truncate("abc", 256), "abc");
    }
}
