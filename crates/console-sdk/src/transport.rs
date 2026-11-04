//! Browser `postMessage` transport for tools (plan 4.7, 4.8).
//!
//! Trust rules (NIST 800-53 AC-4, SI-10):
//! - The console origin comes only from `GET /_console/context` on the
//!   tool's own origin (gateway-authoritative), never from
//!   `document.referrer` or the URL.
//! - Inbound messages are accepted only if `event.origin` equals that origin
//!   exactly ([`crate::codec::check_origin`]) AND `event.source` is
//!   `window.parent`, and the data is a string under the 16 KiB cap.
//! - Outbound messages go to `window.parent` with the console origin as
//!   `targetOrigin`, never `"*"`.
//!
//! Handshake: `Ready{protocol:1}` right away, re-sent at 250 ms, 500 ms and
//! 1 s until `Init` arrives; after 2 s the bridge reports
//! [`Status::Standalone`] (an `Init` that arrives later still connects). A
//! tool opened top-level (no parent) is standalone at once.
//!
//! Keyboard: F6 / Ctrl+F6 sends `FocusShell{reverse:false}`, Shift+F6
//! `FocusShell{reverse:true}`. A `Focus{edge}` from the shell (skip link or
//! F6 into the tool only) focuses the first/last visible tabbable element,
//! and only when nothing in the tool has focus yet (A11Y-001).

use crate::codec::{self, SdkError};
use console_protocol::tools::{ROUTE_TOOL_CONTEXT, SCOPE_HEADER, format_scope_header};
use console_protocol::{
    FocusEdge, Level, PROTOCOL_VERSION, Scope, ShellToTool, ToolContext, ToolToShell, User,
};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::JsValue;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    KeyboardEvent, MessageEvent, Request, RequestCredentials, RequestInit, Response, Window,
};
use yew::Callback;

/// Ready re-send delays (ms after connect), then the standalone deadline.
pub const READY_RETRIES_MS: [i32; 3] = [250, 500, 1000];
pub const STANDALONE_AFTER_MS: i32 = 2000;

/// Errors from connecting or fetching. Wraps the codec's [`SdkError`].
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error(transparent)]
    Sdk(#[from] SdkError),
    /// `/_console/context` failed or returned something unusable.
    #[error("console context unavailable: {0}")]
    Context(String),
    #[error("browser API unavailable: {0}")]
    Browser(&'static str),
}

/// What the shell told the tool (from `Init`, kept current by later messages).
#[derive(Clone, Debug, PartialEq)]
pub struct ConsoleState {
    pub user: User,
    pub scope: Scope,
    /// "light" | "dark" | "system" ("system": follow `prefers-color-scheme`).
    pub theme: String,
    pub locale: String,
    /// Tool-relative path from `Init` / the last `Navigate`.
    pub path: Option<String>,
    pub tool_id: Option<String>,
    /// Negotiated protocol version.
    pub protocol: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Waiting for `Init`.
    Connecting,
    Connected,
    /// No console answered (opened top-level, or the shell is not there).
    Standalone,
}

type Watcher = Rc<dyn Fn()>;

struct Inner {
    console_origin: String,
    tool_id: String,
    window: Window,
    /// Expected `event.source` (`window.parent`; `window` itself when the
    /// tool is top-level, so tests can exercise the receive path).
    parent: Option<Window>,
    /// Framed by another document; only then is anything sent.
    embedded: bool,
    state: RefCell<Option<Rc<ConsoleState>>>,
    status: Cell<Status>,
    next_id: Cell<u64>,
    subscribers: RefCell<Vec<(u64, Callback<ShellToTool>)>>,
    watchers: RefCell<Vec<(u64, Watcher)>>,
    handlers: RefCell<Option<Handlers>>,
    dropped: Cell<u32>,
}

struct Handlers {
    message: Closure<dyn Fn(MessageEvent)>,
    keydown: Closure<dyn Fn(KeyboardEvent)>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(h) = self.handlers.borrow_mut().take() {
            let _ = self
                .window
                .remove_event_listener_with_callback("message", h.message.as_ref().unchecked_ref());
            if let Some(doc) = self.window.document() {
                let _ = doc.remove_event_listener_with_callback(
                    "keydown",
                    h.keydown.as_ref().unchecked_ref(),
                );
            }
        }
    }
}

/// Connection to the console shell. Cheap to clone (shared inner).
#[derive(Clone)]
pub struct ConsoleBridge {
    inner: Rc<Inner>,
}

impl PartialEq for ConsoleBridge {
    fn eq(&self, o: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &o.inner)
    }
}

impl std::fmt::Debug for ConsoleBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsoleBridge")
            .field("console_origin", &self.inner.console_origin)
            .field("tool_id", &self.inner.tool_id)
            .field("status", &self.inner.status.get())
            .finish()
    }
}

/// Drop to unsubscribe.
pub struct Subscription {
    inner: Weak<Inner>,
    id: u64,
    watcher: bool,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(i) = self.inner.upgrade() {
            if self.watcher {
                i.watchers.borrow_mut().retain(|(id, _)| *id != self.id);
            } else {
                i.subscribers.borrow_mut().retain(|(id, _)| *id != self.id);
            }
        }
    }
}

thread_local! {
    /// The most recently connected bridge, for the free functions below.
    static CURRENT: RefCell<Option<ConsoleBridge>> = const { RefCell::new(None) };
}

/// The bridge the free functions use, if one is connected.
pub fn current() -> Option<ConsoleBridge> {
    CURRENT.with(|c| c.borrow().clone())
}

fn window() -> Result<Window, TransportError> {
    web_sys::window().ok_or(TransportError::Browser("window"))
}

/// `GET /_console/context` (same origin, tool session cookie).
pub async fn fetch_context() -> Result<ToolContext, TransportError> {
    let win = window()?;
    let init = RequestInit::new();
    init.set_method("GET");
    init.set_credentials(RequestCredentials::SameOrigin);
    let req = Request::new_with_str_and_init(ROUTE_TOOL_CONTEXT, &init)
        .map_err(|_| TransportError::Browser("Request"))?;
    req.headers()
        .set("Accept", "application/json")
        .map_err(|_| TransportError::Browser("Headers"))?;
    let resp: Response = JsFuture::from(win.fetch_with_request(&req))
        .await
        .map_err(|_| TransportError::Context("network error".into()))?
        .dyn_into()
        .map_err(|_| TransportError::Browser("Response"))?;
    if !resp.ok() {
        return Err(TransportError::Context(format!("HTTP {}", resp.status())));
    }
    let text = JsFuture::from(
        resp.text()
            .map_err(|_| TransportError::Browser("Response.text"))?,
    )
    .await
    .map_err(|_| TransportError::Context("unreadable body".into()))?
    .as_string()
    .ok_or_else(|| TransportError::Context("unreadable body".into()))?;
    serde_json::from_str(&text).map_err(|_| TransportError::Context("unexpected body".into()))
}

/// Candidate selector for elements reachable with Tab; [`is_tabbable`]
/// then drops invisible, inert and hidden ones (A11Y-007).
const TABBABLE: &str = "a[href], button:not([disabled]), \
    input:not([disabled]):not([type=hidden]), select:not([disabled]), textarea:not([disabled]), \
    summary, [tabindex]:not([tabindex=\"-1\"]):not([disabled])";

/// Rendered (has layout boxes: excludes `display:none`, `[hidden]`
/// ancestors and closed `<details>` content), not `inert`, not
/// `visibility:hidden`-style zero-box elements, and not `tabindex=-1`.
fn is_tabbable(el: &web_sys::HtmlElement) -> bool {
    el.get_client_rects().length() > 0
        && el
            .closest("[inert], [hidden], [aria-hidden=\"true\"]")
            .ok()
            .flatten()
            .is_none()
        && el.tab_index() >= 0
}

/// Polling bound for [`focus_edge`]: wait up to `FOCUS_WAIT_TRIES` x
/// `FOCUS_WAIT_MS` for the tool document to receive focus.
const FOCUS_WAIT_MS: i32 = 16;
const FOCUS_WAIT_TRIES: u32 = 30;

/// `Focus{edge}` from the shell (sent only when the user entered the tool
/// with the skip link or F6; A11Y-001). Applied only if nothing inside the
/// tool has focus yet (`activeElement` is `<body>` or none), so a click or a
/// native Tab / Shift+Tab into the frame is never overridden.
///
/// Firefox (fix round 2, F1): the shell's `iframe.focus()` moves focus into
/// the frame document asynchronously, so `Focus{edge}` can arrive before the
/// tool document has focus; focusing an element then is undone when Firefox
/// finishes focusing the document (focus ends on `<body>`). So we wait
/// (bounded) until `document.hasFocus()`, then re-check `<body>` and apply,
/// and re-check once more on the next tick in case focus was reset.
fn focus_edge(win: &Window, edge: FocusEdge) {
    focus_edge_when_focused(win.clone(), edge, 0, false);
}

fn focus_edge_when_focused(win: Window, edge: FocusEdge, tries: u32, applied: bool) {
    let Some(doc) = win.document() else {
        return;
    };
    if !doc.has_focus().unwrap_or(true) {
        if tries < FOCUS_WAIT_TRIES {
            let w = win.clone();
            schedule(&win, FOCUS_WAIT_MS, move || {
                focus_edge_when_focused(w, edge, tries + 1, applied)
            });
        }
        return;
    }
    if !focus_at_body(&doc) {
        return;
    }
    apply_focus_edge(&doc, edge);
    if !applied {
        // One confirming pass: if the browser reset focus to <body> after
        // we moved it, apply again (still only from <body>).
        let w = win.clone();
        schedule(&win, FOCUS_WAIT_MS, move || {
            focus_edge_when_focused(w, edge, FOCUS_WAIT_TRIES, true)
        });
    }
}

fn focus_at_body(doc: &web_sys::Document) -> bool {
    let body = doc.body();
    match (doc.active_element(), &body) {
        (None, _) => true,
        (Some(a), Some(b)) => js_sys::Object::is(&a, b),
        (Some(_), None) => false,
    }
}

fn apply_focus_edge(doc: &web_sys::Document, edge: FocusEdge) {
    let Ok(list) = doc.query_selector_all(TABBABLE) else {
        return;
    };
    let mut it = (0..list.length())
        .filter_map(|i| list.item(i))
        .filter_map(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
        .filter(is_tabbable);
    let pick = match edge {
        FocusEdge::First => it.next(),
        FocusEdge::Last => it.next_back(),
    };
    if let Some(el) = pick {
        let _ = el.focus();
    }
}

impl Inner {
    fn next_id(&self) -> u64 {
        let id = self.next_id.get().wrapping_add(1);
        self.next_id.set(id);
        id
    }

    fn notify_watchers(&self) {
        let ws: Vec<Watcher> = self
            .watchers
            .borrow()
            .iter()
            .map(|(_, w)| w.clone())
            .collect();
        for w in ws {
            w();
        }
    }

    fn set_status(&self, s: Status) {
        if self.status.get() != s {
            self.status.set(s);
            self.notify_watchers();
        }
    }

    fn send(&self, msg: &ToolToShell) -> Result<(), TransportError> {
        let parent = self
            .parent
            .as_ref()
            .filter(|_| self.embedded)
            .ok_or(TransportError::Browser("parent window"))?;
        let s = codec::encode(msg)?;
        // NIST 800-53 AC-4, SC-7: explicit console origin as targetOrigin,
        // never "*".
        parent
            .post_message(&JsValue::from_str(&s), &self.console_origin)
            .map_err(|_| TransportError::Browser("postMessage"))
    }

    /// `event.source === window.parent`.
    fn is_from_parent(&self, e: &MessageEvent) -> bool {
        match (e.source(), &self.parent) {
            (Some(src), Some(p)) => js_sys::Object::is(&src, p),
            _ => false,
        }
    }

    fn receive(&self, e: &MessageEvent) {
        // NIST 800-53 AC-4, SI-10: source must be window.parent AND origin
        // must equal the gateway-provided console origin before decoding.
        let msg = match (self.is_from_parent(e), e.data().as_string()) {
            (true, Some(data)) => codec::decode(&e.origin(), &self.console_origin, &data),
            _ => Err(SdkError::UntrustedOrigin(String::new())),
        };
        let msg = match msg {
            Ok(Some(m)) => m,
            // Newer shell message type: ignored by design.
            Ok(None) => return,
            Err(_) => {
                // Count only; never log message data or origins.
                let n = self.dropped.get().saturating_add(1);
                self.dropped.set(n);
                web_sys::console::debug_1(&format!("console-sdk: dropped {n} message(s)").into());
                return;
            }
        };
        self.apply(&msg);
        let subs: Vec<Callback<ShellToTool>> = self
            .subscribers
            .borrow()
            .iter()
            .map(|(_, c)| c.clone())
            .collect();
        for s in subs {
            s.emit(msg.clone());
        }
        self.notify_watchers();
    }

    fn apply(&self, msg: &ShellToTool) {
        let mut st = self.state.borrow_mut();
        match msg {
            ShellToTool::Init {
                protocol,
                theme,
                locale,
                scope,
                user,
                path,
                tool_id,
            } => {
                *st = Some(Rc::new(ConsoleState {
                    user: user.clone(),
                    scope: scope.clone(),
                    theme: theme.clone(),
                    locale: locale.clone(),
                    path: path.clone(),
                    tool_id: tool_id.clone(),
                    protocol: (*protocol).clamp(1, PROTOCOL_VERSION),
                }));
                drop(st);
                self.status.set(Status::Connected);
            }
            ShellToTool::ScopeChanged { scope } => {
                if let Some(s) = st.as_mut() {
                    Rc::make_mut(s).scope = scope.clone();
                }
            }
            ShellToTool::ThemeChanged { theme } => {
                if let Some(s) = st.as_mut() {
                    Rc::make_mut(s).theme = theme.clone();
                }
            }
            ShellToTool::Navigate { path } => {
                if let Some(s) = st.as_mut() {
                    Rc::make_mut(s).path = Some(path.clone());
                }
            }
            ShellToTool::Focus { edge } => {
                drop(st);
                focus_edge(&self.window, *edge);
            }
            ShellToTool::SessionRefreshed { .. } => {}
        }
    }

    fn on_key(&self, e: &KeyboardEvent) {
        if e.key() != "F6" || !self.embedded || e.alt_key() || e.meta_key() {
            return;
        }
        e.prevent_default();
        if let Err(err) = self.send(&ToolToShell::FocusShell {
            reverse: e.shift_key(),
        }) {
            web_sys::console::warn_1(&format!("console-sdk: {err}").into());
            return;
        }
        // Leaving the tool: drop the tool's own focused element so the next
        // F6 / Shift+F6 / skip link entry lands on the requested edge
        // instead of the remembered control (Firefox keeps the frame's
        // `activeElement` when focus leaves; fix round 2, F1).
        if let Some(a) = self
            .window
            .document()
            .and_then(|d| d.active_element())
            .and_then(|a| a.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = a.blur();
        }
    }
}

fn schedule(win: &Window, ms: i32, f: impl FnOnce() + 'static) {
    let cb = Closure::once_into_js(f);
    let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), ms);
}

impl ConsoleBridge {
    /// Fetch `/_console/context`, then [`ConsoleBridge::with_context`].
    pub async fn connect() -> Result<ConsoleBridge, TransportError> {
        let ctx = fetch_context().await?;
        Self::with_context(ctx)
    }

    /// Install the listeners and start the handshake with a known context.
    /// Used by [`ConsoleBridge::connect`] and by tests.
    pub fn with_context(ctx: ToolContext) -> Result<ConsoleBridge, TransportError> {
        let window = window()?;
        let own = window.location().origin().unwrap_or_default();
        let origin = ctx.console_origin;
        if origin.is_empty() || origin == "null" || !origin.starts_with("http") {
            return Err(TransportError::Context("invalid console_origin".into()));
        }
        if origin == own {
            // NIST 800-53 SC-39: a tool must never share the console origin (plan section 0).
            return Err(TransportError::Context(
                "console_origin equals the tool origin".into(),
            ));
        }
        let parent = window
            .parent()
            .ok()
            .flatten()
            .filter(|p| !js_sys::Object::is(p, &window));
        let top_level = parent.is_none();
        // Tests run top-level: keep `window` as the "parent" source so the
        // receive path is exercised, but never send to it.
        let source_parent = parent.clone().or_else(|| top_level.then(|| window.clone()));
        let inner = Rc::new(Inner {
            console_origin: origin,
            tool_id: ctx.tool_id,
            window: window.clone(),
            parent: source_parent,
            embedded: !top_level,
            state: RefCell::new(None),
            status: Cell::new(if top_level {
                Status::Standalone
            } else {
                Status::Connecting
            }),
            next_id: Cell::new(0),
            subscribers: RefCell::new(Vec::new()),
            watchers: RefCell::new(Vec::new()),
            handlers: RefCell::new(None),
            dropped: Cell::new(0),
        });
        let weak = Rc::downgrade(&inner);
        let message = Closure::<dyn Fn(MessageEvent)>::new({
            let weak = weak.clone();
            move |e: MessageEvent| {
                if let Some(i) = weak.upgrade() {
                    i.receive(&e);
                }
            }
        });
        let keydown = Closure::<dyn Fn(KeyboardEvent)>::new({
            let weak = weak.clone();
            move |e: KeyboardEvent| {
                if let Some(i) = weak.upgrade() {
                    i.on_key(&e);
                }
            }
        });
        window
            .add_event_listener_with_callback("message", message.as_ref().unchecked_ref())
            .map_err(|_| TransportError::Browser("addEventListener"))?;
        if let Some(doc) = window.document() {
            doc.add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref())
                .map_err(|_| TransportError::Browser("addEventListener"))?;
        }
        *inner.handlers.borrow_mut() = Some(Handlers { message, keydown });
        let bridge = ConsoleBridge { inner };
        CURRENT.with(|c| *c.borrow_mut() = Some(bridge.clone()));
        if !top_level {
            bridge.start_handshake();
        }
        Ok(bridge)
    }

    fn start_handshake(&self) {
        let ready = ToolToShell::Ready {
            protocol: PROTOCOL_VERSION,
        };
        if let Err(e) = self.inner.send(&ready) {
            web_sys::console::warn_1(&format!("console-sdk: {e}").into());
        }
        let win = self.inner.window.clone();
        for ms in READY_RETRIES_MS {
            let weak = Rc::downgrade(&self.inner);
            let ready = ready.clone();
            schedule(&win, ms, move || {
                if let Some(i) = weak.upgrade()
                    && i.status.get() == Status::Connecting
                {
                    let _ = i.send(&ready);
                }
            });
        }
        let weak = Rc::downgrade(&self.inner);
        schedule(&win, STANDALONE_AFTER_MS, move || {
            if let Some(i) = weak.upgrade()
                && i.status.get() == Status::Connecting
            {
                i.set_status(Status::Standalone);
            }
        });
    }

    pub fn console_origin(&self) -> &str {
        &self.inner.console_origin
    }

    pub fn tool_id(&self) -> &str {
        &self.inner.tool_id
    }

    pub fn status(&self) -> Status {
        self.inner.status.get()
    }

    /// Shell state after `Init`.
    pub fn state(&self) -> Option<Rc<ConsoleState>> {
        self.inner.state.borrow().clone()
    }

    /// Send to the shell. No-op when top-level; failures are logged as a
    /// console warning (nothing for the tool to recover).
    pub fn send(&self, msg: &ToolToShell) {
        if !self.inner.embedded {
            return;
        }
        if let Err(e) = self.inner.send(msg) {
            web_sys::console::warn_1(&format!("console-sdk: {e}").into());
        }
    }

    /// Every accepted shell message, after the state is updated.
    pub fn subscribe(&self, f: Callback<ShellToTool>) -> Subscription {
        let id = self.inner.next_id();
        self.inner.subscribers.borrow_mut().push((id, f));
        Subscription {
            inner: Rc::downgrade(&self.inner),
            id,
            watcher: false,
        }
    }

    /// Called on any state or status change (for UI re-render).
    pub fn watch(&self, f: impl Fn() + 'static) -> Subscription {
        let id = self.inner.next_id();
        self.inner.watchers.borrow_mut().push((id, Rc::new(f)));
        Subscription {
            inner: Rc::downgrade(&self.inner),
            id,
            watcher: true,
        }
    }

    /// Number of inbound messages dropped by the origin/source/size checks.
    pub fn dropped(&self) -> u32 {
        self.inner.dropped.get()
    }

    /// Adds `X-Console-Scope` (from the current scope) to `req`.
    pub fn prepare_request(&self, req: &Request) -> Result<(), TransportError> {
        if let Some(v) = self.state().and_then(|s| format_scope_header(&s.scope)) {
            req.headers()
                .set(SCOPE_HEADER, &v)
                .map_err(|_| TransportError::Browser("Headers"))?;
        }
        Ok(())
    }
}

/// `fetch` for tool API calls: adds `X-Console-Scope` (advisory selection;
/// the tool backend must authorize it). On 401 asks the shell to refresh the
/// session. Build `req` with `credentials: same-origin` (the default).
pub async fn console_fetch(req: Request) -> Result<Response, TransportError> {
    let bridge = current();
    if let Some(b) = &bridge {
        b.prepare_request(&req)?;
    }
    let resp: Response = JsFuture::from(window()?.fetch_with_request(&req))
        .await
        .map_err(|_| TransportError::Context("network error".into()))?
        .dyn_into()
        .map_err(|_| TransportError::Browser("Response"))?;
    if resp.status() == 401
        && let Some(b) = &bridge
    {
        b.send(&ToolToShell::RequestSessionRefresh);
    }
    Ok(resp)
}

fn send_current(msg: ToolToShell) {
    if let Some(b) = current() {
        b.send(&msg);
    }
}

/// Page title part; the shell shows "<title> - <tool> - Console".
pub fn set_title(t: &str) {
    send_current(ToolToShell::SetTitle { title: t.into() });
}

/// `[(label, tool-relative path or "")]`, after the shell's "Console / <tool>".
pub fn set_breadcrumbs(items: &[(&str, &str)]) {
    send_current(ToolToShell::SetBreadcrumbs {
        items: items
            .iter()
            .map(|(l, p)| ((*l).to_owned(), (*p).to_owned()))
            .collect(),
    });
}

/// Flashbar notification (plain text, max 500 chars; rate-limited by the shell).
pub fn notify(level: Level, msg: &str) {
    send_current(ToolToShell::Notify {
        level,
        message: msg.into(),
    });
}

pub fn set_badge(key: &str, count: Option<u32>) {
    send_current(ToolToShell::SetBadge {
        key: key.into(),
        count,
    });
}

/// Tell the shell the tool's internal route changed (mirrored in its URL).
pub fn path_changed(path: &str) {
    send_current(ToolToShell::PathChanged { path: path.into() });
}

pub fn request_session_refresh() {
    send_current(ToolToShell::RequestSessionRefresh);
}

/// Open another console tool (must be in the user's tool list).
pub fn navigate_console(tool: &str, path: &str) {
    send_current(ToolToShell::NavigateConsole {
        tool: tool.into(),
        path: path.into(),
    });
}
