//! Hostile fixture (`evil` registry id, its own origin). Each button makes
//! one escape attempt and writes the outcome to
//! `[data-testid="evil-result-<name>"]`:
//! - `blocked: <reason>`: the browser or console refused the attempt;
//! - `ESCAPED: <detail>`: the attempt worked (a test must fail on this);
//! - `sent: <n>` / `submitted`: fire-and-forget; E2E checks the shell side.
//!
//! Every message the shell sends back is counted in
//! `[data-testid="evil-shell-messages"]` (`data-count`), so E2E can confirm
//! the probes really reached the shell and were ignored (NIST-002): evil
//! never sends `Ready`, so a correct shell sends it nothing at all unless
//! the refresh loop below is on.
//!
//! `?probe=refresh-loop` in the tool URL (shell URL
//! `/tools/evil/?probe=refresh-loop`) starts an unattended
//! `RequestSessionRefresh` every [`REFRESH_LOOP_MS`] with no user input and
//! records the shell's answers in `evil-refresh-loop` (`data-replies`,
//! `data-ok-true`, `data-idle-expires-at`). With no trusted input in the
//! shell, every answer must be `ok:false` and the idle deadline must not
//! move (SEC-002 / NIST-001).
//!
//! It deliberately does not use the SDK, so nothing here is filtered
//! client-side. `console_origin` comes from `/_console/context`, like the SDK.
//! Messages are sent with that explicit target origin (never `"*"`): the point
//! is to test what the shell accepts, not to broadcast.

use std::collections::BTreeMap;

use console_protocol::ToolContext;
use console_protocol::tools::ROUTE_TOOL_CONTEXT;
use console_ui::{Button, Container, Header, HeaderVariant, Variant};
use js_sys::Reflect;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{HtmlFormElement, RequestCredentials, RequestInit, RequestMode};
use yew::prelude::*;

use crate::util::{fetch, js_err, response_json, use_flash};

/// Attempt names (stable; used in test ids).
pub const ATTEMPTS: &[(&str, &str)] = &[
    ("parent-dom", "Read window.parent.document"),
    ("parent-storage", "Read window.parent.localStorage"),
    ("frame-element", "Reach own <iframe> to remove sandbox"),
    ("top-navigation", "Navigate the top window"),
    (
        "postmessage-shapes",
        "postMessage malformed / unknown / oversized",
    ),
    ("notify-html", "Notify with an HTML payload"),
    ("notify-flood", "Notify 20 times (rate limit)"),
    (
        "navigate-console-bad",
        "NavigateConsole to an unlisted tool",
    ),
    (
        "api-keepalive-cors",
        "fetch console keepalive (cors, credentials)",
    ),
    (
        "api-keepalive-no-cors",
        "fetch console keepalive (no-cors, credentials)",
    ),
    (
        "api-session-read",
        "Read console /api/session (cors, credentials)",
    ),
    ("logout-form", "Form POST to console /logout"),
];

pub const HTML_PROBE: &str =
    "<img src=x data-evil-probe=1 onerror=\"document.title='pwned'\">evil-html-probe";

type Results = BTreeMap<&'static str, String>;

fn win() -> web_sys::Window {
    web_sys::window().expect("window")
}

fn get(target: &JsValue, key: &str) -> Result<JsValue, JsValue> {
    Reflect::get(target, &JsValue::from_str(key))
}

fn parent() -> JsValue {
    get(&win(), "parent").unwrap_or(JsValue::NULL)
}

/// `parent.postMessage(msg, origin)`. The parent is a cross-origin
/// `WindowProxy`, which fails `instanceof Window` in this realm, so
/// `dyn_into` would always fail and nothing would be sent (NIST-002).
/// `postMessage` is one of the few members callable cross-origin.
fn post(origin: &str, msg: &JsValue) -> Result<(), String> {
    let p = parent();
    if p.is_null() || p.is_undefined() || js_sys::Object::is(&p, &win()) {
        return Err("no parent".to_owned());
    }
    p.unchecked_into::<web_sys::Window>()
        .post_message(msg, origin)
        .map_err(js_err)
}

/// Interval of the unattended refresh probe (above the shell's 30 s rate
/// limit is not needed: rate-limited requests are answered `ok:false` too).
pub const REFRESH_LOOP_MS: i32 = 5_000;

/// What the shell sent back to this frame.
#[derive(Clone, Default, PartialEq)]
struct ShellLog {
    messages: u32,
    replies: u32,
    ok_true: u32,
    last_idle_expires_at: String,
}

enum LogAct {
    Message(Option<(bool, String)>),
}

impl Reducible for ShellLog {
    type Action = LogAct;
    fn reduce(self: std::rc::Rc<Self>, a: LogAct) -> std::rc::Rc<Self> {
        let mut s = (*self).clone();
        let LogAct::Message(refreshed) = a;
        s.messages += 1;
        if let Some((ok, at)) = refreshed {
            s.replies += 1;
            s.ok_true += u32::from(ok);
            s.last_idle_expires_at = at;
        }
        s.into()
    }
}

/// `Some((ok, idle_expires_at))` for a `SessionRefreshed` message.
fn as_refreshed(data: &str) -> Option<(bool, String)> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    (v.get("type")?.as_str()? == "SessionRefreshed").then(|| {
        (
            v.get("ok")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            v.get("idle_expires_at")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_owned(),
        )
    })
}

fn refresh_loop_requested() -> bool {
    win()
        .location()
        .search()
        .unwrap_or_default()
        .split(['?', '&'])
        .any(|p| p == "probe=refresh-loop")
}

fn blocked_or_escaped(r: Result<JsValue, JsValue>, what: &str) -> String {
    match r {
        Err(e) => format!("blocked: {}", js_err(e)),
        Ok(v) if v.is_null() || v.is_undefined() => format!("blocked: {what} is null"),
        Ok(_) => format!("ESCAPED: {what} readable"),
    }
}

fn sync_attempt(name: &str, origin: &str) -> String {
    match name {
        "parent-dom" => {
            let r = get(&parent(), "document").and_then(|d| get(&d, "body"));
            blocked_or_escaped(r, "parent.document.body")
        }
        "parent-storage" => {
            blocked_or_escaped(get(&parent(), "localStorage"), "parent.localStorage")
        }
        "frame-element" => match get(&win(), "frameElement") {
            Ok(f) if f.is_null() => "blocked: frameElement is null".into(),
            Ok(f) => {
                // Same-origin with the shell would let us do this.
                let r = f
                    .dyn_into::<web_sys::Element>()
                    .map(|el| el.remove_attribute("sandbox"));
                format!("ESCAPED: frameElement reachable ({})", r.is_ok())
            }
            Err(e) => format!("blocked: {}", js_err(e)),
        },
        "top-navigation" => {
            let r = get(&win(), "top")
                .and_then(|t| get(&t, "location"))
                .and_then(|l| {
                    Reflect::set(
                        &l,
                        &"href".into(),
                        &JsValue::from_str("https://example.invalid/evil"),
                    )
                });
            match r {
                Err(e) => format!("blocked: {}", js_err(e)),
                // Some engines only log; E2E asserts the shell URL is unchanged.
                Ok(_) => "no-exception (check shell URL)".into(),
            }
        }
        "postmessage-shapes" => {
            let big = format!(
                r#"{{"type":"Notify","level":"info","message":"{}"}}"#,
                "A".repeat(17 * 1024)
            );
            let obj = js_sys::Object::new();
            let _ = Reflect::set(&obj, &"type".into(), &"Notify".into());
            let _ = Reflect::set(&obj, &"level".into(), &"error".into());
            let _ = Reflect::set(&obj, &"message".into(), &"evil-object-message".into());
            let msgs: Vec<JsValue> = vec![
                "not json".into(),
                "[1,2,3]".into(),
                r#"{"type":"Notify"}"#.into(),
                r#"{"type":"Bogus","x":1}"#.into(),
                r#"{"type":"SetBreadcrumbs","items":[["x","javascript:alert(1)"]]}"#.into(),
                r#"{"type":"Notify","level":"evil","message":"evil-bad-level"}"#.into(),
                big.into(),
                obj.into(),
            ];
            let n = msgs.len();
            match msgs.iter().try_for_each(|m| post(origin, m)) {
                Ok(()) => format!("sent: {n}"),
                Err(e) => format!("blocked: {e}"),
            }
        }
        "notify-html" => {
            let m = serde_json::json!({"type":"Notify","level":"warning","message": HTML_PROBE});
            match post(origin, &m.to_string().into()) {
                Ok(()) => "sent: 1".into(),
                Err(e) => format!("blocked: {e}"),
            }
        }
        "notify-flood" => {
            let mut sent = 0;
            for i in 0..20 {
                let m = serde_json::json!({"type":"Notify","level":"info","message": format!("evil-flood-{i}")});
                if post(origin, &m.to_string().into()).is_ok() {
                    sent += 1;
                }
            }
            format!("sent: {sent}")
        }
        "navigate-console-bad" => {
            let m = r#"{"type":"NavigateConsole","tool":"denied","path":"/"}"#;
            match post(origin, &m.into()) {
                Ok(()) => "sent: 1".into(),
                Err(e) => format!("blocked: {e}"),
            }
        }
        "logout-form" => submit_logout(origin),
        _ => "unknown".into(),
    }
}

/// POST /logout into a hidden sink frame. The console must refuse it (Origin
/// check); E2E then asserts the console session is still alive.
fn submit_logout(origin: &str) -> String {
    let doc = win().document().expect("document");
    let r = (|| -> Result<(), JsValue> {
        let form: HtmlFormElement = doc.create_element("form")?.dyn_into()?;
        form.set_method("post");
        form.set_action(&format!("{origin}/logout"));
        form.set_target("evil-sink");
        doc.body().ok_or("no body")?.append_child(&form)?;
        form.submit()?;
        form.remove();
        Ok(())
    })();
    match r {
        Ok(()) => "submitted".into(),
        Err(e) => format!("blocked: {}", js_err(e)),
    }
}

async fn fetch_attempt(name: &str, origin: &str) -> String {
    let (path, method, mode) = match name {
        "api-keepalive-cors" => ("/api/session/keepalive", "POST", RequestMode::Cors),
        "api-keepalive-no-cors" => ("/api/session/keepalive", "POST", RequestMode::NoCors),
        _ => ("/api/session", "GET", RequestMode::Cors),
    };
    let init = RequestInit::new();
    init.set_method(method);
    init.set_mode(mode);
    init.set_credentials(RequestCredentials::Include);
    let req = match web_sys::Request::new_with_str_and_init(&format!("{origin}{path}"), &init) {
        Ok(r) => r,
        Err(e) => return format!("blocked: {}", js_err(e)),
    };
    match fetch(&req).await {
        Err(e) => format!("blocked: {e}"),
        // no-cors: the request may reach the server but the response is
        // opaque; the console's CSRF/Origin checks must refuse it. E2E checks
        // the session afterwards.
        Ok(r) if r.type_() == web_sys::ResponseType::Opaque => "sent: opaque".into(),
        Ok(r) => {
            let status = r.status();
            let body = match r.text() {
                Ok(p) => JsFuture::from(p).await.ok().and_then(|v| v.as_string()),
                Err(_) => None,
            }
            .unwrap_or_default();
            format!("ESCAPED: HTTP {status} readable ({} bytes)", body.len())
        }
    }
}

#[function_component]
pub fn Evil() -> Html {
    let flash = use_flash();
    let origin = use_state(|| None::<String>);
    let results = use_state(Results::new);
    let log = use_reducer(ShellLog::default);
    let looping = use_memo((), |_| refresh_loop_requested());

    {
        // Count everything the shell (window.parent, console origin) sends.
        let dispatch = log.dispatcher();
        use_effect_with((*origin).clone(), move |origin| {
            use wasm_bindgen::closure::Closure;
            let mut cleanup = None;
            if let Some(o) = origin.clone() {
                let cb = Closure::<dyn Fn(web_sys::MessageEvent)>::new(
                    move |e: web_sys::MessageEvent| {
                        let from_parent = e
                            .source()
                            .is_some_and(|s| js_sys::Object::is(&s, &parent()));
                        if from_parent && e.origin() == o {
                            let data = e.data().as_string().unwrap_or_default();
                            dispatch.dispatch(LogAct::Message(as_refreshed(&data)));
                        }
                    },
                );
                if win()
                    .add_event_listener_with_callback("message", cb.as_ref().unchecked_ref())
                    .is_ok()
                {
                    cleanup = Some(cb);
                }
            }
            move || {
                if let Some(cb) = cleanup {
                    let _ = win().remove_event_listener_with_callback(
                        "message",
                        cb.as_ref().unchecked_ref(),
                    );
                }
            }
        });
    }
    {
        // Unattended refresh loop (SEC-002 probe); no user input involved.
        let on = *looping;
        use_effect_with(((*origin).clone(), on), move |(origin, on)| {
            use wasm_bindgen::closure::Closure;
            let mut cleanup = None;
            if let (Some(o), true) = (origin.clone(), *on) {
                let cb = Closure::<dyn Fn()>::new(move || {
                    let _ = post(&o, &r#"{"type":"RequestSessionRefresh"}"#.into());
                });
                if let Ok(h) = win().set_interval_with_callback_and_timeout_and_arguments_0(
                    cb.as_ref().unchecked_ref(),
                    REFRESH_LOOP_MS,
                ) {
                    cleanup = Some((h, cb));
                }
            }
            move || {
                if let Some((h, _cb)) = cleanup {
                    win().clear_interval_with_handle(h);
                }
            }
        });
    }

    {
        let origin = origin.clone();
        let flash = flash.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let r = async {
                    let req = web_sys::Request::new_with_str(ROUTE_TOOL_CONTEXT).map_err(js_err)?;
                    let v = response_json(fetch(&req).await?).await?;
                    serde_json::from_value::<ToolContext>(v).map_err(|e| e.to_string())
                }
                .await;
                match r {
                    Ok(c) => origin.set(Some(c.console_origin)),
                    Err(e) => flash.error(format!("Could not load console context: {e}")),
                }
            });
        });
    }

    let run = {
        let origin = origin.clone();
        let results = results.clone();
        Callback::from(move |name: &'static str| {
            let Some(o) = (*origin).clone() else { return };
            let results = results.clone();
            if name.starts_with("api-") {
                wasm_bindgen_futures::spawn_local(async move {
                    let out = fetch_attempt(name, &o).await;
                    let mut m = (*results).clone();
                    m.insert(name, out);
                    results.set(m);
                });
            } else {
                let out = sync_attempt(name, &o);
                let mut m = (*results).clone();
                m.insert(name, out);
                results.set(m);
            }
        })
    };

    let ready = origin.is_some();
    html! {
        <main class="hello" data-testid="evil-root" data-ready={ready.to_string()}>
            { flash.view() }
            <Header variant={HeaderVariant::H1}
                description="Hostile test fixture. Every attempt below must be blocked.">
                { "Evil tool" }
            </Header>
            <Container testid="evil-attempts" header={html!{ <Header>{ "Escape attempts" }</Header> }}>
                <ul class="evil-list">
                { for ATTEMPTS.iter().map(|&(name, label)| {
                    let run = run.clone();
                    let onclick = Callback::from(move |_| run.emit(name));
                    let result = results.get(name).cloned().unwrap_or_default();
                    html! {
                        <li class="evil-item">
                            <Button variant={Variant::Normal} disabled={!ready} {onclick}
                                testid={format!("evil-run-{name}")}>{ label }</Button>
                            <output class="hello-mono hello-wrap" data-testid={format!("evil-result-{name}")}
                                aria-live="polite">{ result }</output>
                        </li>
                    }
                }) }
                </ul>
            </Container>
            <p data-testid="evil-shell-messages" data-count={log.messages.to_string()}>
                { format!("Messages from the shell: {}", log.messages) }
            </p>
            if *looping {
                <p data-testid="evil-refresh-loop" data-replies={log.replies.to_string()}
                    data-ok-true={log.ok_true.to_string()}
                    data-idle-expires-at={log.last_idle_expires_at.clone()}>
                    { format!("Unattended refresh: {} replies, {} honoured, idle_expires_at {}",
                        log.replies, log.ok_true, log.last_idle_expires_at) }
                </p>
            }
            <iframe name="evil-sink" title="Logout attempt sink" class="evil-sink"
                data-testid="evil-sink" sandbox="" />
        </main>
    }
}
