//! SDK v1 wire codec for the shell side, using the browser's `JSON` and
//! `Reflect` instead of serde_json (which costs about 50 KB gz in the shell
//! bundle; same reason as `api::FromJs`, PERF-001).
//!
//! Semantics mirror `ToolToShell::from_json` / `ShellToTool::to_json` in
//! console-protocol (lenient v1 rules: size cap first; non-object JSON
//! (`"x"`, `1`, `null`, `[1]`) and invalid JSON are malformed; on an object,
//! an unknown or missing `type` is "ignore"; a known type with a bad body is
//! malformed; unknown fields are ignored). INT-003: there is no known
//! difference from the serde codec. The wasm tests below check every
//! frozen `sdk_v1` fixture against the serde codec (INT-001 pattern).

use console_protocol::{FocusEdge, Level, MAX_MESSAGE_BYTES, Scope, ShellToTool, ToolToShell};
use web_sys::wasm_bindgen::JsValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    TooLarge,
    Malformed,
}

fn get(v: &JsValue, k: &str) -> JsValue {
    js_sys::Reflect::get(v, &JsValue::from_str(k)).unwrap_or(JsValue::UNDEFINED)
}

fn is_object(v: &JsValue) -> bool {
    v.is_object() && !js_sys::Array::is_array(v)
}

fn req_str(v: &JsValue, k: &str) -> Result<String, WireError> {
    get(v, k).as_string().ok_or(WireError::Malformed)
}

/// serde `u32`: an integral JSON number in range.
fn req_u32(v: &JsValue, k: &str) -> Result<u32, WireError> {
    as_u32(&get(v, k)).ok_or(WireError::Malformed)
}

fn as_u32(x: &JsValue) -> Option<u32> {
    let n = x.as_f64()?;
    (n.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(&n)).then_some(n as u32)
}

fn level(s: &str) -> Result<Level, WireError> {
    Ok(match s {
        "info" => Level::Info,
        "success" => Level::Success,
        "warning" => Level::Warning,
        "error" => Level::Error,
        _ => return Err(WireError::Malformed),
    })
}

fn crumbs(v: &JsValue) -> Result<Vec<(String, String)>, WireError> {
    let items = get(v, "items");
    if !js_sys::Array::is_array(&items) {
        return Err(WireError::Malformed);
    }
    js_sys::Array::from(&items)
        .iter()
        .map(|pair| {
            if !js_sys::Array::is_array(&pair) {
                return Err(WireError::Malformed);
            }
            let a = js_sys::Array::from(&pair);
            if a.length() != 2 {
                return Err(WireError::Malformed);
            }
            match (a.get(0).as_string(), a.get(1).as_string()) {
                (Some(l), Some(p)) => Ok((l, p)),
                _ => Err(WireError::Malformed),
            }
        })
        .collect()
}

/// Decode one tool -> shell message (see module docs).
pub fn decode(data: &str) -> Result<Option<ToolToShell>, WireError> {
    if data.len() > MAX_MESSAGE_BYTES {
        return Err(WireError::TooLarge);
    }
    let v = js_sys::JSON::parse(data).map_err(|_| WireError::Malformed)?;
    // Non-object JSON is malformed, as in console-protocol (INT-003).
    if !is_object(&v) {
        return Err(WireError::Malformed);
    }
    let Some(ty) = get(&v, "type").as_string() else {
        return Ok(None);
    };
    Ok(Some(match ty.as_str() {
        "Ready" => ToolToShell::Ready {
            protocol: req_u32(&v, "protocol")?,
        },
        "SetTitle" => ToolToShell::SetTitle {
            title: req_str(&v, "title")?,
        },
        "SetBreadcrumbs" => ToolToShell::SetBreadcrumbs { items: crumbs(&v)? },
        "Notify" => ToolToShell::Notify {
            level: level(&req_str(&v, "level")?)?,
            message: req_str(&v, "message")?,
        },
        "SetBadge" => ToolToShell::SetBadge {
            key: req_str(&v, "key")?,
            count: {
                let c = get(&v, "count");
                if c.is_undefined() || c.is_null() {
                    None
                } else {
                    Some(as_u32(&c).ok_or(WireError::Malformed)?)
                }
            },
        },
        "NavigateConsole" => ToolToShell::NavigateConsole {
            tool: req_str(&v, "tool")?,
            path: req_str(&v, "path")?,
        },
        "RequestSessionRefresh" => ToolToShell::RequestSessionRefresh,
        "PathChanged" => ToolToShell::PathChanged {
            path: req_str(&v, "path")?,
        },
        "FocusShell" => ToolToShell::FocusShell {
            reverse: {
                let r = get(&v, "reverse");
                if r.is_undefined() {
                    false
                } else {
                    r.as_bool().ok_or(WireError::Malformed)?
                }
            },
        },
        _ => return Ok(None),
    }))
}

struct Obj(js_sys::Object);

impl Obj {
    fn new(ty: &str) -> Self {
        let o = Obj(js_sys::Object::new());
        o.set("type", &JsValue::from_str(ty));
        o
    }
    fn set(&self, k: &str, v: &JsValue) {
        // Reflect.set on a fresh plain object cannot fail.
        let _ = js_sys::Reflect::set(&self.0, &JsValue::from_str(k), v);
    }
    fn s(self, k: &str, v: &str) -> Self {
        self.set(k, &JsValue::from_str(v));
        self
    }
}

fn opt(v: &Option<String>) -> JsValue {
    v.as_deref().map_or(JsValue::NULL, JsValue::from_str)
}

fn scope_obj(s: &Scope) -> JsValue {
    let o = js_sys::Object::new();
    for (k, v) in [
        ("site", &s.site),
        ("tenant", &s.tenant),
        ("environment", &s.environment),
    ] {
        let _ = js_sys::Reflect::set(&o, &JsValue::from_str(k), &opt(v));
    }
    o.into()
}

/// Encode one shell -> tool message (same JSON as `ShellToTool::to_json`).
pub fn encode(msg: &ShellToTool) -> Result<String, WireError> {
    let o = match msg {
        ShellToTool::Init {
            protocol,
            theme,
            locale,
            scope,
            user,
            path,
            tool_id,
        } => {
            let o = Obj::new("Init");
            o.set("protocol", &JsValue::from(*protocol));
            let o = o.s("theme", theme).s("locale", locale);
            o.set("scope", &scope_obj(scope));
            let u = Obj(js_sys::Object::new())
                .s("id", &user.id)
                .s("display_name", &user.display_name);
            let roles: js_sys::Array = user.roles.iter().map(|r| JsValue::from_str(r)).collect();
            u.set("roles", &roles);
            o.set("user", &u.0);
            let o = match path {
                Some(p) => o.s("path", p),
                None => o,
            };
            match tool_id {
                Some(t) => o.s("tool_id", t),
                None => o,
            }
        }
        ShellToTool::ScopeChanged { scope } => {
            let o = Obj::new("ScopeChanged");
            o.set("scope", &scope_obj(scope));
            o
        }
        ShellToTool::ThemeChanged { theme } => Obj::new("ThemeChanged").s("theme", theme),
        ShellToTool::Navigate { path } => Obj::new("Navigate").s("path", path),
        ShellToTool::SessionRefreshed {
            ok,
            idle_expires_at,
        } => {
            let o = Obj::new("SessionRefreshed");
            o.set("ok", &JsValue::from_bool(*ok));
            match idle_expires_at {
                Some(t) => o.s("idle_expires_at", t),
                None => o,
            }
        }
        ShellToTool::Focus { edge } => Obj::new("Focus").s(
            "edge",
            match edge {
                FocusEdge::First => "first",
                FocusEdge::Last => "last",
            },
        ),
    };
    js_sys::JSON::stringify(&o.0)
        .map(String::from)
        .map_err(|_| WireError::Malformed)
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    macro_rules! fx {
        ($n:literal) => {
            include_str!(concat!("../../console-protocol/tests/fixtures/sdk_v1/", $n))
        };
    }

    const TOOL: &[&str] = &[
        fx!("tool.FocusShell.json"),
        fx!("tool.NavigateConsole.json"),
        fx!("tool.Notify.json"),
        fx!("tool.PathChanged.json"),
        fx!("tool.Ready.json"),
        fx!("tool.RequestSessionRefresh.json"),
        fx!("tool.SetBadge.clear.json"),
        fx!("tool.SetBadge.json"),
        fx!("tool.SetBreadcrumbs.json"),
        fx!("tool.SetTitle.json"),
    ];
    const SHELL: &[&str] = &[
        fx!("shell.Focus.json"),
        fx!("shell.Init.json"),
        fx!("shell.Init.pre-m4.json"),
        fx!("shell.Navigate.json"),
        fx!("shell.ScopeChanged.json"),
        fx!("shell.SessionRefreshed.failed.json"),
        fx!("shell.SessionRefreshed.json"),
        fx!("shell.ThemeChanged.json"),
    ];

    /// Same classification as the serde codec: Ok(Some)/Ok(None)/Err kind.
    fn same(raw: &str) {
        let want = ToolToShell::from_json(raw);
        let got = decode(raw);
        match (&want, &got) {
            (Ok(a), Ok(b)) => assert_eq!(a, b, "{raw}"),
            (Err(console_protocol::MessageError::TooLarge(_)), Err(WireError::TooLarge)) => {}
            (Err(console_protocol::MessageError::Malformed(_)), Err(WireError::Malformed)) => {}
            _ => panic!("{raw}: serde {want:?} vs js {got:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn tool_fixtures_decode_like_serde() {
        for raw in TOOL {
            same(raw);
            assert!(matches!(decode(raw), Ok(Some(_))), "{raw}");
        }
    }

    #[wasm_bindgen_test]
    fn edge_cases_match_serde() {
        let big = format!(
            r#"{{"type":"SetTitle","title":"{}"}}"#,
            "a".repeat(MAX_MESSAGE_BYTES)
        );
        for raw in [
            r#"{"type":"Future","x":1}"#,
            r#"{"x":1}"#,
            r#"{"type":5}"#,
            r#""Ready""#,
            r#""x""#,
            "1",
            "null",
            "[1]",
            r#"["Ready",1]"#,
            "[]",
            "{",
            r#"{"type":"Ready"}"#,
            r#"{"type":"Ready","protocol":-1}"#,
            r#"{"type":"Ready","protocol":1.5}"#,
            r#"{"type":"Ready","protocol":"1"}"#,
            r#"{"type":"Ready","protocol":1,"extra":{"a":[1]}}"#,
            r#"{"type":"Notify","level":"loud","message":"x"}"#,
            r#"{"type":"SetBreadcrumbs","items":[["a"]]}"#,
            r#"{"type":"SetBreadcrumbs","items":[["a","/b","c"]]}"#,
            r#"{"type":"SetBreadcrumbs","items":"x"}"#,
            r#"{"type":"SetBadge","key":"k"}"#,
            r#"{"type":"SetBadge","key":"k","count":-2}"#,
            r#"{"type":"FocusShell"}"#,
            r#"{"type":"FocusShell","reverse":"yes"}"#,
            &big,
        ] {
            same(raw);
        }
        // INT-003: non-object JSON is malformed (dropped), never "ignore".
        for raw in [r#""x""#, "1", "null", "[1]"] {
            assert_eq!(decode(raw), Err(WireError::Malformed), "{raw}");
        }
    }

    #[wasm_bindgen_test]
    fn shell_fixtures_encode_like_serde() {
        for raw in SHELL {
            let msg = ShellToTool::from_json(raw).unwrap().unwrap();
            let js = encode(&msg).unwrap();
            assert_eq!(js, msg.to_json().unwrap(), "byte-identical to serde");
            assert_eq!(ShellToTool::from_json(&js).unwrap(), Some(msg));
        }
    }
}
