//! Tool container: `/tools/<id>/*path`. Launches the tool through the
//! gateway (`POST /api/tools/{id}/launch`), frames its own origin in a
//! sandboxed iframe and bridges SDK protocol v1 messages (plan 4.7, 4.9).
//!
//! Trust rules (NIST 800-53 AC-4, SC-7, SI-10):
//! - Inbound messages are accepted only when `event.origin` equals the
//!   registry origin of the mounted tool AND `event.source` is that iframe's
//!   `contentWindow`, the data is a string, and it decodes under the 16 KiB
//!   cap. Anything else is dropped and only counted (no data is logged).
//! - Outbound messages use the registry origin as `targetOrigin`, never `"*"`.
//! - Everything a tool sends for display is rendered as text nodes, capped,
//!   and stripped of control characters. Paths must pass [`safe_tool_path`].
//!
//! Pure logic lives at the top of this file and is unit-tested natively; the
//! component below wires it to the DOM.

use crate::ctx::{PageMeta, ScopeCtx, SessionCtx, ShellCtx, ThemeCtx};
use crate::scope;
use crate::session::{SessionInfo, origin_of};
use crate::tool_wire::{self, WireError};
use crate::tools::{
    MAX_LABEL_CHARS, ToolsCtx, ToolsState, clean_text, safe_tool_path, scope_for, tool_href,
};
use crate::{api, history_go};
use console_protocol::tools::ROUTE_TOOL_LAUNCH;
use console_protocol::{
    FocusEdge, Level, MAX_NOTIFY_CHARS, PROTOCOL_VERSION, Scope, ShellToTool, ToolSummary,
    ToolToShell, User, valid_id,
};
use console_ui::{
    Alert, AlertKind, Badge, BadgeColor, Button, Container, NavLink, Spinner, SpinnerSize,
};
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use yew::prelude::*;
use yew_router::prelude::use_location;

// ---------------------------------------------------------------------------
// Pure logic (native tests)
// ---------------------------------------------------------------------------

/// iframe `sandbox` flags: plan 4.9 plus `allow-modals` (user decision Q4,
/// 2026-11-04). No `allow-top-navigation*`, no
/// `allow-popups-to-escape-sandbox`: popups inherit this sandbox.
/// `allow-same-origin` is safe here only because each tool is served from
/// its own origin (decision Q1, option (a)).
// NIST 800-53 SC-7, SC-18, SC-39: the only sandbox the shell ever applies.
pub const SANDBOX: &str =
    "allow-scripts allow-same-origin allow-forms allow-downloads allow-popups allow-modals";

/// How long after the iframe `load` the shell waits for `Ready`.
pub const READY_TIMEOUT_MS: i32 = 10_000;
/// `Notify`: at most this many per window per tool, then dropped.
pub const NOTIFY_BURST: usize = 5;
pub const NOTIFY_WINDOW_MS: f64 = 10_000.0;
/// Rate limit for `RequestSessionRefresh`: at most one keepalive per this
/// interval. This limit alone does NOT stop a tool from holding the console
/// session open; the user-presence gate in `activity` does (SEC-002 /
/// NIST-001, AC-11/AC-12; see `request_refresh`).
pub const REFRESH_MIN_GAP_MS: f64 = 30_000.0;
pub const MAX_TITLE_CHARS: usize = 120;
pub const MAX_CRUMBS: usize = 8;
pub const MAX_BADGES: usize = 8;
pub const MAX_BADGE_KEY_CHARS: usize = 32;
/// Element id of the tool iframe (target of "Skip to tool content").
pub const FRAME_ID: &str = "tool-frame";

/// Tool-relative path for the current shell URL, or `None` if `pathname` is
/// not under `/tools/<id>`. Scope keys (`site`, `tenant`, `env`) belong to
/// the shell and are removed from the query; other parameters and the hash
/// go to the tool.
pub fn tool_path_from_location(
    id: &str,
    pathname: &str,
    search: &str,
    hash: &str,
) -> Option<String> {
    let rest = pathname.strip_prefix("/tools/")?.strip_prefix(id)?;
    let path = match rest {
        "" => "/",
        r if r.starts_with('/') => r,
        _ => return None,
    };
    let q = scope::to_query(&format!("{search}{hash}"), &Scope::default());
    Some(format!("{path}{q}"))
}

/// Shell URL for a tool path: `/tools/<id><path>?<scope>&<tool query>#hash`.
/// Inverse of [`tool_path_from_location`] (tool query keys named like scope
/// keys are reserved and dropped).
pub fn shell_url(id: &str, tool_path: &str, scope: &Scope) -> String {
    let split = tool_path.find(['?', '#']).unwrap_or(tool_path.len());
    let (path, rest) = tool_path.split_at(split);
    format!("{}{}", tool_href(id, path), scope::to_query(rest, scope))
}

/// Path to send in the launch body: `None` (registry entry) for the tool
/// root, the path if it is safe, otherwise `Err` (and the entry is used).
pub fn launch_path(tool_path: &str) -> Result<Option<String>, ()> {
    if tool_path == "/" {
        Ok(None)
    } else if safe_tool_path(tool_path) {
        Ok(Some(tool_path.to_owned()))
    } else {
        Err(())
    }
}

/// The launch response URL must be `<registry origin>/_console/launch?code=..`.
pub fn accept_launch_url(url: &str, tool_origin: &str) -> bool {
    if url
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return false;
    }
    origin_of(url).as_deref() == Some(tool_origin)
        && url
            .strip_prefix(tool_origin)
            .and_then(|p| p.strip_prefix(ROUTE_TOOL_LAUNCH))
            .is_some_and(|q| q.starts_with('?'))
}

/// Why an inbound `message` event was dropped (only counted, never logged
/// with its data).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    Origin,
    Source,
    NotString,
    TooLarge,
    Malformed,
}

#[derive(Debug, PartialEq)]
pub enum Inbound {
    Drop(DropReason),
    /// Well-formed message of a type this shell does not know (newer SDK).
    Ignore,
    Msg(ToolToShell),
}

/// Origin + source + size + decode checks for one inbound message.
/// `expected_origin` is the registry origin of the mounted tool; `source_ok`
/// is `event.source === iframe.contentWindow`. `decode` is
/// [`tool_wire::decode`] in the browser (the serde codec in native tests);
/// it only ever sees data that passed both checks.
pub fn classify(
    origin: &str,
    expected_origin: &str,
    source_ok: bool,
    data: Option<&str>,
    decode: impl FnOnce(&str) -> Result<Option<ToolToShell>, WireError>,
) -> Inbound {
    if expected_origin.is_empty() || origin == "null" || origin != expected_origin {
        return Inbound::Drop(DropReason::Origin);
    }
    if !source_ok {
        return Inbound::Drop(DropReason::Source);
    }
    let Some(data) = data else {
        return Inbound::Drop(DropReason::NotString);
    };
    match decode(data) {
        Ok(Some(m)) => Inbound::Msg(m),
        Ok(None) => Inbound::Ignore,
        Err(WireError::TooLarge) => Inbound::Drop(DropReason::TooLarge),
        Err(WireError::Malformed) => Inbound::Drop(DropReason::Malformed),
    }
}

/// `[Console, <tool>, ...tool items]`. Item paths are tool-relative; `""`
/// or an unsafe path renders as plain text.
pub fn breadcrumbs(tool: &ToolSummary, items: &[(String, String)], scope: &Scope) -> Vec<NavLink> {
    let mut out = vec![
        NavLink::new("Console", "/"),
        NavLink::new(tool.title.clone(), shell_url(&tool.id, &tool.entry, scope)),
    ];
    out.extend(items.iter().map(|(label, path)| {
        let href = if safe_tool_path(path) {
            shell_url(&tool.id, path, scope)
        } else {
            String::new()
        };
        NavLink::new(label.clone(), href)
    }));
    out
}

/// Cleans `SetBreadcrumbs.items` (labels capped, empties dropped, at most
/// [`MAX_CRUMBS`]).
pub fn clean_crumbs(items: Vec<(String, String)>) -> Vec<(String, String)> {
    items
        .into_iter()
        .map(|(l, p)| (clean_text(&l, MAX_LABEL_CHARS), p))
        .filter(|(l, _)| !l.is_empty())
        .take(MAX_CRUMBS)
        .collect()
}

// NIST 800-53 SI-10: caps on everything a tool can put in the chrome
// (MAX_TITLE_CHARS, MAX_CRUMBS, MAX_BADGES, NOTIFY_BURST).
/// Upserts a badge; `None`/0 removes it. New keys beyond [`MAX_BADGES`] are
/// dropped.
pub fn apply_badge(badges: &mut Vec<(String, u32)>, key: &str, count: Option<u32>) {
    let key = clean_text(key, MAX_BADGE_KEY_CHARS);
    if key.is_empty() {
        return;
    }
    let pos = badges.iter().position(|(k, _)| *k == key);
    match (count.filter(|c| *c > 0), pos) {
        (None, Some(i)) => {
            badges.remove(i);
        }
        (None, None) => {}
        (Some(c), Some(i)) => badges[i].1 = c,
        (Some(c), None) if badges.len() < MAX_BADGES => badges.push((key, c)),
        (Some(_), None) => {}
    }
}

/// Sliding-window limiter for `Notify`.
#[derive(Default, Debug)]
pub struct Burst {
    stamps: Vec<f64>,
    noticed: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// First drop in a run: show one shell notice.
    DropAndNotice,
    Drop,
}

impl Burst {
    pub fn check(&mut self, now_ms: f64) -> Verdict {
        self.stamps.retain(|t| now_ms - *t < NOTIFY_WINDOW_MS);
        if self.stamps.len() < NOTIFY_BURST {
            self.stamps.push(now_ms);
            self.noticed = false;
            Verdict::Allow
        } else if !std::mem::replace(&mut self.noticed, true) {
            Verdict::DropAndNotice
        } else {
            Verdict::Drop
        }
    }
}

pub fn alert_kind(l: Level) -> AlertKind {
    match l {
        Level::Info => AlertKind::Info,
        Level::Success => AlertKind::Success,
        Level::Warning => AlertKind::Warning,
        Level::Error => AlertKind::Error,
    }
}

/// Flash text for a tool notification. Prefixed with the tool title so a
/// tool cannot pass its message off as the console's own.
pub fn notify_text(tool_title: &str, message: &str) -> String {
    format!("{tool_title}: {}", clean_text(message, MAX_NOTIFY_CHARS))
}

/// Protocol the shell speaks with a tool: `min(tool, shell)`, at least 1.
pub fn negotiate(tool: u32) -> u32 {
    tool.clamp(1, PROTOCOL_VERSION)
}

/// Display-only user for `Init`. `roles` is reserved and always empty by
/// design (INT-007, orchestrator decision 2026-11-04): tools take roles from
/// the signed gateway assertion or their backend, never from the shell.
pub fn init_user(s: &SessionInfo) -> User {
    User {
        id: s.user.id.clone(),
        display_name: s.user.display_name.clone(),
        roles: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------
//
// The hook body only collects state and wires effects; the work lives in
// plain functions over `Latest` / `HostState` (PERF-001: smaller
// monomorphised closures).

#[derive(Clone, PartialEq, Debug, Default)]
enum Phase {
    /// Waiting for the tool list and session before launching.
    #[default]
    Waiting,
    Launching,
    Frame(AttrValue),
    /// Unknown id, or a tool the user may not open: the gateway answers 404
    /// for both (INT-009), so the shell shows "Tool not found" either way.
    NotFound,
    Error(AttrValue),
}

impl Phase {
    /// Variant only (effect dependency for focus management).
    fn kind(&self) -> u8 {
        match self {
            Phase::Waiting => 0,
            Phase::Launching => 1,
            Phase::Frame(_) => 2,
            Phase::NotFound => 3,
            Phase::Error(_) => 4,
        }
    }
}

#[derive(Clone, PartialEq, Debug, Default)]
struct HostState {
    phase: Phase,
    /// Bumped by "Reload tool": a new iframe element and a new launch.
    generation: u32,
    connected: bool,
    timed_out: bool,
    title: Option<AttrValue>,
    crumbs: Vec<(String, String)>,
    badges: Vec<(String, u32)>,
    /// Set by "Reload tool": focus follows the new content (A11Y-004).
    refocus: bool,
    /// Polite announcement of a tool title change (A11Y-005).
    announce: AttrValue,
}

enum Act {
    Launching(u32),
    Phase(u32, Phase),
    Connected(u32),
    TimedOut(u32),
    /// New title; `true` if focus is in the frame (announce it).
    Title(String, bool),
    Crumbs(Vec<(String, String)>),
    Badge(String, Option<u32>),
    Reload,
    Refocused,
}

impl Reducible for HostState {
    type Action = Act;
    fn reduce(self: Rc<Self>, a: Act) -> Rc<Self> {
        let mut s = Rc::unwrap_or_clone(self);
        match a {
            // Results for an older generation (before a reload) are stale.
            Act::Launching(g) | Act::Phase(g, _) | Act::Connected(g) | Act::TimedOut(g)
                if g != s.generation => {}
            Act::Launching(_) => s.phase = Phase::Launching,
            Act::Phase(_, p) => s.phase = p,
            Act::Connected(_) => {
                s.connected = true;
                s.timed_out = false;
            }
            Act::TimedOut(_) => s.timed_out = !s.connected,
            Act::Title(t, announce) => {
                if announce && s.title.as_deref() != Some(t.as_str()) {
                    s.announce = t.clone().into();
                }
                s.title = Some(t.into());
            }
            Act::Crumbs(c) => s.crumbs = c,
            Act::Badge(k, c) => apply_badge(&mut s.badges, &k, c),
            Act::Reload => {
                s = HostState {
                    generation: s.generation.wrapping_add(1),
                    refocus: true,
                    ..HostState::default()
                }
            }
            Act::Refocused => s.refocus = false,
        }
        Rc::new(s)
    }
}

type Flash = Callback<(AlertKind, AttrValue)>;

/// Values the long-lived listeners read at event time.
#[derive(Default)]
struct Latest {
    id: String,
    tool: Option<ToolSummary>,
    tools: Option<Rc<Vec<ToolSummary>>>,
    scope: Scope,
    theme: AttrValue,
    session: Option<Rc<SessionInfo>>,
    tool_path: String,
    generation: u32,
    connected: bool,
    frame: NodeRef,
    flash: Flash,
    keepalive: Callback<()>,
    dispatch: Option<UseReducerDispatcher<HostState>>,
    // Bridge bookkeeping.
    last_path: String,
    sent_scope: Option<Scope>,
    sent_theme: Option<AttrValue>,
    notify: Burst,
    last_refresh_ms: Option<f64>,
    /// Session snapshot (pointer) when a tool asked for a refresh.
    refresh_pending: Option<usize>,
    dropped: u32,
}

thread_local! {
    /// The mounted tool host, for the skip link and F6 (A11Y-001).
    static MOUNTED: RefCell<Weak<RefCell<Latest>>> = const { RefCell::new(Weak::new()) };
}

fn session_ptr(s: &Option<Rc<SessionInfo>>) -> usize {
    s.as_ref().map_or(0, |r| Rc::as_ptr(r) as usize)
}

fn content_window(frame: &NodeRef) -> Option<web_sys::Window> {
    frame
        .cast::<web_sys::HtmlIFrameElement>()
        .and_then(|f| f.content_window())
}

/// `document.activeElement` is the tool iframe.
pub(crate) fn frame_has_focus() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.active_element())
        .is_some_and(|e| e.id() == FRAME_ID)
}

/// Attribute for the state-driven frame focus ring (fix round 3, R2-F1).
pub const FRAME_RING_ATTR: &str = "data-frame-focused";

/// Pure rule for the state-driven ring: the shell's `activeElement` is the
/// frame, the frame does not match `:focus` (that ring already applies), and
/// the shell window itself has focus, so focus is on the `<iframe>` stop and
/// not inside the tool's content (the tool draws its own ring there).
pub fn frame_ring_wanted(active_is_frame: bool, matches_focus: bool, shell_focused: bool) -> bool {
    active_is_frame && !matches_focus && shell_focused
}

/// Sets or clears [`FRAME_RING_ATTR`] on the mounted frame. Firefox puts a
/// reverse-Tab stop on the `<iframe>` element without matching `:focus` or
/// `:focus-visible`, so CSS alone draws no ring there (WCAG 2.4.7).
pub(crate) fn sync_frame_ring(shell_focused: bool) {
    let Some(f) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(FRAME_ID))
    else {
        return;
    };
    let active = frame_has_focus();
    let matches =
        f.matches(":focus").unwrap_or(false) || f.matches(":focus-visible").unwrap_or(false);
    if frame_ring_wanted(active, matches, shell_focused) {
        let _ = f.set_attribute(FRAME_RING_ATTR, "");
    } else if f.has_attribute(FRAME_RING_ATTR) {
        let _ = f.remove_attribute(FRAME_RING_ATTR);
    }
}

/// `postMessage` to the tool frame with its registry origin as target.
// NIST 800-53 AC-4, SC-7: explicit registry origin as targetOrigin, never "*".
fn post(l: &Latest, msg: &ShellToTool) {
    let Some(origin) = l.tool.as_ref().map(|t| t.origin.as_str()) else {
        return;
    };
    let sent = tool_wire::encode(msg)
        .ok()
        .zip(content_window(&l.frame))
        .is_some_and(|(s, w)| {
            w.post_message(&web_sys::wasm_bindgen::JsValue::from_str(&s), origin)
                .is_ok()
        });
    if !sent {
        l.flash.emit((
            AlertKind::Warning,
            "Could not send an update to the tool. Reload the tool if it looks out of date.".into(),
        ));
    }
}

fn send_init(l: &mut Latest, protocol: u32) {
    let Some(session) = l.session.clone() else {
        return;
    };
    let scope = l
        .tool
        .as_ref()
        .map(|t| scope_for(t, &l.scope))
        .unwrap_or_default();
    let msg = ShellToTool::Init {
        protocol: negotiate(protocol),
        theme: l.theme.to_string(),
        locale: "en".into(),
        scope: scope.clone(),
        user: init_user(&session),
        path: Some(l.tool_path.clone()),
        tool_id: Some(l.id.clone()),
    };
    post(l, &msg);
    l.last_path = l.tool_path.clone();
    l.sent_scope = Some(scope);
    l.sent_theme = Some(l.theme.clone());
}

/// The mounted tool frame element, if it is not inert (modal open).
fn mounted_frame() -> Option<(Rc<RefCell<Latest>>, web_sys::HtmlIFrameElement)> {
    let latest = MOUNTED.with(|m| m.borrow().upgrade())?;
    let f = latest.borrow().frame.cast::<web_sys::HtmlIFrameElement>()?;
    if f.closest("[inert]").ok().flatten().is_some() {
        return None;
    }
    Some((latest, f))
}

/// Sends `Focus{edge}` if the tool is connected and the frame has focus.
fn post_focus_if_entered(edge: FocusEdge) {
    if !frame_has_focus() {
        return;
    }
    if let Some(latest) = MOUNTED.with(|m| m.borrow().upgrade()) {
        let l = latest.borrow();
        if l.connected {
            post(&l, &ShellToTool::Focus { edge });
        }
    }
}

/// Runs `f` on the next task.
fn defer(f: impl FnOnce() + 'static) {
    use web_sys::wasm_bindgen::{JsCast, closure::Closure};
    if let Some(w) = web_sys::window() {
        let cb = Closure::once_into_js(f);
        let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), 0);
    }
}

/// Moves keyboard focus into the mounted tool frame and, once the tool is
/// connected, sends `Focus{edge}` (A11Y-001). Used only by the "Skip to tool
/// content" link (`First`) and F6 / Shift+F6 in the shell (`First` /
/// `Last`). A click into the frame sends nothing.
/// Returns `false` when no frame is mounted or it is inert (modal open).
///
/// Fix round 2 (F1): Firefox ignores `iframe.focus()` on a cross-origin
/// (out-of-process) frame while a trusted key event is being dispatched
/// (F6, or Enter activating the skip link): `activeElement` stays put and no
/// blur fires (probe in fixes-round-1-shell.md, Round 2). When the
/// synchronous focus did not land, it is retried on the next task, where
/// Firefox honours it.
pub fn enter_tool(edge: FocusEdge) -> bool {
    let Some((_, f)) = mounted_frame() else {
        return false;
    };
    if f.focus().is_err() {
        return false;
    }
    if frame_has_focus() {
        post_focus_if_entered(edge);
    } else {
        defer(move || {
            if let Some((_, f)) = mounted_frame() {
                let _ = f.focus();
                post_focus_if_entered(edge);
            }
        });
    }
    true
}

/// Native Tab / Shift+Tab from the shell into the frame. Chromium and WebKit
/// move focus straight to the tool's first / last control. Firefox stops on
/// the frame document first (`activeElement` is the tool `<body>`), so one
/// Tab would not reach a control. After a trusted Tab in the shell, if
/// focus is now in the frame, send `Focus{First}` (Shift+Tab: `Last`); the
/// SDK applies it only while the tool's focus is on `<body>`, so the
/// Chromium/WebKit result is never overridden.
fn after_shell_tab(e: &web_sys::KeyboardEvent) {
    if e.key() != "Tab" || !e.is_trusted() || e.alt_key() || e.ctrl_key() || e.meta_key() {
        return;
    }
    if frame_has_focus() {
        return;
    }
    let edge = if e.shift_key() {
        FocusEdge::Last
    } else {
        FocusEdge::First
    };
    defer(move || post_focus_if_entered(edge));
}

/// F6 / Ctrl+F6 in the shell: into the tool (first control); Shift+F6:
/// into the tool from the end (last control). The SDK's F6 inside the tool
/// moves back out (`FocusShell`).
fn on_shell_key(e: &web_sys::KeyboardEvent) {
    after_shell_tab(e);
    if e.key() != "F6" || e.alt_key() || e.meta_key() || e.default_prevented() {
        return;
    }
    let edge = if e.shift_key() {
        FocusEdge::Last
    } else {
        FocusEdge::First
    };
    if enter_tool(edge) {
        e.prevent_default();
    }
}

/// Registers this host for [`enter_tool`] and the shell F6 listener.
fn mount_host(latest: &Rc<RefCell<Latest>>, flash: &Flash) -> impl FnOnce() + use<> {
    use web_sys::wasm_bindgen::{JsCast, closure::Closure};
    MOUNTED.with(|m| *m.borrow_mut() = Rc::downgrade(latest));
    let me = Rc::downgrade(latest);
    let doc = web_sys::window().and_then(|w| w.document());
    let cb = Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(|e: web_sys::KeyboardEvent| {
        on_shell_key(&e)
    });
    let ok = doc.as_ref().is_some_and(|d| {
        d.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref())
            .is_ok()
    });
    if !ok {
        flash.emit((
            AlertKind::Warning,
            "F6 cannot move focus into the tool in this browser; use \"Skip to tool content\"."
                .into(),
        ));
    }
    move || {
        if let Some(d) = doc {
            let _ = d.remove_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref());
        }
        MOUNTED.with(|m| {
            if Weak::ptr_eq(&m.borrow(), &me) {
                *m.borrow_mut() = Weak::new();
            }
        });
    }
}

/// Retries for [`focus_shell`] when the browser keeps focus in the frame.
const FOCUS_SHELL_RETRIES: u32 = 5;

/// Focus a shell control when the keyboard user leaves the tool (F6).
///
/// Fix round 2 (F1): Firefox can refuse the focus move while the tool's F6
/// keydown is still being handled in the frame process; `activeElement`
/// stays the iframe. Retry on later tasks (bounded) until focus has left.
///
/// Fix round 3: retries stop once the user acted (trusted shell key or
/// pointer) or focus left the frame since the `FocusShell`, so a quick
/// re-entry is not pulled back out.
fn focus_shell(reverse: bool) {
    focus_shell_once(reverse);
    let epoch = crate::activity::focus_epoch();
    focus_shell_retry(reverse, 0, epoch);
}

fn focus_shell_attempt(reverse: bool, tries: u32, epoch: u32) {
    if crate::activity::focus_epoch() != epoch {
        return;
    }
    focus_shell_once(reverse);
    focus_shell_retry(reverse, tries + 1, epoch);
}

fn focus_shell_retry(reverse: bool, tries: u32, epoch: u32) {
    if frame_has_focus() && tries < FOCUS_SHELL_RETRIES {
        let delay = 16 * i32::try_from(tries).unwrap_or(0);
        if let Some(w) = web_sys::window() {
            use web_sys::wasm_bindgen::{JsCast, closure::Closure};
            let cb = Closure::once_into_js(move || focus_shell_attempt(reverse, tries, epoch));
            let _ =
                w.set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), delay);
        }
    }
}

fn focus_shell_once(reverse: bool) {
    use web_sys::wasm_bindgen::JsCast;
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let pick = |sel: &str| -> Option<web_sys::HtmlElement> {
        let list = doc.query_selector_all(sel).ok()?;
        let n = list.length();
        let i = if reverse { n.checked_sub(1)? } else { 0 };
        list.item(i)?.dyn_into().ok()
    };
    let sels: &[&str] = if reverse {
        &[
            "#side-nav-region:not([hidden]) a",
            "[data-testid=\"nav-toggle\"]",
            "#main-content",
        ]
    } else {
        &[
            "header.cui-topnav a, header.cui-topnav button",
            "#main-content",
        ]
    };
    if let Some(el) = sels.iter().find_map(|s| pick(s)) {
        let _ = el.focus();
    }
}

/// `RequestSessionRefresh` (SEC-002 / NIST-001; NIST 800-53 AC-11, AC-12).
/// Honoured only when the shell saw trusted user input in the last
/// [`crate::activity::REFRESH_INPUT_WINDOW_MS`] or the idle warning is open
/// (see `activity` for the definition and the cross-origin limitation), at
/// most once per [`REFRESH_MIN_GAP_MS`], one at a time. Otherwise the tool
/// gets `SessionRefreshed{ok:false}` with the current idle deadline and the
/// session is not extended, so an unattended tool cannot keep it alive.
fn request_refresh(mut st: std::cell::RefMut<'_, Latest>) {
    let now = js_sys::Date::now();
    let rate_limited = st
        .last_refresh_ms
        .is_some_and(|t| now - t < REFRESH_MIN_GAP_MS);
    if st.session.is_none()
        || rate_limited
        || st.refresh_pending.is_some()
        || !crate::activity::user_present_now()
    {
        let idle_expires_at = st.session.as_ref().map(|s| s.idle_expires_at.clone());
        post(
            &st,
            &ShellToTool::SessionRefreshed {
                ok: false,
                idle_expires_at,
            },
        );
        return;
    }
    st.last_refresh_ms = Some(now);
    st.refresh_pending = Some(session_ptr(&st.session));
    let keepalive = st.keepalive.clone();
    drop(st);
    keepalive.emit(());
}

fn notify(st: &mut Latest, tool_title: &str, level: Level, message: &str) {
    let flash = st.flash.clone();
    match st.notify.check(js_sys::Date::now()) {
        Verdict::Allow => flash.emit((alert_kind(level), notify_text(tool_title, message).into())),
        Verdict::DropAndNotice => flash.emit((
            AlertKind::Warning,
            format!("{tool_title} sent too many notifications; some were not shown.").into(),
        )),
        Verdict::Drop => {}
    }
}

/// Runs from the DOM `message` listener. Holding `latest` mutably across
/// `dispatch`/`flash.emit` is safe because Yew defers rendering to a
/// microtask in the browser; `history_go` and `keepalive` are still called
/// after the borrow is dropped since they reach code outside this module.
fn handle(l: &Rc<RefCell<Latest>>, msg: ToolToShell) {
    let mut st = l.borrow_mut();
    let Some(dispatch) = st.dispatch.clone() else {
        return;
    };
    let generation = st.generation;
    match msg {
        ToolToShell::Ready { protocol } => {
            send_init(&mut st, protocol);
            dispatch.dispatch(Act::Connected(generation));
        }
        ToolToShell::SetTitle { title } => dispatch.dispatch(Act::Title(
            clean_text(&title, MAX_TITLE_CHARS),
            frame_has_focus(),
        )),
        ToolToShell::SetBreadcrumbs { items } => {
            dispatch.dispatch(Act::Crumbs(clean_crumbs(items)))
        }
        ToolToShell::Notify { level, message } => {
            let title = st
                .tool
                .as_ref()
                .map(|t| t.title.clone())
                .unwrap_or_default();
            notify(&mut st, &title, level, &message);
        }
        ToolToShell::SetBadge { key, count } => dispatch.dispatch(Act::Badge(key, count)),
        ToolToShell::NavigateConsole { tool, path } => {
            let known = st
                .tools
                .as_ref()
                .is_some_and(|ts| ts.iter().any(|t| t.id == tool));
            if known && safe_tool_path(&path) {
                let url = shell_url(&tool, &path, &st.scope);
                drop(st);
                history_go(&url, false);
            } else {
                web_sys::console::warn_1(
                    &"console: ignored NavigateConsole to an unknown tool or invalid path".into(),
                );
            }
        }
        ToolToShell::RequestSessionRefresh => request_refresh(st),
        ToolToShell::PathChanged { path } => {
            if safe_tool_path(&path) {
                st.last_path.clone_from(&path);
                st.tool_path.clone_from(&path);
                let url = shell_url(&st.id, &path, &st.scope);
                drop(st);
                // Replace: iframe navigations already add history entries.
                history_go(&url, true);
            }
        }
        ToolToShell::FocusShell { reverse } => {
            drop(st);
            focus_shell(reverse);
            crate::activity::note_tool_moved_focus();
        }
    }
}

fn on_message(l: &Rc<RefCell<Latest>>, e: web_sys::MessageEvent) {
    // NIST 800-53 AC-4, SC-7, SI-10: origin == registry origin AND
    // source === this iframe's contentWindow before any decoding.
    let (expected, source_ok) = {
        let st = l.borrow();
        let expected = st
            .tool
            .as_ref()
            .map(|t| t.origin.clone())
            .unwrap_or_default();
        let source_ok = match (e.source(), content_window(&st.frame)) {
            (Some(src), Some(w)) => js_sys::Object::is(&src, &w),
            _ => false,
        };
        (expected, source_ok)
    };
    let data = e.data().as_string();
    match classify(
        &e.origin(),
        &expected,
        source_ok,
        data.as_deref(),
        tool_wire::decode,
    ) {
        Inbound::Msg(m) => handle(l, m),
        Inbound::Ignore => {}
        Inbound::Drop(_) => {
            let mut st = l.borrow_mut();
            st.dropped = st.dropped.saturating_add(1);
            // Count only: never log message data or origins.
            web_sys::console::debug_1(
                &format!("console: dropped {} tool message(s)", st.dropped).into(),
            );
        }
    }
}

/// `message` listener while a tool is known; returns the cleanup.
fn listen_messages(
    latest: &Rc<RefCell<Latest>>,
    flash: &Flash,
    active: bool,
) -> impl FnOnce() + use<> {
    use web_sys::wasm_bindgen::{JsCast, closure::Closure};
    let mut cleanup = None;
    if let (true, Some(win)) = (active, web_sys::window()) {
        let l = latest.clone();
        let cb = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |e| on_message(&l, e));
        if win
            .add_event_listener_with_callback("message", cb.as_ref().unchecked_ref())
            .is_ok()
        {
            cleanup = Some((win, cb));
        } else {
            flash.emit((
                AlertKind::Error,
                "Tool messages cannot be received in this browser.".into(),
            ));
        }
    }
    move || {
        if let Some((win, cb)) = cleanup {
            let _ = win.remove_event_listener_with_callback("message", cb.as_ref().unchecked_ref());
        }
    }
}

/// Starts the `Ready` wait after the iframe `load` event.
fn start_ready_timer(dispatch: UseReducerDispatcher<HostState>, generation: u32) {
    use web_sys::wasm_bindgen::JsCast;
    let cb = web_sys::wasm_bindgen::closure::Closure::once_into_js(move || {
        dispatch.dispatch(Act::TimedOut(generation));
    });
    if let Some(w) = web_sys::window() {
        let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.unchecked_ref(),
            READY_TIMEOUT_MS,
        );
    }
}

/// Inputs of the launch effect.
#[derive(Clone, PartialEq)]
struct LaunchDeps {
    id: String,
    generation: u32,
    /// 0 loading, 1 ready, 2 failed.
    tools_phase: u8,
    csrf: Option<String>,
    session_failed: bool,
}

/// Launches once the list and session are in (per generation).
fn start_launch(
    d: &LaunchDeps,
    waiting: bool,
    tool: Option<ToolSummary>,
    tool_path: &str,
    tools: Option<ToolsCtx>,
    flash: &Flash,
    dispatch: UseReducerDispatcher<HostState>,
) {
    let generation = d.generation;
    if !waiting {
        return;
    }
    if !valid_id(&d.id) {
        dispatch.dispatch(Act::Phase(generation, Phase::NotFound));
        return;
    }
    if d.tools_phase == 2 || d.session_failed {
        dispatch.dispatch(Act::Phase(
            generation,
            Phase::Error("The tool list or your session could not be loaded.".into()),
        ));
        return;
    }
    let (1, Some(csrf)) = (d.tools_phase, d.csrf.clone()) else {
        return;
    };
    let path = launch_path(tool_path).unwrap_or_else(|()| {
        flash.emit((
            AlertKind::Warning,
            "Ignored an invalid tool path in the address; opening the tool's start page.".into(),
        ));
        None
    });
    dispatch.dispatch(Act::Launching(generation));
    let id = d.id.clone();
    yew::platform::spawn_local(async move {
        let phase = match api::launch_tool(&id, path.as_deref(), &csrf).await {
            Ok(r) => match &tool {
                Some(t) if accept_launch_url(&r.url, &t.origin) => Phase::Frame(r.url.into()),
                Some(_) => Phase::Error("The console returned an unexpected tool address.".into()),
                None => {
                    // Allowed but not in our (stale) list.
                    if let Some(t) = &tools {
                        t.reload.emit(());
                    }
                    Phase::Error("The tool list was out of date. Reload the tool.".into())
                }
            },
            // Unknown and not-permitted tools are both 404 (INT-009).
            Err(api::ApiError::Status(404)) => Phase::NotFound,
            Err(api::ApiError::Unauthenticated) => return,
            Err(e) => Phase::Error(
                e.message("Could not open the tool")
                    .unwrap_or_default()
                    .into(),
            ),
        };
        dispatch.dispatch(Act::Phase(generation, phase));
    });
}

/// Inputs of the outbound-update effect (one effect, PERF-001).
#[derive(Clone, PartialEq)]
struct Outbound {
    connected: bool,
    scope: Scope,
    theme: AttrValue,
    path: String,
}

/// `ScopeChanged` / `ThemeChanged` / `Navigate`, only while connected and
/// only on change. A shell URL change made by the user (back/forward, nav
/// link) sends `Navigate` with no relaunch; our own `PathChanged` replace
/// already set `last_path`.
fn sync_outbound(l: &mut Latest, o: &Outbound) {
    if !o.connected {
        return;
    }
    if l.sent_scope.as_ref() != Some(&o.scope) {
        post(
            l,
            &ShellToTool::ScopeChanged {
                scope: o.scope.clone(),
            },
        );
        l.sent_scope = Some(o.scope.clone());
    }
    if l.sent_theme.as_ref() != Some(&o.theme) {
        post(
            l,
            &ShellToTool::ThemeChanged {
                theme: o.theme.to_string(),
            },
        );
        l.sent_theme = Some(o.theme.clone());
    }
    if l.last_path != o.path {
        l.last_path.clone_from(&o.path);
        post(
            l,
            &ShellToTool::Navigate {
                path: o.path.clone(),
            },
        );
    }
}

/// Answers a pending `RequestSessionRefresh` once the keepalive settles.
fn answer_refresh(l: &mut Latest, ptr: usize, busy: bool) {
    let Some(before) = l.refresh_pending else {
        return;
    };
    if busy {
        return;
    }
    l.refresh_pending = None;
    let ok = ptr != before && ptr != 0;
    // Current deadline either way, so the tool never has to guess.
    let idle_expires_at = l.session.as_ref().map(|s| s.idle_expires_at.clone());
    post(
        l,
        &ShellToTool::SessionRefreshed {
            ok,
            idle_expires_at,
        },
    );
}

/// After "Reload tool": focus the loading region, then the frame wrapper or
/// the error/not-found panel; never into the tool (A11Y-004). Stops as soon
/// as the user has moved focus elsewhere.
fn refocus(kind: u8, dispatch: &UseReducerDispatcher<HostState>) {
    let doc = web_sys::window().and_then(|w| w.document());
    let free = doc
        .as_ref()
        .is_some_and(|d| match (d.active_element(), d.body()) {
            (None, _) => true,
            (Some(a), Some(b)) if a == *b.as_ref() => true,
            (Some(a), _) => a
                .closest("[data-testid=\"tool-host\"]")
                .ok()
                .flatten()
                .is_some(),
        });
    if free {
        crate::focus_selector("[data-testid=\"tool-host\"] [data-refocus]");
    }
    if !free || kind >= 2 {
        dispatch.dispatch(Act::Refocused);
    }
}

const NOT_FOUND_TITLE: &str = "Tool not found";

/// Page header, breadcrumbs and document title for the current state.
fn page_meta(tool: Option<&ToolSummary>, id: &str, s: &HostState, scope: &Scope) -> PageMeta {
    if s.phase == Phase::NotFound {
        // A11Y-003: heading and document title say what happened.
        return PageMeta::simple(NOT_FOUND_TITLE);
    }
    let title: AttrValue = tool.map_or_else(|| id.to_owned().into(), |t| t.title.clone().into());
    PageMeta {
        title: title.clone(),
        description: None,
        breadcrumbs: match tool {
            Some(t) => breadcrumbs(t, &s.crumbs, scope),
            None => vec![
                NavLink::new("Console", "/"),
                NavLink::new(title.clone(), ""),
            ],
        },
        document_title: s.title.as_ref().map(|t| format!("{t} - {title}").into()),
    }
}

fn not_found_panel(scope: &Scope) -> Html {
    let home = format!("/{}", scope::to_query("", scope));
    let onclick = {
        let home = home.clone();
        Callback::from(move |e: MouseEvent| {
            if e.button() == 0 && !e.ctrl_key() && !e.meta_key() && !e.shift_key() && !e.alt_key() {
                e.prevent_default();
                history_go(&home, false);
            }
        })
    };
    html! {
        <div role="alert" tabindex="-1" data-refocus="" data-testid="tool-not-found">
            <Container>
                <p>{ "This tool does not exist, is no longer available, or you do not have access to it." }</p>
                <p>
                    <a href={home} {onclick} data-testid="tool-not-found-home">{ "Go to Console home" }</a>
                </p>
            </Container>
        </div>
    }
}

fn frame_view(
    s: &HostState,
    url: &AttrValue,
    id: &str,
    title: String,
    frame: NodeRef,
    reload_button: Html,
    dispatch: UseReducerDispatcher<HostState>,
) -> Html {
    let onload = {
        let generation = s.generation;
        Callback::from(move |_: Event| start_ready_timer(dispatch.clone(), generation))
    };
    let connected = s.connected;
    let label = format!("{title} tool");
    // NIST 800-53 SC-7, SC-18, SC-39: sandbox flags fixed by [`SANDBOX`];
    // the frame is always a separate per-tool origin (decision Q1).
    html! {
        <>
            // Persistent live region so the warning is announced (A11Y-002).
            <div role="status" class="tool-host__status">
                if s.timed_out && !connected {
                    <div data-testid="tool-not-connected">
                        <Alert kind={AlertKind::Warning}
                            header={Some(AttrValue::Static("Tool has not connected to the console"))}
                            action={Some(reload_button)}>
                            { "The tool may still work, but scope, theme, breadcrumbs and notifications are not shared with it." }
                        </Alert>
                    </div>
                }
            </div>
            if !connected && !s.timed_out {
                <div class="tool-host__loading" data-testid="tool-loading">
                    <Spinner label="Connecting to tool" />
                </div>
            }
            <div class="tool-host__frame-wrap" role="group" aria-label={label}
                tabindex="-1" data-refocus=""
                aria-busy={(!connected && !s.timed_out).to_string()}>
                <iframe key={format!("{id}-{}", s.generation)} ref={frame}
                    id={FRAME_ID} class="tool-frame" data-testid="tool-frame"
                    sandbox={SANDBOX} allow="" referrerpolicy="no-referrer"
                    {title} src={url.clone()} {onload} />
            </div>
        </>
    }
}

#[derive(Properties, PartialEq)]
pub struct ToolHostProps {
    pub id: AttrValue,
}

/// Tool container page. Keyed by tool id in the router, so an id change
/// tears this down and launches the new tool.
#[function_component]
pub fn ToolHost(p: &ToolHostProps) -> Html {
    let shell = use_context::<ShellCtx>();
    let session = use_context::<SessionCtx>();
    let tools = use_context::<ToolsCtx>();
    let scope = use_context::<ScopeCtx>()
        .map(|s| s.scope)
        .unwrap_or_default();
    let theme: AttrValue = use_context::<ThemeCtx>()
        .map(|t| t.pref.as_str())
        .unwrap_or("system")
        .into();
    let location = use_location();
    let state = use_reducer(HostState::default);
    let frame = use_node_ref();
    let latest = use_mut_ref(Latest::default);

    let id = p.id.to_string();
    let tool = tools.as_ref().and_then(|t| t.find(&id)).cloned();
    let (tools_list, tools_phase) = match tools.as_ref().map(|t| &t.state) {
        Some(ToolsState::Ready(l)) => (Some(l.clone()), 1u8),
        Some(ToolsState::Failed) => (None, 2),
        _ => (None, 0),
    };
    let flash: Flash = shell.as_ref().map(|s| s.flash.clone()).unwrap_or_default();
    let sess = session.as_ref().and_then(|s| s.session.clone());
    let busy = session.as_ref().is_some_and(|s| s.busy);
    let tpath = location
        .as_ref()
        .and_then(|l| tool_path_from_location(&id, l.path(), l.query_str(), l.hash()))
        .unwrap_or_else(|| "/".into());

    {
        let mut l = latest.borrow_mut();
        l.id.clone_from(&id);
        l.tool.clone_from(&tool);
        l.tools = tools_list;
        l.scope = scope.clone();
        l.theme = theme.clone();
        l.session = sess.clone();
        l.tool_path.clone_from(&tpath);
        l.generation = state.generation;
        l.connected = state.connected;
        l.frame = frame.clone();
        l.flash = flash.clone();
        l.keepalive = session
            .as_ref()
            .map(|s| s.keepalive.clone())
            .unwrap_or_default();
        l.dispatch = Some(state.dispatcher());
    }

    {
        let (latest, flash) = (latest.clone(), flash.clone());
        use_effect_with((), move |_| mount_host(&latest, &flash));
    }
    {
        let deps = LaunchDeps {
            id: id.clone(),
            generation: state.generation,
            tools_phase,
            csrf: sess.as_ref().map(|s| s.csrf_token.clone()),
            session_failed: session.as_ref().is_some_and(|s| s.failed),
        };
        let waiting = state.phase == Phase::Waiting;
        let (tool, tpath, tools, flash) =
            (tool.clone(), tpath.clone(), tools.clone(), flash.clone());
        let dispatch = state.dispatcher();
        // Re-run only when an input that can unblock the launch changes
        // (the csrf value itself changes on keepalive; presence is enough).
        let key = (
            deps.id.clone(),
            deps.generation,
            deps.tools_phase,
            deps.csrf.is_some(),
            deps.session_failed,
        );
        use_effect_with(key, move |_| {
            start_launch(&deps, waiting, tool, &tpath, tools, &flash, dispatch)
        });
    }
    {
        let (latest, flash) = (latest.clone(), flash.clone());
        let active = tool.is_some();
        use_effect_with((active, state.generation), move |(active, _)| {
            listen_messages(&latest, &flash, *active)
        });
    }
    {
        let latest = latest.clone();
        let o = Outbound {
            connected: state.connected,
            scope: tool
                .as_ref()
                .map(|t| scope_for(t, &scope))
                .unwrap_or_default(),
            theme,
            path: tpath,
        };
        use_effect_with(o, move |o| sync_outbound(&mut latest.borrow_mut(), o));
    }
    {
        let latest = latest.clone();
        use_effect_with((session_ptr(&sess), busy), move |(ptr, busy)| {
            answer_refresh(&mut latest.borrow_mut(), *ptr, *busy)
        });
    }
    {
        let dispatch = state.dispatcher();
        use_effect_with((state.refocus, state.phase.kind()), move |(on, kind)| {
            if *on {
                refocus(*kind, &dispatch);
            }
        });
    }
    {
        let meta = page_meta(tool.as_ref(), &id, &state, &scope);
        let set_page = shell.as_ref().map(|s| s.set_page.clone());
        use_effect_with((meta, set_page), |(m, sp)| {
            if let Some(sp) = sp {
                sp.emit(m.clone());
            }
        });
    }

    let reload_button = {
        let dispatch = state.dispatcher();
        let onclick = Callback::from(move |_: MouseEvent| dispatch.dispatch(Act::Reload));
        html! {
            <Button {onclick} testid={AttrValue::Static("tool-reload")}>{ "Reload tool" }</Button>
        }
    };
    let badges = (!state.badges.is_empty()).then(|| html! {
        <ul class="tool-host__badges" aria-label="Tool status" data-testid="tool-badges">
            { for state.badges.iter().map(|(k, c)| html! {
                <li key={k.clone()}><Badge color={BadgeColor::Blue}>{ format!("{k}: {c}") }</Badge></li>
            }) }
        </ul>
    });
    let body = match &state.phase {
        Phase::NotFound => not_found_panel(&scope),
        Phase::Error(msg) => html! {
            <div role="alert" tabindex="-1" data-refocus="" data-testid="tool-error">
                <Alert kind={AlertKind::Error} header={Some(AttrValue::Static("The tool could not be opened"))}
                    action={Some(reload_button)}>
                    { msg.clone() }
                </Alert>
            </div>
        },
        Phase::Waiting | Phase::Launching => html! {
            <div class="tool-host__loading" aria-busy="true" tabindex="-1" data-refocus=""
                data-testid="tool-loading">
                <Spinner size={SpinnerSize::Large} label="Opening tool" />
            </div>
        },
        Phase::Frame(url) => {
            let title = tool
                .as_ref()
                .map_or_else(|| id.clone(), |t| t.title.clone());
            frame_view(
                &state,
                url,
                &id,
                title,
                frame.clone(),
                reload_button,
                state.dispatcher(),
            )
        }
    };
    html! {
        <div class="tool-host" data-testid="tool-host" data-tool-id={p.id.clone()}
            data-tool-connected={state.connected.to_string()}>
            <div class="cui-visually-hidden" role="status" aria-live="polite" aria-atomic="true"
                data-testid="tool-announcer">{ state.announce.clone() }</div>
            { for badges }
            { body }
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_ring_only_on_the_iframe_stop() {
        assert!(frame_ring_wanted(true, false, true), "Firefox iframe stop");
        assert!(!frame_ring_wanted(true, true, true), ":focus ring applies");
        assert!(
            !frame_ring_wanted(true, false, false),
            "focus inside the tool"
        );
        assert!(!frame_ring_wanted(false, false, true), "focus elsewhere");
    }
    use console_protocol::ToolNavItem;

    const ORIGIN: &str = "https://hello.tools.ops.example.mil";

    fn tool() -> ToolSummary {
        ToolSummary {
            id: "hello".into(),
            title: "Hello Tool".into(),
            category: None,
            icon: None,
            origin: ORIGIN.into(),
            entry: "/".into(),
            nav: vec![ToolNavItem {
                label: "Overview".into(),
                path: "/".into(),
            }],
            scopes: vec!["site".into()],
        }
    }

    fn scope() -> Scope {
        Scope {
            site: Some("hq".into()),
            tenant: Some("ops".into()),
            environment: None,
        }
    }

    #[test]
    fn sandbox_flags_are_exact() {
        let flags: Vec<&str> = SANDBOX.split(' ').collect();
        assert_eq!(
            flags,
            [
                "allow-scripts",
                "allow-same-origin",
                "allow-forms",
                "allow-downloads",
                "allow-popups",
                "allow-modals"
            ]
        );
        assert!(!SANDBOX.contains("top-navigation"));
        assert!(!SANDBOX.contains("escape-sandbox"));
    }

    #[test]
    fn tool_path_from_shell_location() {
        let f = tool_path_from_location;
        assert_eq!(f("hello", "/tools/hello", "", "").as_deref(), Some("/"));
        assert_eq!(f("hello", "/tools/hello/", "", "").as_deref(), Some("/"));
        assert_eq!(
            f("hello", "/tools/hello/a/b", "?site=hq&x=1", "#h").as_deref(),
            Some("/a/b?x=1#h")
        );
        assert_eq!(
            f("hello", "/tools/hello/a", "?site=hq&tenant=ops", "").as_deref(),
            Some("/a")
        );
        assert_eq!(f("hello", "/tools/hellox/a", "", ""), None);
        assert_eq!(f("hello", "/gallery", "", ""), None);
    }

    #[test]
    fn shell_url_round_trips() {
        let s = scope();
        assert_eq!(
            shell_url("hello", "/x?y=1#h", &s),
            "/tools/hello/x?site=hq&tenant=ops&y=1#h"
        );
        assert_eq!(shell_url("hello", "/", &Scope::default()), "/tools/hello/");
        for p in ["/", "/a/b", "/a?x=1", "/a#frag", "/a?x=1&z=2#f"] {
            let url = shell_url("hello", p, &s);
            let (path, rest) = url.split_at(url.find(['?', '#']).unwrap_or(url.len()));
            let (search, hash) = rest.split_at(rest.find('#').unwrap_or(rest.len()));
            assert_eq!(
                tool_path_from_location("hello", path, search, hash).as_deref(),
                Some(p),
                "{p}"
            );
        }
    }

    #[test]
    fn launch_path_rules() {
        assert_eq!(launch_path("/"), Ok(None));
        assert_eq!(launch_path("/x?y=1"), Ok(Some("/x?y=1".into())));
        assert_eq!(launch_path("/a/../b"), Err(()));
        assert_eq!(launch_path("/a\\b"), Err(()));
    }

    #[test]
    fn launch_url_must_be_tool_origin_bootstrap() {
        assert!(accept_launch_url(
            &format!("{ORIGIN}/_console/launch?code=abc"),
            ORIGIN
        ));
        for bad in [
            "https://evil.example/_console/launch?code=abc".to_owned(),
            format!("{ORIGIN}/t/hello/"),
            format!("{ORIGIN}/_console/launchx?code=a"),
            format!("{ORIGIN}.evil.example/_console/launch?code=a"),
            format!("{ORIGIN}@evil.example/_console/launch?code=a"),
            format!("{ORIGIN}/_console/launch?code=a b"),
            "javascript:alert(1)".to_owned(),
        ] {
            assert!(!accept_launch_url(&bad, ORIGIN), "{bad}");
        }
    }

    /// Native stand-in for `tool_wire::decode` (same rules, serde codec).
    fn serde_decode(d: &str) -> Result<Option<ToolToShell>, WireError> {
        ToolToShell::from_json(d).map_err(|e| match e {
            console_protocol::MessageError::TooLarge(_) => WireError::TooLarge,
            console_protocol::MessageError::Malformed(_) => WireError::Malformed,
        })
    }

    fn classify(o: &str, e: &str, src: bool, d: Option<&str>) -> Inbound {
        super::classify(o, e, src, d, serde_decode)
    }

    #[test]
    fn decoder_never_sees_untrusted_data() {
        let seen = std::cell::Cell::new(false);
        let probe = |_: &str| {
            seen.set(true);
            Ok(None)
        };
        let ready = Some(r#"{"type":"Ready","protocol":1}"#);
        super::classify("https://evil.example", ORIGIN, true, ready, probe);
        super::classify(ORIGIN, ORIGIN, false, ready, probe);
        assert!(!seen.get());
    }

    #[test]
    fn classify_checks_origin_then_source_then_data() {
        let ready = r#"{"type":"Ready","protocol":1}"#;
        assert_eq!(
            classify(ORIGIN, ORIGIN, true, Some(ready)),
            Inbound::Msg(ToolToShell::Ready { protocol: 1 })
        );
        assert_eq!(
            classify("https://evil.example", ORIGIN, true, Some(ready)),
            Inbound::Drop(DropReason::Origin)
        );
        assert_eq!(
            classify("null", "null", true, Some(ready)),
            Inbound::Drop(DropReason::Origin)
        );
        assert_eq!(
            classify("", "", true, Some(ready)),
            Inbound::Drop(DropReason::Origin)
        );
        assert_eq!(
            classify(&format!("{ORIGIN}:443"), ORIGIN, true, Some(ready)),
            Inbound::Drop(DropReason::Origin)
        );
        assert_eq!(
            classify(ORIGIN, ORIGIN, false, Some(ready)),
            Inbound::Drop(DropReason::Source)
        );
        assert_eq!(
            classify(ORIGIN, ORIGIN, true, None),
            Inbound::Drop(DropReason::NotString)
        );
        assert_eq!(
            classify(ORIGIN, ORIGIN, true, Some("{")),
            Inbound::Drop(DropReason::Malformed)
        );
        assert_eq!(
            classify(
                ORIGIN,
                ORIGIN,
                true,
                Some(r#"{"type":"Notify","level":"loud","message":"x"}"#)
            ),
            Inbound::Drop(DropReason::Malformed)
        );
        assert_eq!(
            classify(ORIGIN, ORIGIN, true, Some(r#"{"type":"Future"}"#)),
            Inbound::Ignore
        );
        let big = format!(
            r#"{{"type":"SetTitle","title":"{}"}}"#,
            "a".repeat(17 * 1024)
        );
        assert_eq!(
            classify(ORIGIN, ORIGIN, true, Some(&big)),
            Inbound::Drop(DropReason::TooLarge)
        );
    }

    #[test]
    fn breadcrumb_mapping() {
        let items = clean_crumbs(vec![
            ("Reports".into(), "/reports".into()),
            ("Group".into(), "".into()),
            ("Evil".into(), "//evil.example".into()),
            ("JS".into(), "javascript:alert(1)".into()),
            ("  ".into(), "/x".into()),
        ]);
        assert_eq!(items.len(), 4, "empty label dropped");
        let c = breadcrumbs(&tool(), &items, &scope());
        let hrefs: Vec<&str> = c.iter().map(|n| n.href.as_str()).collect();
        assert_eq!(
            hrefs,
            [
                "/",
                "/tools/hello/?site=hq&tenant=ops",
                "/tools/hello/reports?site=hq&tenant=ops",
                "",
                "",
                ""
            ]
        );
        let many: Vec<(String, String)> =
            (0..20).map(|i| (format!("c{i}"), String::new())).collect();
        assert_eq!(clean_crumbs(many).len(), MAX_CRUMBS);
    }

    #[test]
    fn badges_upsert_remove_and_cap() {
        let mut b = Vec::new();
        apply_badge(&mut b, "alerts", Some(3));
        apply_badge(&mut b, "alerts", Some(5));
        assert_eq!(b, [("alerts".to_owned(), 5)]);
        apply_badge(&mut b, "alerts", Some(0));
        assert!(b.is_empty());
        apply_badge(&mut b, "\u{0}", Some(1));
        assert!(b.is_empty());
        for i in 0..20 {
            apply_badge(&mut b, &format!("k{i}"), Some(1));
        }
        assert_eq!(b.len(), MAX_BADGES);
        apply_badge(&mut b, "k0", None);
        assert_eq!(b.len(), MAX_BADGES - 1);
    }

    #[test]
    fn notify_rate_limit() {
        let mut b = Burst::default();
        for i in 0..NOTIFY_BURST {
            assert_eq!(b.check(i as f64), Verdict::Allow);
        }
        assert_eq!(b.check(100.0), Verdict::DropAndNotice);
        assert_eq!(b.check(200.0), Verdict::Drop);
        assert_eq!(b.check(NOTIFY_WINDOW_MS + 1.0), Verdict::Allow);
        assert_eq!(
            notify_text("Hello", &"x".repeat(600)).chars().count(),
            "Hello: ".len() + MAX_NOTIFY_CHARS + 1
        );
        assert_eq!(notify_text("Hello", "<b>hi</b>\n"), "Hello: <b>hi</b>");
    }

    #[test]
    fn reducer_ignores_stale_generations() {
        let s = Rc::new(HostState::default());
        let s = s.reduce(Act::Reload);
        assert_eq!(s.generation, 1);
        let s = s.reduce(Act::Phase(0, Phase::NotFound));
        assert_eq!(s.phase, Phase::Waiting);
        let s = s.reduce(Act::Connected(1));
        assert!(s.connected);
        let s = s.reduce(Act::TimedOut(1));
        assert!(!s.timed_out, "no timeout banner once connected");
    }

    #[test]
    fn reload_requests_refocus_until_settled() {
        let s = Rc::new(HostState::default());
        assert!(!s.refocus, "first launch never moves focus");
        let s = s.reduce(Act::Reload);
        assert!(s.refocus);
        assert_eq!(s.phase.kind(), 0);
        let s = s.reduce(Act::Refocused);
        assert!(!s.refocus);
    }

    #[test]
    fn title_announced_only_when_focus_in_frame_and_changed() {
        let s = Rc::new(HostState::default());
        let s = s.reduce(Act::Title("A".into(), false));
        assert_eq!(s.announce, "");
        let s = s.reduce(Act::Title("B".into(), true));
        assert_eq!(s.announce, "B");
        let s = s.reduce(Act::Title("C".into(), false));
        assert_eq!(s.announce, "B", "unchanged when focus is in the shell");
        assert_eq!(s.title.as_deref(), Some("C"));
    }

    #[test]
    fn not_found_meta_has_error_heading() {
        let mut st = HostState {
            phase: Phase::NotFound,
            ..HostState::default()
        };
        let m = page_meta(Some(&tool()), "hello", &st, &scope());
        assert_eq!(m.title, "Tool not found");
        assert_eq!(m.document_title, None);
        st.phase = Phase::Waiting;
        assert_eq!(
            page_meta(Some(&tool()), "hello", &st, &scope()).title,
            "Hello Tool"
        );
    }

    #[test]
    fn protocol_negotiation() {
        assert_eq!(negotiate(0), 1);
        assert_eq!(negotiate(1), 1);
        assert_eq!(negotiate(7), PROTOCOL_VERSION);
    }
}

/// Browser checks for the `event.source` comparison used by the listener.
#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;
    use wasm_bindgen_test::*;
    use web_sys::wasm_bindgen::JsCast;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn source_identity_distinguishes_frames() {
        let doc = web_sys::window().unwrap().document().unwrap();
        let mk = || {
            let f: web_sys::HtmlIFrameElement =
                doc.create_element("iframe").unwrap().dyn_into().unwrap();
            doc.body().unwrap().append_child(&f).unwrap();
            f
        };
        let (a, b) = (mk(), mk());
        let (wa, wb) = (a.content_window().unwrap(), b.content_window().unwrap());
        assert!(js_sys::Object::is(&wa, &a.content_window().unwrap()));
        assert!(!js_sys::Object::is(&wa, &wb));
        let top = web_sys::window().unwrap();
        assert!(!js_sys::Object::is(&top, &wa));
        a.remove();
        b.remove();
    }

    #[wasm_bindgen_test]
    fn notify_text_is_rendered_as_text() {
        let doc = web_sys::window().unwrap().document().unwrap();
        let host = doc.create_element("div").unwrap();
        host.set_text_content(Some(&notify_text("T", "<img src=x onerror=alert(1)>")));
        assert_eq!(host.children().length(), 0);
    }
}
