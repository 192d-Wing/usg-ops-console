//! Frozen protocol v1 fixtures. Never edit an existing fixture: they pin what
//! deployed peers send. Add a new file for a new variant or field.

use std::fs;
use std::path::PathBuf;

use console_protocol::{
    AssertionClaims, LaunchRequest, LaunchResponse, ShellToTool, ToolContext, ToolToShell,
    ToolsResponse,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

fn dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn fixtures(name: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = fs::read_dir(dir(name))
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            let n = p.file_name().unwrap().to_string_lossy().into_owned();
            (n, fs::read_to_string(&p).unwrap())
        })
        .collect();
    v.sort();
    v
}

fn round_trip<T: Serialize + DeserializeOwned>(json: &str) -> T {
    let t: T = serde_json::from_str(json).unwrap();
    let back = serde_json::to_value(&t).unwrap();
    let orig: Value = serde_json::from_str(json).unwrap();
    assert_eq!(back, orig);
    t
}

#[test]
fn every_variant_has_a_fixture() {
    let names: Vec<String> = fixtures("sdk_v1").into_iter().map(|(n, _)| n).collect();
    for ty in [
        "shell.Init.",
        "shell.ScopeChanged.",
        "shell.ThemeChanged.",
        "shell.Navigate.",
        "shell.SessionRefreshed.",
        "shell.Focus.",
        "tool.Ready.",
        "tool.SetTitle.",
        "tool.SetBreadcrumbs.",
        "tool.Notify.",
        "tool.SetBadge.",
        "tool.NavigateConsole.",
        "tool.RequestSessionRefresh.",
        "tool.PathChanged.",
        "tool.FocusShell.",
    ] {
        assert!(names.iter().any(|n| n.starts_with(ty)), "missing {ty}");
    }
}

#[test]
fn sdk_v1_fixtures_round_trip_and_decode() {
    for (name, json) in fixtures("sdk_v1") {
        let json = json.trim();
        if name.starts_with("shell.") {
            let m: ShellToTool = round_trip(json);
            assert_eq!(ShellToTool::from_json(json).unwrap(), Some(m), "{name}");
        } else {
            let m: ToolToShell = round_trip(json);
            assert_eq!(ToolToShell::from_json(json).unwrap(), Some(m), "{name}");
        }
    }
}

#[test]
fn pre_m4_init_parses_with_new_fields_absent() {
    let json = fs::read_to_string(dir("sdk_v1").join("shell.Init.pre-m4.json")).unwrap();
    match ShellToTool::from_json(&json).unwrap() {
        Some(ShellToTool::Init { path, tool_id, .. }) => {
            assert_eq!(path, None);
            assert_eq!(tool_id, None);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn unknown_fields_are_ignored() {
    for (name, json) in fixtures("sdk_v1") {
        let mut v: Value = serde_json::from_str(&json).unwrap();
        v["from_the_future"] = Value::from(42);
        let s = v.to_string();
        if name.starts_with("shell.") {
            assert!(ShellToTool::from_json(&s).unwrap().is_some(), "{name}");
        } else {
            assert!(ToolToShell::from_json(&s).unwrap().is_some(), "{name}");
        }
    }
}

#[test]
fn unknown_types_are_ignored_known_but_malformed_errors() {
    for s in [
        r#"{"type":"FromProtocol2","x":1}"#,
        r#"{"no_type":true}"#,
        r#"{"type":7}"#,
    ] {
        assert_eq!(ShellToTool::from_json(s).unwrap(), None, "{s}");
        assert_eq!(ToolToShell::from_json(s).unwrap(), None, "{s}");
    }
    // A shell message is an unknown type to the tool->shell decoder.
    assert_eq!(ToolToShell::from_json(r#"{"type":"Init"}"#).unwrap(), None);
    assert!(ShellToTool::from_json(r#"{"type":"Navigate"}"#).is_err());
    assert!(ShellToTool::from_json("not json").is_err());
    // Non-object JSON is malformed (INT-003), for both directions.
    for s in ["\"x\"", "1", "null", "[1]", "[]", "\"Init\"", "true"] {
        assert!(
            matches!(
                ShellToTool::from_json(s),
                Err(console_protocol::MessageError::Malformed(_))
            ),
            "{s}"
        );
        assert!(
            matches!(
                ToolToShell::from_json(s),
                Err(console_protocol::MessageError::Malformed(_))
            ),
            "{s}"
        );
    }
}

#[test]
fn oversized_messages_are_rejected_before_parsing() {
    let big = format!(
        r#"{{"type":"SetTitle","title":"{}"}}"#,
        "a".repeat(console_protocol::MAX_MESSAGE_BYTES)
    );
    assert!(matches!(
        ToolToShell::from_json(&big),
        Err(console_protocol::MessageError::TooLarge(_))
    ));
}

#[test]
fn tools_v1_fixtures_round_trip() {
    let d = dir("tools_v1");
    let read = |f: &str| fs::read_to_string(d.join(f)).unwrap();
    let tr: ToolsResponse = round_trip(&read("tools_response.json"));
    assert_eq!(tr.tools[0].origin, "https://hello.tools.ops.example.mil");
    round_trip::<LaunchRequest>(&read("launch_request.json"));
    round_trip::<LaunchResponse>(&read("launch_response.json"));
    round_trip::<ToolContext>(&read("tool_context.json"));
    let e: console_protocol::AuthErrorBody = round_trip(&read("error_bad_request.json"));
    assert_eq!(e.error, console_protocol::AuthErrorKind::BadRequest);
    let c: AssertionClaims = round_trip(&read("assertion_claims.json"));
    assert_eq!(c.exp - c.iat, console_protocol::tools::ASSERTION_TTL_SECS);
}

/// Guard like `session_v1`: optional fields stay optional, unknown fields are
/// tolerated, so a newer gateway never breaks an older tool or shell.
#[test]
fn tools_v1_minimal_and_extended_shapes_parse() {
    let t: ToolsResponse = serde_json::from_str(
        r#"{"tools":[{"id":"a","title":"A","origin":"https://a.t","future":1}],"next":2}"#,
    )
    .unwrap();
    assert_eq!(t.tools[0].entry, "/");
    assert!(t.tools[0].nav.is_empty());
    let empty: LaunchRequest = serde_json::from_str("{}").unwrap();
    assert_eq!(empty.path, None);
    let c: AssertionClaims = serde_json::from_str(
        r#"{"iss":"https://c","aud":"a","sub":"s","sid":"x","jti":"j","iat":1,"nbf":1,"exp":61,"cnf":{}}"#,
    )
    .unwrap();
    assert!(c.roles.is_empty());
    assert_eq!(c.acr, None);
}
