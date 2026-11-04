//! Shared test kit: manual clock, test config, a test signing key, and an
//! in-process stub OIDC provider (discovery, JWKS, token endpoint).
#![allow(dead_code)] // each test binary uses a different subset

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, TimeZone, Utc};
use console_gateway::config::Config;
use console_gateway::oidc::{KcClaims, KcIdToken, RealmAccess};
use console_gateway::session::Clock;
use openidconnect::core::{CoreGenderClaim, CoreJwsSigningAlgorithm, CoreRsaPrivateSigningKey};
use openidconnect::{
    Audience, AuthenticationContextClass, ClientId, EndUserName, EndUserUsername, IdTokenClaims,
    IssuerUrl, JsonWebKeyId, JsonWebKeySet, LocalizedClaim, Nonce, PrivateSigningKey,
    ProviderMetadataWithLogout, StandardClaims, SubjectIdentifier,
};

pub const CLIENT_ID: &str = "usg-ops-console";
pub const CLIENT_SECRET: &str = "test-only-client-secret";
pub const PUBLIC_URL: &str = "http://localhost:8080";
pub const KID: &str = "test-kid";
pub const ACR: &str = "urn:test:cac";

pub struct ManualClock(pub Mutex<DateTime<Utc>>);

impl ManualClock {
    pub fn at(t: DateTime<Utc>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(t)))
    }
    pub fn advance(&self, secs: i64) {
        let mut t = self.0.lock().unwrap();
        *t += Duration::seconds(secs);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

pub fn t0() -> DateTime<Utc> {
    Utc.timestamp_opt(1_800_000_000, 0).unwrap()
}

pub fn config(issuer: &str, extra: &[(&str, &str)]) -> Config {
    let mut env: HashMap<String, String> = [
        ("CONSOLE_PUBLIC_URL", PUBLIC_URL),
        ("CONSOLE_OIDC_ISSUER", issuer),
        ("CONSOLE_OIDC_CLIENT_ID", CLIENT_ID),
        ("CONSOLE_OIDC_CLIENT_SECRET", CLIENT_SECRET),
        ("CONSOLE_DATABASE_URL", "postgres://unused@localhost/unused"),
        (
            "CONSOLE_AUDIT_DATABASE_URL",
            "postgres://unused@localhost/unused",
        ),
        ("CONSOLE_DIST", "/nonexistent"),
        ("CONSOLE_REQUIRED_ACR", ACR),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    for (k, v) in extra {
        env.insert((*k).to_owned(), (*v).to_owned());
    }
    Config::from_lookup(|k| env.get(k).cloned(), |_| None).unwrap()
}

pub fn key() -> CoreRsaPrivateSigningKey {
    CoreRsaPrivateSigningKey::from_pem(
        include_str!("../fixtures/test-only-rsa.pem"),
        Some(JsonWebKeyId::new(KID.into())),
    )
    .unwrap()
}

pub fn other_key() -> CoreRsaPrivateSigningKey {
    CoreRsaPrivateSigningKey::from_pem(
        include_str!("../fixtures/test-only-rsa-other.pem"),
        Some(JsonWebKeyId::new(KID.into())),
    )
    .unwrap()
}

pub fn metadata_json(issuer: &str) -> serde_json::Value {
    serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
        "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
        "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
        "end_session_endpoint": format!("{issuer}/protocol/openid-connect/logout"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
    })
}

pub fn metadata(issuer: &str) -> ProviderMetadataWithLogout {
    let meta: ProviderMetadataWithLogout = serde_json::from_value(metadata_json(issuer)).unwrap();
    meta.set_jwks(JsonWebKeySet::new(vec![key().as_verification_key()]))
}

/// Knobs for a signed id_token.
#[derive(Clone)]
pub struct TokenSpec {
    pub issuer: String,
    pub audiences: Vec<String>,
    pub azp: Option<String>,
    pub nonce: Option<String>,
    pub iat: DateTime<Utc>,
    pub exp: DateTime<Utc>,
    pub sub: String,
    pub roles: Vec<String>,
    pub acr: Option<String>,
    pub auth_time: Option<DateTime<Utc>>,
}

impl TokenSpec {
    pub fn valid(issuer: &str, nonce: &str, now: DateTime<Utc>) -> Self {
        Self {
            issuer: issuer.into(),
            audiences: vec![CLIENT_ID.into()],
            azp: Some(CLIENT_ID.into()),
            nonce: Some(nonce.into()),
            iat: now,
            exp: now + Duration::minutes(5),
            sub: "sub-123".into(),
            roles: vec!["console-user".into(), "offline_access".into()],
            acr: Some(ACR.into()),
            auth_time: Some(now),
        }
    }

    pub fn sign_with(&self, key: &CoreRsaPrivateSigningKey) -> String {
        let mut std_claims =
            StandardClaims::<CoreGenderClaim>::new(SubjectIdentifier::new(self.sub.clone()))
                .set_preferred_username(Some(EndUserUsername::new("jdoe".into())));
        std_claims = std_claims.set_name(Some(LocalizedClaim::from(EndUserName::new(
            "Jane Doe".into(),
        ))));
        let claims = IdTokenClaims::new(
            IssuerUrl::new(self.issuer.clone()).unwrap(),
            self.audiences.iter().cloned().map(Audience::new).collect(),
            self.exp,
            self.iat,
            std_claims,
            KcClaims {
                realm_access: Some(RealmAccess {
                    roles: self.roles.clone(),
                }),
            },
        )
        .set_nonce(self.nonce.clone().map(Nonce::new))
        .set_authorized_party(self.azp.clone().map(ClientId::new))
        .set_auth_context_ref(self.acr.clone().map(AuthenticationContextClass::new))
        .set_auth_time(self.auth_time);
        let tok: KcIdToken = KcIdToken::new(
            claims,
            key,
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            None,
            None,
        )
        .unwrap();
        tok.to_string()
    }

    pub fn sign(&self) -> String {
        self.sign_with(&key())
    }
}

/// How the stub answers a `grant_type=refresh_token` request.
#[derive(Clone, Default)]
pub enum RefreshMode {
    /// Re-sign `next` (with `iat` = `now` if set; roles as currently in `next`).
    #[default]
    Ok,
    /// `400 invalid_grant` (user disabled / SSO session revoked).
    Reject,
    /// `502` with a non-JSON body: a transport/parse failure, i.e. the
    /// "IdP unavailable" path (not a rejection).
    Broken,
}

/// Stub IdP state: the next token response is built from `next`.
#[derive(Clone, Default)]
pub struct StubIdp {
    pub issuer: String,
    pub next: Arc<Mutex<Option<TokenSpec>>>,
    pub token_requests: Arc<Mutex<Vec<String>>>,
    pub refresh: Arc<Mutex<RefreshMode>>,
    /// Test clock; refreshed id_tokens are stamped `iat = now`, like a real IdP.
    pub clock: Arc<Mutex<Option<Arc<ManualClock>>>>,
}

/// Starts the stub on an ephemeral 127.0.0.1 port; returns it with its issuer.
pub async fn start_stub_idp() -> StubIdp {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let issuer = format!("http://127.0.0.1:{}/realms/icam", addr.port());
    let stub = StubIdp {
        issuer: issuer.clone(),
        ..Default::default()
    };
    let app = Router::new()
        .route(
            "/realms/icam/.well-known/openid-configuration",
            get(discovery),
        )
        .route("/realms/icam/protocol/openid-connect/certs", get(jwks))
        .route("/realms/icam/protocol/openid-connect/token", post(token))
        .with_state(stub.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    stub
}

async fn discovery(State(s): State<StubIdp>) -> Json<serde_json::Value> {
    Json(metadata_json(&s.issuer))
}

async fn jwks() -> Json<serde_json::Value> {
    let set = JsonWebKeySet::new(vec![key().as_verification_key()]);
    Json(serde_json::to_value(set).unwrap())
}

async fn token(State(s): State<StubIdp>, headers: HeaderMap, body: String) -> Response {
    s.token_requests.lock().unwrap().push(body.clone());
    let basic_ok = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("Basic "));
    if basic_ok && body.contains("grant_type=refresh_token") {
        let mode = s.refresh.lock().unwrap().clone();
        let spec = s.next.lock().unwrap().clone();
        return match (mode, spec) {
            (RefreshMode::Ok, Some(mut spec)) if body.contains("refresh_token=stub-refresh-") => {
                spec.nonce = None;
                if let Some(c) = s.clock.lock().unwrap().as_ref() {
                    spec.iat = c.now();
                    spec.exp = spec.iat + Duration::minutes(5);
                }
                Json(serde_json::json!({
                    "access_token": "stub-access-token-2",
                    "token_type": "Bearer",
                    "expires_in": 300,
                    "refresh_token": "stub-refresh-2",
                    "id_token": spec.sign(),
                }))
                .into_response()
            }
            (RefreshMode::Broken, _) => {
                (StatusCode::BAD_GATEWAY, "upstream exploded").into_response()
            }
            _ => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_grant"})),
            )
                .into_response(),
        };
    }
    if !basic_ok || !body.contains("code_verifier=") || !body.contains("code=good-code") {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_grant"})),
        )
            .into_response();
    }
    let Some(spec) = s.next.lock().unwrap().clone() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_grant"})),
        )
            .into_response();
    };
    Json(serde_json::json!({
        "access_token": "stub-access-token",
        "token_type": "Bearer",
        "expires_in": 300,
        "refresh_token": "stub-refresh-1",
        "id_token": spec.sign(),
    }))
    .into_response()
}
