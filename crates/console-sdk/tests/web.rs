//! Browser tests for the tool-side transport (plan 8): origin and source
//! pinning, size cap, state updates, scope header.
//! `wasm-pack test --headless --chrome crates/console-sdk --features web`
#![cfg(all(target_arch = "wasm32", feature = "web"))]

use console_protocol::{ShellToTool, ToolContext};
use console_sdk::transport::{ConsoleBridge, Status, TransportError};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::JsValue;
use wasm_bindgen_test::*;
use web_sys::{MessageEvent, MessageEventInit, Request};

wasm_bindgen_test_configure!(run_in_browser);

const CONSOLE: &str = "https://console.invalid";
const INIT: &str = r#"{"type":"Init","protocol":1,"theme":"dark","locale":"en","scope":{"site":"dc1","tenant":null,"environment":null},"user":{"id":"u1","display_name":"Jane","roles":[]},"path":"/x","tool_id":"hello"}"#;

fn bridge() -> ConsoleBridge {
    ConsoleBridge::with_context(ToolContext {
        tool_id: "hello".into(),
        console_origin: CONSOLE.into(),
        protocol: 1,
    })
    .expect("bridge")
}

fn win() -> web_sys::Window {
    web_sys::window().unwrap()
}

/// Dispatch a synthetic `message` event on `window`.
fn post(origin: &str, source: Option<&js_sys::Object>, data: &JsValue) {
    let init = MessageEventInit::new();
    init.set_origin(origin);
    init.set_data(data);
    init.set_source(source);
    let e = MessageEvent::new_with_event_init_dict("message", &init).unwrap();
    win().dispatch_event(&e).unwrap();
}

fn top() -> js_sys::Object {
    win().unchecked_into()
}

#[wasm_bindgen_test]
fn rejects_foreign_origin() {
    let b = bridge();
    for bad in [
        "https://evil.invalid",
        "null",
        "",
        "https://console.invalid:444",
        "http://console.invalid",
    ] {
        post(bad, Some(&top()), &JsValue::from_str(INIT));
    }
    assert!(b.state().is_none());
    assert!(b.dropped() >= 5);
}

#[wasm_bindgen_test]
fn rejects_wrong_source() {
    let b = bridge();
    let doc = win().document().unwrap();
    let f: web_sys::HtmlIFrameElement = doc.create_element("iframe").unwrap().unchecked_into();
    doc.body().unwrap().append_child(&f).unwrap();
    let other: js_sys::Object = f.content_window().unwrap().unchecked_into();
    post(CONSOLE, Some(&other), &JsValue::from_str(INIT));
    post(CONSOLE, None, &JsValue::from_str(INIT));
    f.remove();
    assert!(b.state().is_none());
}

#[wasm_bindgen_test]
fn rejects_non_string_and_oversize() {
    let b = bridge();
    let obj = js_sys::JSON::parse(INIT).unwrap();
    post(CONSOLE, Some(&top()), &obj);
    let big = INIT.replace("Jane", &"J".repeat(17 * 1024));
    post(CONSOLE, Some(&top()), &JsValue::from_str(&big));
    assert!(b.state().is_none());
}

#[wasm_bindgen_test]
fn accepts_console_and_tracks_state() {
    let b = bridge();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sub = {
        let seen = seen.clone();
        b.subscribe(yew::Callback::from(move |m: ShellToTool| {
            seen.borrow_mut().push(m)
        }))
    };
    post(CONSOLE, Some(&top()), &JsValue::from_str(INIT));
    let st = b.state().expect("Init accepted");
    assert_eq!(st.theme, "dark");
    assert_eq!(st.scope.site.as_deref(), Some("dc1"));
    assert_eq!(st.path.as_deref(), Some("/x"));
    assert_eq!(b.status(), Status::Connected);

    post(
        CONSOLE,
        Some(&top()),
        &JsValue::from_str(r#"{"type":"ThemeChanged","theme":"light"}"#),
    );
    post(
        CONSOLE,
        Some(&top()),
        &JsValue::from_str(r#"{"type":"Navigate","path":"/y"}"#),
    );
    post(
        CONSOLE,
        Some(&top()),
        &JsValue::from_str(r#"{"type":"FromTheFuture"}"#),
    );
    let st = b.state().unwrap();
    assert_eq!(st.theme, "light");
    assert_eq!(st.path.as_deref(), Some("/y"));
    assert_eq!(seen.borrow().len(), 3, "unknown type not delivered");

    drop(sub);
    post(
        CONSOLE,
        Some(&top()),
        &JsValue::from_str(r#"{"type":"ThemeChanged","theme":"dark"}"#),
    );
    assert_eq!(seen.borrow().len(), 3, "unsubscribed on drop");
}

#[wasm_bindgen_test]
fn scope_header_from_state() {
    let b = bridge();
    let req = Request::new_with_str("/t/hello/api/x").unwrap();
    b.prepare_request(&req).unwrap();
    assert_eq!(
        req.headers().get("x-console-scope").unwrap(),
        None,
        "no state, no header"
    );
    post(CONSOLE, Some(&top()), &JsValue::from_str(INIT));
    let req = Request::new_with_str("/t/hello/api/x").unwrap();
    b.prepare_request(&req).unwrap();
    assert_eq!(
        req.headers().get("x-console-scope").unwrap().as_deref(),
        Some("site=dc1")
    );
}

#[wasm_bindgen_test]
fn bad_console_origin_is_refused() {
    let own = win().location().origin().unwrap();
    for bad in ["", "null", "javascript:x", own.as_str()] {
        let r = ConsoleBridge::with_context(ToolContext {
            tool_id: "hello".into(),
            console_origin: bad.into(),
            protocol: 1,
        });
        assert!(matches!(r, Err(TransportError::Context(_))), "{bad}");
    }
}

#[wasm_bindgen_test]
fn top_level_is_standalone() {
    assert_eq!(bridge().status(), Status::Standalone);
}

/// A11Y-001 / A11Y-007: `Focus` skips hidden, inert and closed-details
/// targets, and never moves focus that is already inside the tool.
#[wasm_bindgen_test]
fn focus_edge_picks_visible_target_only_from_body() {
    let b = bridge();
    let doc = win().document().unwrap();
    let body = doc.body().unwrap();
    let host = doc.create_element("div").unwrap();
    host.set_inner_html(
        r#"<button id="fe-hidden" hidden>h</button>
           <div hidden><button id="fe-hidden-parent">p</button></div>
           <div inert><button id="fe-inert">i</button></div>
           <details><summary id="fe-summary">s</summary><button id="fe-closed">c</button></details>
           <button id="fe-a">a</button><button id="fe-b">b</button>"#,
    );
    body.prepend_with_node_1(&host).unwrap();
    let active_id = || doc.active_element().map(|e| e.id()).unwrap_or_default();
    if let Some(a) = doc.active_element() {
        a.unchecked_into::<web_sys::HtmlElement>().blur().unwrap();
    }
    let first = JsValue::from_str(r#"{"type":"Focus","edge":"first"}"#);
    post(CONSOLE, Some(&top()), &first);
    assert_eq!(active_id(), "fe-summary", "first visible tabbable");
    // Focus already inside the tool: a later Focus is ignored.
    doc.get_element_by_id("fe-b")
        .unwrap()
        .unchecked_into::<web_sys::HtmlElement>()
        .focus()
        .unwrap();
    post(CONSOLE, Some(&top()), &first);
    assert_eq!(active_id(), "fe-b", "existing focus kept");
    host.remove();
    drop(b);
}
