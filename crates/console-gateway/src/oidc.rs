//! Keycloak OIDC relying party: discovery, authorize URL (PKCE S256 + nonce),
//! code exchange (`client_secret_basic`), id_token verification, and the
//! RP-initiated logout URL.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use openidconnect::core::{
    CoreAuthDisplay, CoreAuthPrompt, CoreAuthenticationFlow, CoreErrorResponseType,
    CoreGenderClaim, CoreJsonWebKey, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm,
    CoreRevocableToken, CoreRevocationErrorResponse, CoreTokenIntrospectionResponse, CoreTokenType,
};
use openidconnect::{
    AdditionalClaims, AuthenticationContextClass, AuthorizationCode, Client, ClientId,
    ClientSecret, CsrfToken, EmptyExtraTokenFields, EndpointMaybeSet, EndpointNotSet, EndpointSet,
    IdToken, IdTokenFields, IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge,
    PkceCodeVerifier, ProviderMetadataWithLogout, RedirectUrl, RefreshToken, RequestTokenError,
    Scope, StandardErrorResponse, StandardTokenResponse, TokenResponse,
};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::sync::RwLock;
use tracing::Instrument;
use url::Url;

use crate::config::Config;
use crate::session::{Clock, truncate};

/// Keycloak's `realm_access.roles` claim (needs the realm-roles mapper with
/// `id.token.claim=true`; see the usg-dcim gotcha).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KcClaims {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_access: Option<RealmAccess>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RealmAccess {
    #[serde(default)]
    pub roles: Vec<String>,
}

impl AdditionalClaims for KcClaims {}

pub type KcIdTokenFields = IdTokenFields<
    KcClaims,
    EmptyExtraTokenFields,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
>;
pub type KcTokenResponse = StandardTokenResponse<KcIdTokenFields, CoreTokenType>;
pub type KcIdToken =
    IdToken<KcClaims, CoreGenderClaim, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm>;
type KcClient = Client<
    KcClaims,
    CoreAuthDisplay,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJsonWebKey,
    CoreAuthPrompt,
    StandardErrorResponse<CoreErrorResponseType>,
    KcTokenResponse,
    CoreTokenIntrospectionResponse,
    CoreRevocableToken,
    CoreRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// Signature algorithms accepted for id_tokens (no `none`, no HS*).
const ALLOWED_ALGS: [CoreJwsSigningAlgorithm; 3] = [
    CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
    CoreJwsSigningAlgorithm::RsaSsaPssSha256,
    CoreJwsSigningAlgorithm::EcdsaP256Sha256,
];
const IAT_SKEW: chrono::Duration = chrono::Duration::seconds(60);
const JWKS_REFRESH_MIN: Duration = Duration::from_secs(60);
const MAX_ROLES: usize = 64;
const MAX_ROLE_LEN: usize = 128;
const MAX_NAME_LEN: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum OidcError {
    #[error("OIDC discovery failed for the configured issuer")]
    Discovery,
    #[error("OIDC configuration invalid: {0}")]
    Config(&'static str),
    #[error("HTTP client could not be built")]
    Http,
}

/// Why a callback failed. `code()` is the audit reason; no token text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackFailure {
    ExchangeFailed,
    TokenInvalid,
}

impl CallbackFailure {
    pub fn code(self) -> &'static str {
        match self {
            Self::ExchangeFailed => "exchange_failed",
            Self::TokenInvalid => "token_invalid",
        }
    }
}

/// Why a keepalive refresh grant failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshFailure {
    /// The IdP (or our verification) refused: end the session. The value is
    /// the `session.revoked` audit reason.
    Rejected(&'static str),
    /// Transport failure or timeout: do not extend, do not end.
    Unavailable,
}

/// Verified identity from the id_token. `roles` is unfiltered here; see
/// [`filter_roles`].
#[derive(Clone)]
pub struct Identity {
    pub sub: String,
    pub username: String,
    pub display_name: String,
    pub roles: Vec<String>,
    /// Kept server-side for `id_token_hint` at logout; never logged.
    pub id_token: String,
    /// Kept server-side for the keepalive refresh grant; never logged.
    pub refresh_token: Option<String>,
    /// `auth_time` claim (SEC-001 fresh-authentication check).
    pub auth_time: Option<DateTime<Utc>>,
    /// `acr` claim (SEC-002 assurance check).
    pub acr: Option<String>,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("sub", &self.sub)
            .field("username", &self.username)
            .field("roles", &self.roles)
            .field("acr", &self.acr)
            .field("auth_time", &self.auth_time)
            .field("id_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .finish()
    }
}

/// Exact (constant-time) nonce match, or, for refreshed id_tokens (`None`),
/// any nonce: OIDC Core 12.2 lets the IdP repeat or omit it.
fn nonce_check(expected: Option<&str>) -> impl FnOnce(Option<&Nonce>) -> Result<(), String> + '_ {
    move |got: Option<&Nonce>| match (expected, got) {
        (None, _) => Ok(()),
        (Some(e), Some(g)) if bool::from(e.as_bytes().ct_eq(g.secret().as_bytes())) => Ok(()),
        (Some(_), Some(_)) => Err("nonce mismatch".into()),
        (Some(_), None) => Err("nonce missing".into()),
    }
}

struct Provider {
    client: KcClient,
    end_session: Option<Url>,
    fetched_at: Instant,
}

pub struct OidcClient {
    http: reqwest::Client,
    /// Keepalive refresh grants only: connect 2 s, total
    /// `CONSOLE_OIDC_REFRESH_TIMEOUT_SECS` (PERF-008).
    refresh_http: reqwest::Client,
    issuer: IssuerUrl,
    client_id: ClientId,
    client_secret: ClientSecret,
    redirect: RedirectUrl,
    post_logout: String,
    clock: Arc<dyn Clock>,
    provider: RwLock<Arc<Provider>>,
}

// COMPLIANCE MAPPING:
// - NIST 800-53: SC-8 (Transmission Confidentiality) — rustls to the IdP; no redirects followed
// - NIST 800-53: SC-5 (Denial of Service Protection) — bounded IdP I/O timeouts
pub fn http_client() -> Result<reqwest::Client, OidcError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| OidcError::Http)
}

/// Client for the keepalive refresh grant: a short budget so a slow IdP
/// yields a retryable 503 quickly instead of holding the shell (PERF-008).
// COMPLIANCE MAPPING:
// - NIST 800-53: SC-5 (Denial of Service Protection) — bounded refresh-grant latency
pub fn refresh_http_client(total: Duration) -> Result<reqwest::Client, OidcError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2).min(total))
        .timeout(total)
        .build()
        .map_err(|_| OidcError::Http)
}

impl OidcClient {
    /// Discovers the provider, retrying every 2 s up to the configured count.
    pub async fn discover(cfg: &Config, clock: Arc<dyn Clock>) -> Result<Self, OidcError> {
        let http = http_client()?;
        let issuer =
            IssuerUrl::new(cfg.oidc_issuer.clone()).map_err(|_| OidcError::Config("issuer"))?;
        let mut attempt = 0;
        let meta = loop {
            match ProviderMetadataWithLogout::discover_async(issuer.clone(), &http).await {
                Ok(m) => break m,
                Err(e) if attempt < cfg.oidc_discovery_retries => {
                    attempt += 1;
                    tracing::warn!(attempt, error = %e, "OIDC discovery failed; retrying");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Err(e) => {
                    tracing::error!(error = %e, "OIDC discovery failed");
                    return Err(OidcError::Discovery);
                }
            }
        };
        Self::from_metadata(meta, cfg, http, clock)
    }

    /// Builds the client from already-fetched metadata (tests use this with
    /// a stub JWKS).
    pub fn from_metadata(
        meta: ProviderMetadataWithLogout,
        cfg: &Config,
        http: reqwest::Client,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, OidcError> {
        let issuer = meta.issuer().clone();
        let client_id = ClientId::new(cfg.oidc_client_id.clone());
        let client_secret = ClientSecret::new(cfg.oidc_client_secret.expose().to_owned());
        let redirect =
            RedirectUrl::new(cfg.redirect_url()).map_err(|_| OidcError::Config("redirect"))?;
        let provider = build_provider(meta, &client_id, &client_secret, &redirect)?;
        Ok(Self {
            http,
            refresh_http: refresh_http_client(cfg.oidc_refresh_timeout)?,
            issuer,
            client_id,
            client_secret,
            redirect,
            post_logout: cfg.post_logout_url(),
            clock,
            provider: RwLock::new(Arc::new(provider)),
        })
    }

    async fn provider(&self) -> Arc<Provider> {
        self.provider.read().await.clone()
    }

    /// Authorize URL for `state`/`nonce` and the S256 challenge of `verifier`.
    /// `reauth` adds `prompt=login` (no silent SSO reuse after expiry or
    /// sign-out); `acr` values are requested via `acr_values`.
    // COMPLIANCE MAPPING:
    // - NIST 800-53: AC-12 (Session Termination), IA-11 (Re-authentication) — prompt=login after a session ended
    // - NIST 800-53: IA-2(12) (Acceptance of PIV Credentials) — requested acr_values
    pub async fn authorize_url(
        &self,
        state: &str,
        nonce: &str,
        verifier: &str,
        reauth: bool,
        acr: &[String],
    ) -> Url {
        let p = self.provider().await;
        let challenge =
            PkceCodeChallenge::from_code_verifier_sha256(&PkceCodeVerifier::new(verifier.into()));
        let (state, nonce) = (state.to_owned(), nonce.to_owned());
        let mut req = p
            .client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                move || CsrfToken::new(state),
                move || Nonce::new(nonce),
            )
            .add_scope(Scope::new("profile".into()))
            .set_pkce_challenge(challenge);
        if reauth {
            req = req.add_prompt(CoreAuthPrompt::Login);
        }
        for a in acr {
            req = req.add_auth_context_value(AuthenticationContextClass::new(a.clone()));
        }
        let (url, _, _) = req.url();
        url
    }

    /// Exchanges the code and verifies the returned id_token.
    pub async fn exchange(
        &self,
        code: &str,
        verifier: &str,
        nonce: &str,
    ) -> Result<Identity, CallbackFailure> {
        let p = self.provider().await;
        let req = p
            .client
            .exchange_code(AuthorizationCode::new(code.into()))
            .map_err(|_| CallbackFailure::ExchangeFailed)?
            .set_pkce_verifier(PkceCodeVerifier::new(verifier.into()));
        let resp = req.request_async(&self.http).await.map_err(|e| {
            // The error type may echo the IdP's error body but never our secret.
            tracing::warn!(error = %e, "token exchange failed");
            CallbackFailure::ExchangeFailed
        })?;
        let token = resp.id_token().ok_or(CallbackFailure::TokenInvalid)?;
        let mut id = self.verify(token, Some(nonce)).await?;
        id.refresh_token = resp.refresh_token().map(|r| r.secret().clone());
        Ok(id)
    }

    /// Keepalive revalidation: refresh grant, then verify the refreshed
    /// id_token (same iss/aud/azp/alg/exp rules, any nonce) and require the
    /// same subject. Roles are returned unfiltered for the role gate.
    // COMPLIANCE MAPPING:
    // - NIST 800-53: AC-2 (Account Management), AC-2(3) — disabled/de-roled users lose the session at the next keepalive
    // COMPLIANCE MAPPING:
    // - NIST 800-53: AU-12 (Audit Record Generation) — PERF-007 span `oidc.refresh_grant{duration_ms, outcome}`
    pub async fn refresh_session(
        &self,
        refresh_token: &str,
        expected_sub: &str,
    ) -> Result<Identity, RefreshFailure> {
        let span = tracing::info_span!(
            "oidc.refresh_grant",
            duration_ms = tracing::field::Empty,
            outcome = tracing::field::Empty
        );
        let t0 = Instant::now();
        let res = self
            .refresh_inner(refresh_token, expected_sub)
            .instrument(span.clone())
            .await;
        span.record(
            "duration_ms",
            u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX),
        );
        span.record(
            "outcome",
            match &res {
                Ok(_) => "ok",
                Err(RefreshFailure::Rejected(_)) => "rejected",
                Err(RefreshFailure::Unavailable) => "unavailable",
            },
        );
        span.in_scope(|| tracing::info!("refresh grant finished"));
        res
    }

    async fn refresh_inner(
        &self,
        refresh_token: &str,
        expected_sub: &str,
    ) -> Result<Identity, RefreshFailure> {
        let p = self.provider().await;
        let rt = RefreshToken::new(refresh_token.to_owned());
        let req = p
            .client
            .exchange_refresh_token(&rt)
            .map_err(|_| RefreshFailure::Unavailable)?;
        let resp = match req.request_async(&self.refresh_http).await {
            Ok(r) => r,
            Err(RequestTokenError::ServerResponse(e)) => {
                tracing::warn!(error = %e.error(), "refresh grant rejected by IdP");
                return Err(RefreshFailure::Rejected("idp_rejected"));
            }
            Err(e) => {
                tracing::warn!(error = %e, "refresh grant failed");
                return Err(RefreshFailure::Unavailable);
            }
        };
        let token = resp
            .id_token()
            .ok_or(RefreshFailure::Rejected("token_invalid"))?;
        let mut id = self
            .verify(token, None)
            .await
            .map_err(|_| RefreshFailure::Rejected("token_invalid"))?;
        if !bool::from(id.sub.as_bytes().ct_eq(expected_sub.as_bytes())) {
            tracing::warn!("refreshed id_token subject differs from the session");
            return Err(RefreshFailure::Rejected("subject_mismatch"));
        }
        id.refresh_token = resp.refresh_token().map(|r| r.secret().clone());
        Ok(id)
    }

    /// Verifies signature (alg allow-list), iss, aud, azp, exp, iat skew and
    /// nonce. Refreshes JWKS once on an unknown key id (rate-limited).
    // COMPLIANCE MAPPING:
    // - NIST 800-53: IA-2 (Identification and Authentication) — identity accepted only from a verified id_token
    // - NIST 800-53: SC-13 (Cryptographic Protection) — vetted JOSE verification, pinned algorithms
    /// `nonce: None` only for refreshed tokens (see [`nonce_check`]).
    pub async fn verify(
        &self,
        token: &KcIdToken,
        nonce: Option<&str>,
    ) -> Result<Identity, CallbackFailure> {
        let p = self.provider().await;
        match self.verify_with(&p, token, nonce) {
            Err(Some(openidconnect::SignatureVerificationError::NoMatchingKey))
                if p.fetched_at.elapsed() >= JWKS_REFRESH_MIN =>
            {
                // `refresh_provider` re-checks `fetched_at` under the write
                // lock, so concurrent callers trigger one fetch (PERF-006).
                let p = self
                    .refresh_provider()
                    .await
                    .ok_or(CallbackFailure::TokenInvalid)?;
                self.verify_with(&p, token, nonce)
                    .map_err(|_| CallbackFailure::TokenInvalid)
            }
            other => other.map_err(|_| CallbackFailure::TokenInvalid),
        }
    }

    /// `Err(Some(sig))` for signature errors (to decide on JWKS refresh).
    fn verify_with(
        &self,
        p: &Provider,
        token: &KcIdToken,
        nonce: Option<&str>,
    ) -> Result<Identity, Option<openidconnect::SignatureVerificationError>> {
        let clock = self.clock.clone();
        let iat_clock = self.clock.clone();
        let verifier = p
            .client
            .id_token_verifier()
            .set_allowed_algs(ALLOWED_ALGS)
            // Extra audiences are allowed only with azp == client_id (checked below).
            .set_other_audience_verifier_fn(|_| true)
            .set_time_fn(move || clock.now())
            .set_issue_time_verifier_fn(move |iat: DateTime<Utc>| {
                if iat > iat_clock.now() + IAT_SKEW {
                    Err("iat in the future".into())
                } else {
                    Ok(())
                }
            });
        let claims = token.claims(&verifier, nonce_check(nonce)).map_err(|e| {
            tracing::warn!(error = %e, "id_token rejected");
            match e {
                openidconnect::ClaimsVerificationError::SignatureVerification(s) => Some(s),
                _ => None,
            }
        })?;
        let azp = claims.authorized_party();
        let azp_ok = match azp {
            Some(a) => a == &self.client_id,
            None => claims.audiences().len() == 1,
        };
        if !azp_ok {
            tracing::warn!("id_token rejected: azp mismatch");
            return Err(None);
        }
        let sub = claims.subject().as_str().to_owned();
        let username = claims
            .preferred_username()
            .map(|u| u.as_str().to_owned())
            .unwrap_or_else(|| sub.clone());
        let display_name = claims
            .name()
            .and_then(|n| n.get(None))
            .map(|n| n.as_str().to_owned())
            .unwrap_or_else(|| username.clone());
        let roles = claims
            .additional_claims()
            .realm_access
            .as_ref()
            .map(|r| r.roles.clone())
            .unwrap_or_default();
        Ok(Identity {
            username: truncate(&username, MAX_NAME_LEN),
            display_name: truncate(&display_name, MAX_NAME_LEN),
            sub: truncate(&sub, MAX_NAME_LEN),
            roles,
            id_token: token.to_string(),
            refresh_token: None,
            auth_time: claims.auth_time(),
            acr: claims
                .auth_context_ref()
                .map(|a| truncate(a.as_str(), MAX_ROLE_LEN)),
        })
    }

    async fn refresh_provider(&self) -> Option<Arc<Provider>> {
        let mut guard = self.provider.write().await;
        if guard.fetched_at.elapsed() < JWKS_REFRESH_MIN {
            return Some(guard.clone());
        }
        let meta = ProviderMetadataWithLogout::discover_async(self.issuer.clone(), &self.http)
            .await
            .map_err(|e| tracing::warn!(error = %e, "JWKS refresh failed"))
            .ok()?;
        let p = build_provider(meta, &self.client_id, &self.client_secret, &self.redirect).ok()?;
        *guard = Arc::new(p);
        Some(guard.clone())
    }

    /// Origin of the end-session endpoint (`LogoutResponse.end_session_origin`).
    pub async fn end_session_origin(&self) -> Option<String> {
        let p = self.provider().await;
        p.end_session
            .as_ref()
            .map(|u| u.origin().ascii_serialization())
            .filter(|o| o != "null")
    }

    /// RP-initiated logout URL. `id_token_hint` (stored server-side per the
    /// M3 decision) lets Keycloak skip its confirmation page. Without a hint
    /// (403 page "Sign out of single sign-on") Keycloak asks to confirm.
    ///
    /// SEC-003 (accepted, documented): the hint travels in the query string,
    /// so it can reach browser history and IdP/ingress access logs. Mitigations:
    /// `Referrer-Policy: no-referrer`, the shell uses `location.replace`, the
    /// console never accepts id_tokens as bearer credentials, and the id_token
    /// lifespan should be short (≤ 5 min) in the icam client. Operators must
    /// drop the query string from end-session access logs. POST logout to the
    /// end-session endpoint (needs a form auto-submit page) is a POA&M item.
    pub async fn end_session_url(&self, id_token_hint: Option<&str>) -> String {
        let p = self.provider().await;
        let Some(mut url) = p.end_session.clone() else {
            return self.post_logout.clone();
        };
        {
            let mut q = url.query_pairs_mut();
            if let Some(hint) = id_token_hint {
                q.append_pair("id_token_hint", hint);
            }
            q.append_pair("post_logout_redirect_uri", &self.post_logout);
            q.append_pair("client_id", self.client_id.as_str());
        }
        url.into()
    }
}

fn build_provider(
    meta: ProviderMetadataWithLogout,
    client_id: &ClientId,
    secret: &ClientSecret,
    redirect: &RedirectUrl,
) -> Result<Provider, OidcError> {
    let end_session = meta
        .additional_metadata()
        .end_session_endpoint
        .as_ref()
        .map(|u| u.url().clone());
    if meta.token_endpoint().is_none() {
        return Err(OidcError::Config("provider has no token endpoint"));
    }
    let client = KcClient::from_provider_metadata(meta, client_id.clone(), Some(secret.clone()))
        .set_redirect_uri(redirect.clone());
    Ok(Provider {
        client,
        end_session,
        fetched_at: Instant::now(),
    })
}

/// Required-role gate on the unfiltered roles.
pub fn has_role(roles: &[String], required: &str) -> bool {
    roles.iter().any(|r| r == required)
}

// COMPLIANCE MAPPING:
// - NIST 800-53: SI-10 (Information Input Validation) — only bounded `console-*` roles stored/echoed
pub fn filter_roles(roles: &[String]) -> Vec<String> {
    filter_roles_with(roles, |_| false)
}

/// [`filter_roles`] that also keeps roles `extra` accepts (M4: roles a
/// registry tool requires or forwards, e.g. `hello:user`).
// COMPLIANCE MAPPING:
// - NIST 800-53: AC-6 (Least Privilege) — only roles some registry entry uses are stored
pub fn filter_roles_with(roles: &[String], extra: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = roles
        .iter()
        .filter(|r| (r.starts_with("console-") || extra(r)) && r.len() <= MAX_ROLE_LEN)
        .take(MAX_ROLES)
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}
