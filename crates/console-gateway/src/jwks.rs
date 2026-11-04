//! `GET /.well-known/jwks.json` (console origin, public): the assertion
//! verification keys (active + rotation keys from
//! `CONSOLE_ASSERTION_PUBLISH_KEYS_FILE`). Never contains private material.

use axum::extract::State;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};

use crate::routes::Shared;

const EMPTY: &str = r#"{"keys":[]}"#;

// COMPLIANCE MAPPING:
// - NIST 800-53: SC-12 (Cryptographic Key Establishment and Management), IA-5(2) — public-key distribution for assertion verification
pub async fn jwks(State(s): State<Shared>) -> Response {
    let body = s
        .tools
        .keys()
        .map(|k| k.jwks_json().to_owned())
        .unwrap_or_else(|| EMPTY.to_owned());
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/jwk-set+json"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=300"),
            ),
        ],
        body,
    )
        .into_response()
}
