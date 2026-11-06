//! console-sdk against the shared protocol v1 corpus
//! (`console-protocol/tests/fixtures/MANIFEST.json`, format in the README
//! there). Same corpus as `console-protocol` and `sdk-js`.
#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::path::PathBuf;

use console_protocol::MessageError;
use console_sdk::{SdkError, ToolToShell, decode, encode};
use serde_json::Value;

const CONSOLE: &str = "https://console.ops.example.mil";

fn path(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../console-protocol/tests/fixtures")
        .join(file)
}

fn message(file: &str) -> String {
    fs::read_to_string(path(file))
        .unwrap()
        .trim_end()
        .to_owned()
}

fn entries() -> Vec<Value> {
    let m: Value = serde_json::from_str(&message("MANIFEST.json")).unwrap();
    assert_eq!(m["format_version"], 1);
    assert_eq!(m["protocol"], console_sdk::PROTOCOL_VERSION);
    m["fixtures"].as_array().unwrap().clone()
}

fn sdk_entries(direction: &str) -> Vec<Value> {
    entries()
        .into_iter()
        .filter(|e| e["kind"] == "sdk-message" && e["direction"] == direction)
        .collect()
}

fn want(e: &Value) -> Value {
    let f = e["canonical"].as_str().or(e["file"].as_str()).unwrap();
    serde_json::from_str(&message(f)).unwrap()
}

/// Tool side receives shell-to-tool messages through `decode`.
#[test]
fn decode_follows_manifest() {
    let all = sdk_entries("shell-to-tool");
    assert!(!all.is_empty());
    for e in all {
        let file = e["file"].as_str().unwrap();
        let data = message(file);
        match (
            e["expect"].as_str().unwrap(),
            decode(CONSOLE, CONSOLE, &data),
        ) {
            ("decode", Ok(Some(m))) => {
                let v: Value = serde_json::from_str(&m.to_json().unwrap()).unwrap();
                assert_eq!(v["type"], e["type"], "{file}");
                assert_eq!(v, want(&e), "{file}");
            }
            ("ignore", Ok(None)) => {}
            ("malformed", Err(SdkError::Message(MessageError::Malformed(_)))) => {}
            ("too_large", Err(SdkError::Message(MessageError::TooLarge(_)))) => {}
            (w, got) => panic!("{file}: expected {w}, got {got:?}"),
        }
        // The origin pin applies before anything else, valid or not.
        assert!(
            matches!(
                decode("https://evil.example", CONSOLE, &data),
                Err(SdkError::UntrustedOrigin(_))
            ),
            "{file}"
        );
    }
}

/// Tool side sends tool-to-shell messages through `encode`; every decodable
/// fixture must encode back to its canonical JSON.
#[test]
fn encode_matches_canonical_json() {
    let all: Vec<Value> = sdk_entries("tool-to-shell")
        .into_iter()
        .filter(|e| e["expect"] == "decode")
        .collect();
    assert!(!all.is_empty());
    for e in all {
        let file = e["file"].as_str().unwrap();
        let m = ToolToShell::from_json(&message(file)).unwrap().unwrap();
        let v: Value = serde_json::from_str(&encode(&m).unwrap()).unwrap();
        assert_eq!(v, want(&e), "{file}");
    }
}
