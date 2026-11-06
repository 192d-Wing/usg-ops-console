//! Gateway identity assertion: an ES256 compact JWS (`X-Console-Assertion`)
//! minted per proxied request, the JWKS that publishes its public keys, and
//! the per-tool session pseudonym (`sid`). Claims are
//! `console_protocol::AssertionClaims` (plan.md section 4.3).
//!
//! Keys load at start-up only (rotation = restart; see the runbook in
//! DEPLOYMENT.md). The private key never leaves this module, and `Debug`
//! output is redacted. Signing uses ring (not jsonwebtoken's RustCrypto
//! backend, whose crypto-bigint 0.5 is not constant-time: RUSTSEC-2026-0387);
//! tests verify independently with jsonwebtoken, as tools do.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use console_protocol::AssertionClaims;
use console_protocol::tools::{ASSERTION_ALG, ASSERTION_TYP};
use ring::hmac;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::session::{RngError, random_bytes};

/// Domain separation for the derived `sid` key (HKDF-extract style:
/// HMAC(salt, PKCS#8 key)) when no dedicated key file is configured.
const SID_KEY_SALT: &[u8] = b"usg-ops-console/tool-sid/v1";
/// Max keys published (active + rotation).
const MAX_PUBLISHED: usize = 8;
/// DER prefix of a P-256 `SubjectPublicKeyInfo` (id-ecPublicKey,
/// prime256v1, 66-byte BIT STRING); the uncompressed point follows.
const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KeyError {
    #[error("CONSOLE_ASSERTION_KEY_FILE: not a PKCS#8 PEM P-256 private key")]
    PrivateKey,
    #[error("CONSOLE_ASSERTION_PUBLISH_KEYS_FILE: block {0} is not a P-256 PUBLIC KEY")]
    PublishKey(usize),
    #[error("CONSOLE_ASSERTION_PUBLISH_KEYS_FILE: at most {MAX_PUBLISHED} keys")]
    TooManyKeys,
    #[error("assertion signing failed")]
    Sign,
}

/// One JWK (RFC 7517) for a P-256 verification key.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Jwk {
    pub kty: &'static str,
    pub crv: &'static str,
    pub x: String,
    pub y: String,
    pub kid: String,
    #[serde(rename = "use")]
    pub use_: &'static str,
    pub alg: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Jwks {
    pub keys: Vec<Jwk>,
}

/// JWK of an uncompressed P-256 point (`04 || x || y`); `kid` is its
/// RFC 7638 thumbprint. `None` if `point` is not 65 bytes starting 0x04.
pub fn jwk(point: &[u8]) -> Option<Jwk> {
    if point.len() != 65 || point[0] != 0x04 {
        return None;
    }
    let x = B64URL.encode(&point[1..33]);
    let y = B64URL.encode(&point[33..65]);
    // RFC 7638: required members in lexicographic order, no whitespace.
    let canonical = format!(r#"{{"crv":"P-256","kty":"EC","x":"{x}","y":"{y}"}}"#);
    let kid = B64URL.encode(Sha256::digest(canonical.as_bytes()));
    Some(Jwk {
        kty: "EC",
        crv: "P-256",
        x,
        y,
        kid,
        use_: "sig",
        alg: ASSERTION_ALG,
    })
}

/// PEM blocks as (label, DER). Malformed base64 yields an empty DER, which
/// the caller rejects.
fn pem_blocks(text: &str) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut cur: Option<(String, String)> = None;
    for line in text.lines().map(str::trim) {
        if let Some(label) = line
            .strip_prefix("-----BEGIN ")
            .and_then(|l| l.strip_suffix("-----"))
        {
            cur = Some((label.to_owned(), String::new()));
        } else if line.starts_with("-----END ") {
            if let Some((label, b64)) = cur.take() {
                out.push((label, B64.decode(b64.as_bytes()).unwrap_or_default()));
            }
        } else if let Some((_, b64)) = cur.as_mut() {
            b64.push_str(line);
        }
    }
    out
}

pub struct AssertionKeys {
    key_pair: EcdsaKeyPair,
    rng: SystemRandom,
    kid: String,
    jwks: Jwks,
    jwks_json: String,
    sid_key: hmac::Key,
}

impl fmt::Debug for AssertionKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssertionKeys")
            .field("kid", &self.kid)
            .field("published", &self.jwks.keys.len())
            .field("private", &"<redacted>")
            .finish()
    }
}

// COMPLIANCE MAPPING:
// - NIST 800-53: SC-12 (Cryptographic Key Establishment and Management) — key from file, kid thumbprint, published-key rotation;
//   sid HMAC key from its own file (required outside dev mode by ToolsState::from_config; derived only in dev mode)
// - NIST 800-53: SC-13 (Cryptographic Protection) — ES256 via ring (constant-time P-256), the same backend as TLS
impl AssertionKeys {
    /// `private_pem`: PKCS#8 P-256 key (`-----BEGIN PRIVATE KEY-----`, as
    /// `openssl pkcs8 -topk8 -nocrypt` writes it). `publish_pem`: extra
    /// SPKI public keys to publish (next/previous key during rotation).
    /// `sid_key`: dedicated HMAC key (`CONSOLE_TOOL_SESSION_HMAC_KEY_FILE`).
    /// `None` derives it from the private key; `ToolsState::from_config`
    /// allows that only in dev mode (SEC-005). Rotating the sid key changes
    /// every tool's `sid` for live sessions (tools see a new pseudonym).
    pub fn from_pem(
        private_pem: &str,
        publish_pem: Option<&str>,
        sid_key: Option<&[u8]>,
    ) -> Result<Self, KeyError> {
        let der = pem_blocks(private_pem)
            .into_iter()
            .find(|(label, _)| label == "PRIVATE KEY")
            .map(|(_, der)| der)
            .ok_or(KeyError::PrivateKey)?;
        let rng = SystemRandom::new();
        let key_pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &rng)
            .map_err(|_| KeyError::PrivateKey)?;
        let active = jwk(key_pair.public_key().as_ref()).ok_or(KeyError::PrivateKey)?;
        let kid = active.kid.clone();
        let mut keys = vec![active];
        for (i, (label, der)) in pem_blocks(publish_pem.unwrap_or_default())
            .into_iter()
            .enumerate()
        {
            let point = der
                .strip_prefix(&P256_SPKI_PREFIX[..])
                .filter(|_| label == "PUBLIC KEY");
            let j = point.and_then(jwk).ok_or(KeyError::PublishKey(i))?;
            if !keys.iter().any(|k| k.kid == j.kid) {
                keys.push(j);
            }
        }
        if keys.len() > MAX_PUBLISHED {
            return Err(KeyError::TooManyKeys);
        }
        let sid_key = match sid_key {
            Some(k) => hmac::Key::new(hmac::HMAC_SHA256, k),
            None => {
                let salt = hmac::Key::new(hmac::HMAC_SHA256, SID_KEY_SALT);
                let prk = hmac::sign(&salt, &der);
                hmac::Key::new(hmac::HMAC_SHA256, prk.as_ref())
            }
        };
        let jwks = Jwks { keys };
        let jwks_json = serde_json::to_string(&jwks).map_err(|_| KeyError::Sign)?;
        Ok(Self {
            key_pair,
            rng,
            kid,
            jwks,
            jwks_json,
            sid_key,
        })
    }

    pub fn kid(&self) -> &str {
        &self.kid
    }

    pub fn jwks(&self) -> &Jwks {
        &self.jwks
    }

    /// Serialized JWKS (computed once at start-up).
    pub fn jwks_json(&self) -> &str {
        &self.jwks_json
    }

    /// Compact JWS (RFC 7515) with `{"alg":"ES256","typ":"console-assertion+jwt","kid":…}`;
    /// the signature is the fixed-width `r || s` that ES256 requires.
    pub fn sign(&self, claims: &AssertionClaims) -> Result<String, KeyError> {
        #[derive(Serialize)]
        struct JoseHeader<'a> {
            alg: &'static str,
            typ: &'static str,
            kid: &'a str,
        }
        let header = serde_json::to_vec(&JoseHeader {
            alg: ASSERTION_ALG,
            typ: ASSERTION_TYP,
            kid: &self.kid,
        })
        .map_err(|_| KeyError::Sign)?;
        let payload = serde_json::to_vec(claims).map_err(|_| KeyError::Sign)?;
        let signing_input = format!("{}.{}", B64URL.encode(header), B64URL.encode(payload));
        let sig = self
            .key_pair
            .sign(&self.rng, signing_input.as_bytes())
            .map_err(|_| KeyError::Sign)?;
        Ok(format!("{signing_input}.{}", B64URL.encode(sig.as_ref())))
    }

    /// Per-session, per-tool pseudonym: HMAC-SHA256(key, len(hash) ‖ hash ‖
    /// tool id), first 16 bytes, b64url. Stable for one console session at
    /// one tool. It cannot be linked across tools or to the console
    /// `session_ref` without the key (brief Q5).
    // COMPLIANCE MAPPING:
    // - NIST 800-53: AC-4 (Information Flow Enforcement), SC-39 — no cross-tool correlation handle (keyed by the
    //   dedicated sid key outside dev mode)
    pub fn sid(&self, session_hash: &[u8], tool_id: &str) -> String {
        let mut ctx = hmac::Context::with_key(&self.sid_key);
        ctx.update(
            &u32::try_from(session_hash.len())
                .unwrap_or(u32::MAX)
                .to_be_bytes(),
        );
        ctx.update(session_hash);
        ctx.update(tool_id.as_bytes());
        let out = ctx.sign();
        B64URL.encode(&out.as_ref()[..16])
    }
}

/// 128-bit random `jti`.
pub fn new_jti() -> Result<String, RngError> {
    random_bytes::<16>().map(|b| B64URL.encode(b))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use console_protocol::Scope;
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};

    /// Test-only P-256 keys (generated for this repository's tests; never
    /// used anywhere else).
    pub const TEST_KEY: &str = include_str!("../tests/fixtures/test-only-p256.pem");
    const OTHER_KEY: &str = include_str!("../tests/fixtures/test-only-p256-other.pem");
    const OTHER_PUB: &str = include_str!("../tests/fixtures/test-only-p256-other.pub.pem");

    fn claims(now: u64) -> AssertionClaims {
        AssertionClaims {
            iss: "https://console.ops.example.mil".into(),
            aud: "hello".into(),
            sub: "8f1c".into(),
            preferred_username: "jdoe".into(),
            name: "Jane Doe".into(),
            roles: vec!["hello:user".into()],
            ctx: Scope::default(),
            sid: "x".into(),
            jti: new_jti().unwrap(),
            iat: now,
            nbf: now,
            exp: now + 60,
            acr: Some("urn:test:cac".into()),
        }
    }

    #[test]
    fn signs_es256_with_kid_and_verifies_via_jwk() {
        let keys = AssertionKeys::from_pem(TEST_KEY, None, None).unwrap();
        let now = jsonwebtoken::get_current_timestamp();
        let token = keys.sign(&claims(now)).unwrap();
        let h = decode_header(&token).unwrap();
        assert_eq!(h.alg, Algorithm::ES256);
        assert_eq!(h.typ.as_deref(), Some(ASSERTION_TYP));
        assert_eq!(h.kid.as_deref(), Some(keys.kid()));
        let jwks: serde_json::Value = serde_json::from_str(keys.jwks_json()).unwrap();
        let jwk: jsonwebtoken::jwk::Jwk = serde_json::from_value(jwks["keys"][0].clone()).unwrap();
        let mut v = Validation::new(Algorithm::ES256);
        v.set_audience(&["hello"]);
        v.set_issuer(&["https://console.ops.example.mil"]);
        let got = decode::<AssertionClaims>(&token, &DecodingKey::from_jwk(&jwk).unwrap(), &v)
            .unwrap()
            .claims;
        assert_eq!(got.sub, "8f1c");
        // A different key does not verify it.
        let other = AssertionKeys::from_pem(OTHER_KEY, None, None).unwrap();
        let ojwk: jsonwebtoken::jwk::Jwk =
            serde_json::from_str(&serde_json::to_string(&other.jwks().keys[0]).unwrap()).unwrap();
        assert!(
            decode::<AssertionClaims>(&token, &DecodingKey::from_jwk(&ojwk).unwrap(), &v).is_err()
        );
    }

    /// Wire format tools (otter-go) verify: `jti` is 22-char b64url (128
    /// bits), unique per mint; `acr` is the session's id_token acr and is
    /// omitted (not null) when the session has none.
    #[test]
    fn jti_unique_and_acr_wire_format() {
        let keys = AssertionKeys::from_pem(TEST_KEY, None, None).unwrap();
        let now = jsonwebtoken::get_current_timestamp();
        let payload = |tok: &str| -> serde_json::Value {
            let p = tok.split('.').nth(1).unwrap();
            serde_json::from_slice(&B64URL.decode(p).unwrap()).unwrap()
        };
        let a = payload(&keys.sign(&claims(now)).unwrap());
        let b = payload(&keys.sign(&claims(now)).unwrap());
        let jti = a["jti"].as_str().unwrap();
        assert_eq!(jti.len(), 22);
        assert!(B64URL.decode(jti).is_ok_and(|d| d.len() == 16));
        assert_ne!(a["jti"], b["jti"]);
        assert_eq!(a["acr"], "urn:test:cac");
        assert_eq!(a["exp"].as_u64().unwrap() - a["iat"].as_u64().unwrap(), 60);
        let mut c = claims(now);
        c.acr = None;
        assert!(payload(&keys.sign(&c).unwrap()).get("acr").is_none());
    }

    #[test]
    fn jwks_publishes_rotation_keys_and_kid_is_thumbprint() {
        let keys = AssertionKeys::from_pem(TEST_KEY, Some(OTHER_PUB), None).unwrap();
        assert_eq!(keys.jwks().keys.len(), 2);
        let other = AssertionKeys::from_pem(OTHER_KEY, None, None).unwrap();
        assert_eq!(keys.jwks().keys[1].kid, other.kid());
        assert_eq!(keys.kid().len(), 43, "b64url sha256");
        let j: serde_json::Value = serde_json::from_str(keys.jwks_json()).unwrap();
        let k = &j["keys"][0];
        assert_eq!(k["kty"], "EC");
        assert_eq!(k["crv"], "P-256");
        assert_eq!(k["use"], "sig");
        assert_eq!(k["alg"], "ES256");
        assert!(k.get("d").is_none(), "never the private scalar");
        // Duplicates of the active key are not published twice.
        let pub_self = AssertionKeys::from_pem(OTHER_KEY, Some(OTHER_PUB), None).unwrap();
        assert_eq!(pub_self.jwks().keys.len(), 1);
    }

    #[test]
    fn rejects_bad_keys() {
        assert_eq!(
            AssertionKeys::from_pem("nope", None, None).unwrap_err(),
            KeyError::PrivateKey
        );
        assert_eq!(
            AssertionKeys::from_pem(TEST_KEY, Some(OTHER_KEY), None).unwrap_err(),
            KeyError::PublishKey(0),
            "a private key in the publish file is refused"
        );
        let dbg = format!(
            "{:?}",
            AssertionKeys::from_pem(TEST_KEY, None, None).unwrap()
        );
        assert!(dbg.contains("redacted") && !dbg.contains("PRIVATE"));
    }

    #[test]
    fn sid_is_stable_per_tool_and_unlinkable_across_tools() {
        let keys = AssertionKeys::from_pem(TEST_KEY, None, None).unwrap();
        let h = [7u8; 32];
        let a = keys.sid(&h, "hello");
        assert_eq!(a, keys.sid(&h, "hello"));
        assert_ne!(a, keys.sid(&h, "evil"));
        assert_ne!(a, keys.sid(&[8u8; 32], "hello"));
        assert_eq!(a.len(), 22, "16 bytes b64url");
        let dedicated = AssertionKeys::from_pem(TEST_KEY, None, Some(&[1u8; 32])).unwrap();
        assert_ne!(a, dedicated.sid(&h, "hello"));
    }
}
