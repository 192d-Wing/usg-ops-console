//! Verification of the gateway's `X-Console-Assertion` (plan §4.3).
//!
//! Rules every tool backend should copy:
//! - pin `alg = ES256` and `typ = console-assertion+jwt`; a header that says
//!   anything else (`none`, `HS256`, ...) is rejected before key lookup;
//! - pick the key by `kid` from the gateway JWKS;
//! - require `iss` (console origin), `aud == own tool id`, `exp`, `nbf`, `sub`,
//!   with leeway [`ASSERTION_MAX_LEEWAY_SECS`].
//!
//! JWKS cache (PERF-004 / SEC-006):
//! - the hot path takes a read lock only long enough to clone an `Arc` of the
//!   key set; no lock is held across `.await` on the verify path;
//! - refreshes are single-flight (one fetch at a time, behind a separate
//!   async mutex that never blocks readers) and happen outside the cache lock;
//! - after [`JWKS_REFRESH_AFTER`] one request refreshes while every other
//!   request keeps using the last good keys; a failed refresh keeps them;
//! - after [`JWKS_MAX_AGE`] without a successful fetch, verification fails
//!   closed (503 `KeysUnavailable`);
//! - an unknown `kid` refetches at most once per [`JWKS_MIN_REFETCH`];
//! - the fetch has a total timeout and a streamed size limit (the body is
//!   never buffered past [`JWKS_MAX_BYTES`]).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use console_protocol::AssertionClaims;
use console_protocol::tools::{ASSERTION_MAX_LEEWAY_SECS, ASSERTION_TYP};
use jsonwebtoken::jwk::{AlgorithmParameters, EllipticCurve, Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use tokio::sync::Mutex;

/// Background refresh age (matches the gateway's JWKS `max-age=300`).
pub const JWKS_REFRESH_AFTER: Duration = Duration::from_secs(300);
/// Keys older than this are never used (fail closed).
pub const JWKS_MAX_AGE: Duration = Duration::from_secs(600);
/// Minimum gap between refetches caused by an unknown `kid`.
pub const JWKS_MIN_REFETCH: Duration = Duration::from_secs(60);
/// Minimum gap between retries while the keys are expired and the JWKS is
/// down, so an outage does not turn every request into a fetch.
pub const JWKS_RETRY_MIN: Duration = Duration::from_secs(5);
const JWKS_FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const JWKS_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
pub const JWKS_MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VerifyError {
    #[error("missing assertion header")]
    Missing,
    #[error("malformed assertion")]
    Malformed,
    #[error("unexpected alg or typ")]
    BadHeader,
    #[error("unknown key id")]
    UnknownKid,
    #[error("signing keys unavailable")]
    KeysUnavailable,
    #[error("invalid assertion: {0}")]
    Invalid(String),
}

/// Immutable snapshot of the key set.
#[derive(Default)]
struct KeySet {
    keys: HashMap<String, DecodingKey>,
    /// Last successful fetch (`None`: never). Static sets use `None` + `fixed`.
    fetched_at: Option<Instant>,
}

#[derive(Clone, Copy)]
struct Timing {
    refresh_after: Duration,
    max_age: Duration,
    retry_min: Duration,
}

/// JWKS-backed verifier. `url = None` means a static key set (tests).
pub struct Verifier {
    issuer: String,
    url: Option<String>,
    http: reqwest::Client,
    timing: Timing,
    /// Read path: clone the `Arc` and drop the guard immediately.
    current: RwLock<Arc<KeySet>>,
    /// Single-flight refresh; holds the time of the last fetch attempt.
    refresh: Mutex<Option<Instant>>,
}

impl Verifier {
    pub fn remote(issuer: String, jwks_url: String) -> Result<Self, String> {
        Self::remote_with(
            issuer,
            jwks_url,
            Timing {
                refresh_after: JWKS_REFRESH_AFTER,
                max_age: JWKS_MAX_AGE,
                retry_min: JWKS_RETRY_MIN,
            },
        )
    }

    fn remote_with(issuer: String, jwks_url: String, timing: Timing) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(JWKS_FETCH_TIMEOUT)
            .connect_timeout(JWKS_CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("JWKS client: {e}"))?;
        Ok(Self {
            issuer,
            url: Some(jwks_url),
            http,
            timing,
            current: RwLock::new(Arc::new(KeySet::default())),
            refresh: Mutex::new(None),
        })
    }

    /// Fixed key set that never expires or refetches.
    pub fn with_static_keys(issuer: String, set: &JwkSet) -> Self {
        Self {
            issuer,
            url: None,
            http: reqwest::Client::new(),
            timing: Timing {
                refresh_after: Duration::MAX,
                max_age: Duration::MAX,
                retry_min: Duration::MAX,
            },
            current: RwLock::new(Arc::new(KeySet {
                keys: keys_from_set(set),
                fetched_at: Some(Instant::now()),
            })),
            refresh: Mutex::new(None),
        }
    }

    /// Verify `token` for tool `aud`. Returns the trusted claims.
    pub async fn verify(&self, token: &str, aud: &str) -> Result<AssertionClaims, VerifyError> {
        let header = decode_header(token).map_err(|_| VerifyError::Malformed)?;
        // `Algorithm::ES256` is `ASSERTION_ALG` ("ES256"); pinned, never taken from the token.
        if header.alg != Algorithm::ES256 || header.typ.as_deref() != Some(ASSERTION_TYP) {
            return Err(VerifyError::BadHeader);
        }
        let kid = header.kid.ok_or(VerifyError::BadHeader)?;
        let key = self.key_for(&kid).await?;

        let mut v = Validation::new(Algorithm::ES256);
        v.leeway = ASSERTION_MAX_LEEWAY_SECS;
        v.validate_nbf = true;
        v.set_audience(&[aud]);
        v.set_issuer(&[self.issuer.as_str()]);
        v.set_required_spec_claims(&["exp", "nbf", "iss", "aud", "sub"]);
        let data = decode::<AssertionClaims>(token, &key, &v)
            .map_err(|e| VerifyError::Invalid(format!("{:?}", e.kind())))?;
        Ok(data.claims)
    }

    fn snapshot(&self) -> Arc<KeySet> {
        self.current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn age(&self, set: &KeySet, now: Instant) -> Option<Duration> {
        set.fetched_at.map(|t| now.saturating_duration_since(t))
    }

    async fn key_for(&self, kid: &str) -> Result<DecodingKey, VerifyError> {
        let set = self.snapshot();
        let Some(url) = self.url.as_deref() else {
            return set.keys.get(kid).cloned().ok_or(VerifyError::UnknownKid);
        };
        let now = Instant::now();
        let age = self.age(&set, now);
        let usable = age.is_some_and(|a| a < self.timing.max_age);
        if usable && let Some(k) = set.keys.get(kid) {
            if age.is_some_and(|a| a >= self.timing.refresh_after) {
                // Due for refresh: at most one request does it; everyone
                // else (including this one on failure) keeps the last good
                // keys until max-age.
                if let Ok(mut last) = self.refresh.try_lock()
                    && self.snapshot().fetched_at == set.fetched_at
                    && last
                        .is_none_or(|t| now.saturating_duration_since(t) >= self.timing.retry_min)
                {
                    self.refetch(url, &mut last).await;
                }
            }
            return Ok(k.clone());
        }
        // Unknown kid or expired keys: wait for the single in-flight fetch.
        let mut last = self.refresh.lock().await;
        let latest = self.snapshot();
        if latest.fetched_at != set.fetched_at {
            // Someone refreshed while we waited.
            return self.pick(&latest, kid);
        }
        let gap = if usable {
            JWKS_MIN_REFETCH
        } else {
            self.timing.retry_min
        };
        if last.is_none_or(|t| now.saturating_duration_since(t) >= gap) {
            self.refetch(url, &mut last).await;
        }
        self.pick(&self.snapshot(), kid)
    }

    fn pick(&self, set: &KeySet, kid: &str) -> Result<DecodingKey, VerifyError> {
        if !self
            .age(set, Instant::now())
            .is_some_and(|a| a < self.timing.max_age)
        {
            // Fail closed: never verify with keys we could not refresh.
            return Err(VerifyError::KeysUnavailable);
        }
        set.keys.get(kid).cloned().ok_or(VerifyError::UnknownKid)
    }

    /// Fetch and publish. Called with the refresh mutex held, never the
    /// cache lock. A failure keeps the last good keys.
    async fn refetch(&self, url: &str, last: &mut Option<Instant>) {
        *last = Some(Instant::now());
        match self.fetch(url).await {
            Ok(set) => {
                let next = Arc::new(KeySet {
                    keys: keys_from_set(&set),
                    fetched_at: Some(Instant::now()),
                });
                *self
                    .current
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
            }
            Err(e) => tracing::warn!(error = %e, "JWKS fetch failed; keeping last good keys"),
        }
    }

    async fn fetch(&self, url: &str) -> Result<JwkSet, String> {
        let mut resp = self.http.get(url).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("status {}", resp.status()));
        }
        if resp
            .content_length()
            .is_some_and(|n| n > JWKS_MAX_BYTES as u64)
        {
            return Err("JWKS too large".into());
        }
        // Streamed limit: stop reading as soon as the cap is exceeded.
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            if body.len() + chunk.len() > JWKS_MAX_BYTES {
                return Err("JWKS too large".into());
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|e| e.to_string())
    }
}

/// Usable keys only: EC P-256 with a `kid`. Anything else is skipped so a
/// stray RSA/HMAC key in the set can never be selected.
fn keys_from_set(set: &JwkSet) -> HashMap<String, DecodingKey> {
    set.keys
        .iter()
        .filter_map(|jwk: &Jwk| {
            let kid = jwk.common.key_id.clone()?;
            match &jwk.algorithm {
                AlgorithmParameters::EllipticCurve(p) if p.curve == EllipticCurve::P256 => {
                    DecodingKey::from_jwk(jwk).ok().map(|k| (kid, k))
                }
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
pub(crate) use tests::{test_token, test_verifier};

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};

    pub(crate) const ISS: &str = "https://console.test";
    const KEY_A: &str = include_str!("../tests/keys/test-a.pem");
    const KEY_B: &str = include_str!("../tests/keys/test-b.pem");

    fn jwks() -> JwkSet {
        serde_json::from_value(serde_json::json!({"keys": [{
            "kty": "EC", "crv": "P-256", "use": "sig", "alg": "ES256", "kid": "kid-a",
            "x": "8ELcWO36LLKgHvJ1zK7KduW_HJ0BG68OKQLx4LeYaSA",
            "y": "O0OB4vemy4dZv8eeEEG9RU3zaL70M2Gs4gAJVbsq04k"
        }]}))
        .unwrap()
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn claims(aud: &str, iat: u64) -> AssertionClaims {
        serde_json::from_value(serde_json::json!({
            "iss": ISS, "aud": aud, "sub": "u1", "preferred_username": "jdoe",
            "name": "Jane Doe", "roles": ["hello:user"], "ctx": {"site": "dc1"},
            "sid": "s", "jti": "j", "iat": iat, "nbf": iat, "exp": iat + 60
        }))
        .unwrap()
    }

    fn sign(c: &AssertionClaims, pem: &str, kid: &str) -> String {
        let mut h = Header::new(Algorithm::ES256);
        h.kid = Some(kid.into());
        h.typ = Some(ASSERTION_TYP.into());
        encode(&h, c, &EncodingKey::from_ec_pem(pem.as_bytes()).unwrap()).unwrap()
    }

    fn verifier() -> Verifier {
        Verifier::with_static_keys(ISS.into(), &jwks())
    }

    #[tokio::test]
    async fn accepts_valid() {
        let t = sign(&claims("hello", now()), KEY_A, "kid-a");
        let c = verifier().verify(&t, "hello").await.unwrap();
        assert_eq!(c.name, "Jane Doe");
        assert_eq!(c.ctx.site.as_deref(), Some("dc1"));
    }

    #[tokio::test]
    async fn rejects_wrong_aud() {
        let t = sign(&claims("evil", now()), KEY_A, "kid-a");
        assert!(matches!(
            verifier().verify(&t, "hello").await,
            Err(VerifyError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn rejects_wrong_issuer() {
        let mut c = claims("hello", now());
        c.iss = "https://attacker.test".into();
        let t = sign(&c, KEY_A, "kid-a");
        assert!(matches!(
            verifier().verify(&t, "hello").await,
            Err(VerifyError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn rejects_expired_beyond_leeway() {
        let t = sign(&claims("hello", now() - 60 - 31), KEY_A, "kid-a");
        assert!(matches!(
            verifier().verify(&t, "hello").await,
            Err(VerifyError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn accepts_expired_within_leeway() {
        let t = sign(&claims("hello", now() - 60 - 10), KEY_A, "kid-a");
        assert!(verifier().verify(&t, "hello").await.is_ok());
    }

    #[tokio::test]
    async fn rejects_unknown_kid_and_wrong_key() {
        let t = sign(&claims("hello", now()), KEY_A, "kid-x");
        assert_eq!(
            verifier().verify(&t, "hello").await,
            Err(VerifyError::UnknownKid)
        );
        // Signed by key B but claims to be kid-a.
        let t = sign(&claims("hello", now()), KEY_B, "kid-a");
        assert!(matches!(
            verifier().verify(&t, "hello").await,
            Err(VerifyError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn rejects_alg_none_hs256_and_bad_typ() {
        let c = claims("hello", now());
        // alg=none: hand-built token.
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let none = format!(
            "{}.{}.",
            b64.encode(r#"{"alg":"none","typ":"console-assertion+jwt","kid":"kid-a"}"#),
            b64.encode(serde_json::to_vec(&c).unwrap())
        );
        assert!(verifier().verify(&none, "hello").await.is_err());
        // HS256 keyed with the public x coordinate (classic alg confusion).
        let mut h = Header::new(Algorithm::HS256);
        h.kid = Some("kid-a".into());
        h.typ = Some(ASSERTION_TYP.into());
        let hs = encode(&h, &c, &EncodingKey::from_secret(b"8ELcWO36")).unwrap();
        assert_eq!(
            verifier().verify(&hs, "hello").await,
            Err(VerifyError::BadHeader)
        );
        // Right key, wrong typ.
        let mut h = Header::new(Algorithm::ES256);
        h.kid = Some("kid-a".into());
        let t = encode(&h, &c, &EncodingKey::from_ec_pem(KEY_A.as_bytes()).unwrap()).unwrap();
        assert_eq!(
            verifier().verify(&t, "hello").await,
            Err(VerifyError::BadHeader)
        );
    }

    #[tokio::test]
    async fn rejects_garbage() {
        assert_eq!(
            verifier().verify("not-a-jwt", "hello").await,
            Err(VerifyError::Malformed)
        );
    }

    #[tokio::test]
    async fn remote_unreachable_fails_closed() {
        let v = Verifier::remote(ISS.into(), "http://127.0.0.1:9/jwks.json".into()).unwrap();
        let t = sign(&claims("hello", now()), KEY_A, "kid-a");
        assert_eq!(
            v.verify(&t, "hello").await,
            Err(VerifyError::KeysUnavailable)
        );
    }

    /// Local JWKS endpoint: serves `body` while `up`, else 503; counts hits.
    async fn jwks_server(
        body: String,
        up: Arc<std::sync::atomic::AtomicBool>,
        hits: Arc<std::sync::atomic::AtomicUsize>,
    ) -> String {
        use std::sync::atomic::Ordering;
        let app = axum::Router::new().route(
            "/jwks.json",
            axum::routing::get(move || {
                let (body, up, hits) = (body.clone(), up.clone(), hits.clone());
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    // Make concurrent callers overlap with the fetch.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    if up.load(Ordering::SeqCst) {
                        (axum::http::StatusCode::OK, body)
                    } else {
                        (axum::http::StatusCode::SERVICE_UNAVAILABLE, String::new())
                    }
                }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://{addr}/jwks.json")
    }

    fn flags() -> (
        Arc<std::sync::atomic::AtomicBool>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        (
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cold_cache_fetch_is_single_flight() {
        let (up, hits) = flags();
        let url = jwks_server(serde_json::to_string(&jwks()).unwrap(), up, hits.clone()).await;
        let v = Arc::new(Verifier::remote(ISS.into(), url).unwrap());
        let t = sign(&claims("hello", now()), KEY_A, "kid-a");
        let tasks: Vec<_> = (0..20)
            .map(|_| {
                let (v, t) = (v.clone(), t.clone());
                tokio::spawn(async move { v.verify(&t, "hello").await })
            })
            .collect();
        for task in tasks {
            assert!(task.await.unwrap().is_ok());
        }
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn serves_last_good_keys_then_fails_closed() {
        use std::sync::atomic::Ordering;
        let (up, hits) = flags();
        let url = jwks_server(
            serde_json::to_string(&jwks()).unwrap(),
            up.clone(),
            hits.clone(),
        )
        .await;
        let t = sign(&claims("hello", now()), KEY_A, "kid-a");
        // Always due for refresh, never expired: a JWKS outage is invisible.
        let v = Verifier::remote_with(
            ISS.into(),
            url.clone(),
            Timing {
                refresh_after: Duration::ZERO,
                max_age: Duration::from_secs(3600),
                retry_min: Duration::ZERO,
            },
        )
        .unwrap();
        assert!(v.verify(&t, "hello").await.is_ok());
        up.store(false, Ordering::SeqCst);
        assert!(v.verify(&t, "hello").await.is_ok(), "last good keys served");
        assert_eq!(
            hits.load(Ordering::SeqCst),
            2,
            "a refresh was attempted and failed"
        );
        // Expired immediately: no successful refresh means fail closed.
        let v = Verifier::remote_with(
            ISS.into(),
            url,
            Timing {
                refresh_after: Duration::ZERO,
                max_age: Duration::from_millis(1),
                retry_min: Duration::ZERO,
            },
        )
        .unwrap();
        assert_eq!(
            v.verify(&t, "hello").await,
            Err(VerifyError::KeysUnavailable)
        );
    }

    #[tokio::test]
    async fn oversized_jwks_is_refused() {
        let (up, hits) = flags();
        let big = format!(
            r#"{{"keys":[],"pad":"{}"}}"#,
            "x".repeat(JWKS_MAX_BYTES + 1)
        );
        let url = jwks_server(big, up, hits).await;
        let v = Verifier::remote(ISS.into(), url).unwrap();
        let t = sign(&claims("hello", now()), KEY_A, "kid-a");
        assert_eq!(
            v.verify(&t, "hello").await,
            Err(VerifyError::KeysUnavailable)
        );
    }

    /// Shared with the router tests.
    pub(crate) fn test_token(aud: &str) -> String {
        sign(&claims(aud, now()), KEY_A, "kid-a")
    }
    pub(crate) fn test_verifier() -> Verifier {
        verifier()
    }
}
