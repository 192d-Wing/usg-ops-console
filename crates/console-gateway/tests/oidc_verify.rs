//! id_token verification against a locally signed token and stub JWKS
//! (no network, no database).

mod common;

use std::str::FromStr;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use chrono::Duration;
use common::*;
use console_gateway::oidc::{CallbackFailure, KcIdToken, OidcClient, filter_roles, has_role};

const ISSUER: &str = "http://localhost:8180/realms/icam";
const NONCE: &str = "n-0123456789";

fn client() -> OidcClient {
    let cfg = config(ISSUER, &[]);
    OidcClient::from_metadata(
        metadata(ISSUER),
        &cfg,
        console_gateway::oidc::http_client().unwrap(),
        ManualClock::at(t0()),
    )
    .unwrap()
}

async fn verify(
    token: &str,
    nonce: &str,
) -> Result<console_gateway::oidc::Identity, CallbackFailure> {
    let tok = KcIdToken::from_str(token).map_err(|_| CallbackFailure::TokenInvalid)?;
    client().verify(&tok, Some(nonce)).await
}

#[tokio::test]
async fn valid_token_yields_identity() {
    let id = verify(&TokenSpec::valid(ISSUER, NONCE, t0()).sign(), NONCE)
        .await
        .unwrap();
    assert_eq!(id.sub, "sub-123");
    assert_eq!(id.username, "jdoe");
    assert_eq!(id.display_name, "Jane Doe");
    assert!(has_role(&id.roles, "console-user"));
    assert_eq!(filter_roles(&id.roles), vec!["console-user".to_owned()]);
    assert!(
        !format!("{id:?}").contains(&id.id_token),
        "Debug redacts the token"
    );
}

#[tokio::test]
async fn rejects_bad_claims() {
    let ok = TokenSpec::valid(ISSUER, NONCE, t0());
    let mut cases: Vec<(&str, TokenSpec)> = Vec::new();
    let mut s = ok.clone();
    s.issuer = "http://localhost:8180/realms/other".into();
    cases.push(("wrong iss", s));
    let mut s = ok.clone();
    s.audiences = vec!["someone-else".into()];
    s.azp = None;
    cases.push(("wrong aud", s));
    let mut s = ok.clone();
    s.audiences = vec![CLIENT_ID.into(), "other".into()];
    s.azp = None;
    cases.push(("multi aud without azp", s));
    let mut s = ok.clone();
    s.audiences = vec![CLIENT_ID.into(), "other".into()];
    s.azp = Some("other".into());
    cases.push(("azp mismatch", s));
    let mut s = ok.clone();
    s.nonce = Some("other-nonce".into());
    cases.push(("wrong nonce", s));
    let mut s = ok.clone();
    s.nonce = None;
    cases.push(("missing nonce", s));
    let mut s = ok.clone();
    s.exp = t0() - Duration::seconds(1);
    s.iat = t0() - Duration::minutes(5);
    cases.push(("expired", s));
    let mut s = ok.clone();
    s.iat = t0() + Duration::seconds(120);
    s.exp = t0() + Duration::minutes(10);
    cases.push(("iat in the future beyond skew", s));
    for (name, spec) in cases {
        assert_eq!(
            verify(&spec.sign(), NONCE).await.unwrap_err(),
            CallbackFailure::TokenInvalid,
            "{name}"
        );
    }
}

#[tokio::test]
async fn accepts_extra_audience_with_matching_azp() {
    let mut s = TokenSpec::valid(ISSUER, NONCE, t0());
    s.audiences = vec![CLIENT_ID.into(), "account".into()];
    assert!(verify(&s.sign(), NONCE).await.is_ok());
}

#[tokio::test]
async fn rejects_wrong_key_and_alg_none() {
    let spec = TokenSpec::valid(ISSUER, NONCE, t0());
    assert!(
        verify(&spec.sign_with(&other_key()), NONCE).await.is_err(),
        "foreign key"
    );

    // Re-encode the valid payload under alg=none / HS256 headers.
    let good = spec.sign();
    let payload = good.split('.').nth(1).unwrap();
    for alg in ["none", "HS256"] {
        let header = B64URL.encode(format!(r#"{{"alg":"{alg}","kid":"{KID}"}}"#));
        let forged = format!("{header}.{payload}.{}", B64URL.encode(b"sig"));
        assert!(verify(&forged, NONCE).await.is_err(), "alg={alg}");
    }
}

#[tokio::test]
async fn end_session_url_carries_hint_and_client() {
    let c = client();
    let url = url::Url::parse(&c.end_session_url(Some("tok.en.x")).await).unwrap();
    assert_eq!(url.path(), "/realms/icam/protocol/openid-connect/logout");
    let q: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    assert!(q.contains(&("id_token_hint".into(), "tok.en.x".into())));
    assert!(q.contains(&("client_id".into(), CLIENT_ID.into())));
    assert!(q.contains(&(
        "post_logout_redirect_uri".into(),
        format!("{PUBLIC_URL}/logged-out")
    )));
    assert_eq!(
        c.end_session_origin().await.as_deref(),
        Some("http://localhost:8180")
    );
}

#[tokio::test]
async fn authorize_url_has_pkce_s256_nonce_state() {
    let c = client();
    let url = c
        .authorize_url("st", "nn", &"v".repeat(86), false, &[])
        .await;
    let q: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["state"], "st");
    assert_eq!(q["nonce"], "nn");
    assert_eq!(q["client_id"], CLIENT_ID);
    assert_eq!(q["redirect_uri"], format!("{PUBLIC_URL}/callback"));
    assert!(q["scope"].contains("openid") && q["scope"].contains("profile"));
    assert!(!q.contains_key("code_verifier"));
    assert!(!q.contains_key("prompt") && !q.contains_key("acr_values"));
}

#[tokio::test]
async fn authorize_url_reauth_and_acr() {
    let c = client();
    let acr = vec!["urn:a".to_owned(), "urn:b".to_owned()];
    let url = c
        .authorize_url("st", "nn", &"v".repeat(86), true, &acr)
        .await;
    let q: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
    assert_eq!(q["prompt"], "login");
    assert_eq!(q["acr_values"], "urn:a urn:b");
}

#[tokio::test]
async fn acr_and_auth_time_are_surfaced() {
    let mut spec = TokenSpec::valid(ISSUER, NONCE, t0());
    spec.acr = Some("urn:test:cac".into());
    spec.auth_time = Some(t0() - Duration::seconds(5));
    let id = verify(&spec.sign(), NONCE).await.unwrap();
    assert_eq!(id.acr.as_deref(), Some("urn:test:cac"));
    assert_eq!(id.auth_time, Some(t0() - Duration::seconds(5)));
    assert!(!format!("{id:?}").contains("eyJ"), "Debug redacts tokens");
    // Refreshed tokens (nonce None) may omit or repeat the nonce.
    let tok = KcIdToken::from_str(&spec.sign()).unwrap();
    assert!(client().verify(&tok, None).await.is_ok());
    spec.nonce = None;
    let tok = KcIdToken::from_str(&spec.sign()).unwrap();
    assert!(client().verify(&tok, None).await.is_ok());
    assert!(client().verify(&tok, Some(NONCE)).await.is_err());
}

#[test]
fn role_filtering_bounds() {
    let mut roles: Vec<String> = (0..100).map(|i| format!("console-r{i:03}")).collect();
    roles.push(format!("console-{}", "x".repeat(200)));
    roles.push("admin".into());
    let f = filter_roles(&roles);
    assert_eq!(f.len(), 64);
    assert!(
        f.iter()
            .all(|r| r.starts_with("console-") && r.len() <= 128)
    );
}
