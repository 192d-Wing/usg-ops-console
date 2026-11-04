//! Console shell entry point: chrome, routing, flash notifications, and theme.
//! Tool registry (`/api/tools`) and the iframe tool container arrive in M4.

mod activity;
mod api;
mod chrome;
mod ctx;
mod demo;
#[cfg(feature = "gallery")]
mod gallery;
mod idle;
mod scope;
mod session;
mod theme;
mod tool_host;
mod tool_wire;
mod tools;

pub use ctx::{PageMeta, ScopeCtx, SessionCtx, ShellCtx, ThemeCtx, use_page_meta};
use session::SessionInfo;
use tools::ToolsCtx;

use console_protocol::Scope;
use console_ui::{
    AlertKind, Breadcrumbs, Container, FlashId, FlashItem, Flashbar, Header, HeaderVariant,
    NavLink, NavToggle, SideNavItem, SideNavigation, TopNavigation, flash_dismiss, flash_push,
};
use std::rc::Rc;
use yew::prelude::*;
use yew_router::history::{BrowserHistory, History};
use yew_router::prelude::*;

/// Id of the side navigation wrapper (target of the nav toggle).
pub const SIDE_NAV_REGION_ID: &str = "side-nav-region";
/// Width at and above which the side nav is always shown (A11Y-008).
const NAV_BREAKPOINT: &str = "(min-width: 45rem)";

/// Shell routes. `Routable` is implemented by hand (not derived) so the
/// derive's `urlencoding` round-trip of params is not in the bundle
/// (PERF-001). Params are taken verbatim: `id` must pass `valid_id`
/// (lowercase ASCII, so never percent-encoded) and `ToolHost` reads the tool
/// path from `location`, not from `path`.
#[derive(Clone, PartialEq, Debug)]
pub enum Route {
    /// `/`
    Home,
    /// `/tools/:id` - tool container; deep links and reload relaunch at the path.
    Tool { id: String },
    /// `/tools/:id/*path`
    ToolPath { id: String, path: String },
    /// `/gallery`
    #[cfg(feature = "gallery")]
    Gallery,
    /// `/404` and anything unmatched.
    NotFound,
}

impl Routable for Route {
    fn from_path(path: &str, params: &std::collections::HashMap<&str, &str>) -> Option<Self> {
        let p = |k: &str| params.get(k).map(|v| (*v).to_owned());
        match path {
            "/" => Some(Route::Home),
            "/tools/:id" => Some(Route::Tool { id: p("id")? }),
            "/tools/:id/*path" => Some(Route::ToolPath {
                id: p("id")?,
                path: p("path")?,
            }),
            #[cfg(feature = "gallery")]
            "/gallery" => Some(Route::Gallery),
            "/404" => Some(Route::NotFound),
            _ => None,
        }
    }

    fn to_path(&self) -> String {
        match self {
            Route::Home => "/".to_owned(),
            Route::Tool { id } => format!("/tools/{id}"),
            Route::ToolPath { id, path } => format!("/tools/{id}/{path}"),
            #[cfg(feature = "gallery")]
            Route::Gallery => "/gallery".to_owned(),
            Route::NotFound => "/404".to_owned(),
        }
    }

    fn routes() -> Vec<&'static str> {
        vec![
            "/",
            "/tools/:id",
            "/tools/:id/*path",
            #[cfg(feature = "gallery")]
            "/gallery",
            "/404",
        ]
    }

    fn not_found_route() -> Option<Self> {
        Some(Route::NotFound)
    }

    fn recognize(pathname: &str) -> Option<Self> {
        thread_local! {
            static ROUTER: yew_router::__macro::Router = yew_router::__macro::build_router::<Route>();
        }
        ROUTER.with(|r| yew_router::__macro::recognize_with_router(r, pathname))
    }
}

/// Id of the chrome region. Modals pass it as `inert_root` and must be
/// rendered outside it, into `#modal-root` (see [`modal_root`]).
pub const APP_CHROME_ID: &str = "app-chrome";

/// Host element for modals, outside the region that is made inert.
pub fn modal_root() -> Option<web_sys::Element> {
    web_sys::window()?
        .document()?
        .get_element_by_id("modal-root")
}

/// Page name used for `document.title` ("<page> - USG Ops Console").
fn page_title(route: Option<&Route>) -> &'static str {
    match route {
        Some(Route::Home) | None => "Home",
        Some(Route::Tool { .. } | Route::ToolPath { .. }) => "Tool",
        #[cfg(feature = "gallery")]
        Some(Route::Gallery) => "Component gallery",
        Some(Route::NotFound) => "Not found",
    }
}

fn document() -> Result<web_sys::Document, &'static str> {
    web_sys::window()
        .and_then(|w| w.document())
        .ok_or("Document not available")
}

/// `document.title` from the page meta (WCAG 2.4.2).
fn set_title(page: &str) -> Result<(), &'static str> {
    document()?.set_title(&format!("{page} - USG Ops Console"));
    Ok(())
}

/// On client-side navigation, move focus to `<main>` so screen-reader users
/// hear the new page (WCAG 2.4.3).
fn focus_main() -> Result<(), &'static str> {
    use web_sys::wasm_bindgen::JsCast;
    document()?
        .get_element_by_id("main-content")
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        .ok_or("Main content region not found")?
        .focus()
        .map_err(|_| "Could not move focus to main content")
}

fn switch(route: Route) -> Html {
    match route {
        Route::Home => html! {
            <Container header={html! { <h2>{ "Welcome" }</h2> }}>
                <p>{ "Open a tool from the Services menu or the side navigation." }</p>
            </Container>
        },
        Route::Tool { id } | Route::ToolPath { id, .. } => html! {
            <tool_host::ToolHost key={id.as_str()} id={AttrValue::from(id.clone())} />
        },
        #[cfg(feature = "gallery")]
        Route::Gallery => html! { <gallery::Gallery /> },
        Route::NotFound => html! {
            <Container>
                <p>{ "This page does not exist." }</p>
            </Container>
        },
    }
}

#[derive(Properties, PartialEq)]
struct ThemeToggleProps {
    pref: theme::ThemePref,
    on_change: Callback<theme::ThemePref>,
    on_error: Callback<(AlertKind, AttrValue)>,
}

#[function_component]
fn ThemeToggle(props: &ThemeToggleProps) -> Html {
    let onclick = {
        let pref = props.pref;
        let on_change = props.on_change.clone();
        let on_error = props.on_error.clone();
        Callback::from(move |_: MouseEvent| {
            let next = theme::next(pref);
            if let Err(e) = theme::save(next) {
                on_error.emit((AlertKind::Warning, format!("Theme not saved: {e}").into()));
            }
            on_change.emit(next);
        })
    };
    html! {
        <button type="button" class="topbar__theme" data-testid="theme-toggle"
            title="Cycle theme: system, light, dark" {onclick}>
            { theme::label(props.pref) }
        </button>
    }
}

/// Theme preference, applied to `<html data-theme>` and shared with tools
/// through [`ThemeCtx`].
#[hook]
fn use_theme(
    flash: Callback<(AlertKind, AttrValue)>,
) -> (theme::ThemePref, Callback<theme::ThemePref>) {
    let pref = use_state(theme::load);
    use_effect_with(*pref, move |p| {
        if let Err(e) = theme::apply(*p) {
            flash.emit((AlertKind::Error, format!("Theme: {e}").into()));
        }
    });
    let set = use_callback(pref.setter(), |p, set| set.set(p));
    (*pref, set)
}

fn nav_items(tools: &tools::ToolsState, active_tool: Option<&str>) -> Vec<SideNavItem> {
    #[allow(unused_mut)]
    let mut items = vec![SideNavItem::Link(NavLink::new("Home", "/"))];
    items.extend(tools::nav_sections(tools, active_tool));
    #[cfg(feature = "gallery")]
    items.push(SideNavItem::Section {
        text: "Developer".into(),
        items: vec![NavLink::new("Component gallery", "/gallery")],
    });
    items
}

/// Forces `url` (path + query/hash) to be same-origin-relative: a run of
/// leading `/` or `\\` collapses to one `/`, and anything not starting with a
/// slash becomes `/`. Without this, a crafted pathname like `//evil.example/`
/// resolves cross-origin and `pushState` throws, which gloo-history turns
/// into a panic (SEC-001; NIST 800-53 SI-10, SI-11).
pub(crate) fn same_origin_url(url: &str) -> String {
    let rest = url.trim_start_matches(['/', '\\']);
    if rest.len() == url.len() {
        return "/".to_owned();
    }
    format!("/{rest}")
}

/// Push (or replace) a same-origin URL through the router's shared history so
/// `use_location` subscribers are notified.
pub(crate) fn history_go(url: &str, replace: bool) {
    let url = same_origin_url(url);
    let h = BrowserHistory::new();
    if replace {
        h.replace(url);
    } else {
        h.push(url);
    }
}

/// Current `location.pathname`, `search` and `hash`, read at call time so
/// callbacks need not be rebuilt on every location change (PERF-003).
pub(crate) fn current_location() -> (String, String, String) {
    web_sys::window()
        .map(|w| w.location())
        .map(|l| {
            (
                l.pathname().unwrap_or_else(|_| "/".to_owned()),
                l.search().unwrap_or_default(),
                l.hash().unwrap_or_default(),
            )
        })
        .unwrap_or_else(|| ("/".to_owned(), String::new(), String::new()))
}

/// Flashbar state. A reducer gives a stable dispatcher, so the `flash`
/// callback in [`ShellCtx`] never changes identity (PERF-003/004).
#[derive(Clone, Default, PartialEq)]
struct Flashes {
    items: Vec<FlashItem>,
    next_id: u64,
}

enum FlashAction {
    Push(AlertKind, AttrValue),
    Dismiss(FlashId),
}

impl Reducible for Flashes {
    type Action = FlashAction;
    fn reduce(self: Rc<Self>, action: FlashAction) -> Rc<Self> {
        let mut s = Rc::unwrap_or_clone(self);
        match action {
            FlashAction::Push(kind, content) => {
                s.next_id += 1;
                flash_push(
                    &mut s.items,
                    FlashItem {
                        id: FlashId(s.next_id),
                        kind,
                        header: None,
                        content,
                        dismissible: true,
                    },
                );
            }
            FlashAction::Dismiss(id) => {
                flash_dismiss(&mut s.items, id);
            }
        }
        Rc::new(s)
    }
}

pub(crate) fn focus_selector(sel: &str) {
    use web_sys::wasm_bindgen::JsCast;
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.query_selector(sel).ok().flatten())
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = el.focus();
    }
}

/// `matchMedia("(min-width: 45rem)")` state, kept in sync via its change event.
#[hook]
fn use_wide(flash: Callback<(AlertKind, AttrValue)>) -> bool {
    // Initial value from matchMedia so narrow first paint never flashes the
    // side nav open (A11Y-005).
    let wide = use_state(|| {
        web_sys::window()
            .and_then(|w| w.match_media(NAV_BREAKPOINT).ok().flatten())
            .is_none_or(|m| m.matches())
    });
    {
        let wide = wide.clone();
        use_effect_with((), move |_| {
            use web_sys::wasm_bindgen::{JsCast, closure::Closure};
            let mql = web_sys::window().and_then(|w| w.match_media(NAV_BREAKPOINT).ok().flatten());
            let mut cleanup: Option<(web_sys::MediaQueryList, Closure<dyn Fn()>)> = None;
            match mql {
                Some(mql) => {
                    wide.set(mql.matches());
                    let m = mql.clone();
                    let cb = Closure::<dyn Fn()>::new(move || wide.set(m.matches()));
                    if mql
                        .add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())
                        .is_err()
                    {
                        flash.emit((
                            AlertKind::Warning,
                            "Layout: width changes not tracked".into(),
                        ));
                    }
                    cleanup = Some((mql, cb));
                }
                None => flash.emit((
                    AlertKind::Warning,
                    "Layout: media queries unavailable".into(),
                )),
            }
            move || {
                if let Some((mql, cb)) = cleanup {
                    let _ = mql
                        .remove_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
                }
            }
        });
    }
    *wide
}

/// `window.location.origin` (empty if unavailable, which matches nothing).
pub(crate) fn own_origin() -> String {
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default()
}

/// Reads `console_notice`, set by the gateway on `return_to` after a sign-in
/// that followed an expiry or revocation (CONTRACT DELTA 8). No side effects:
/// [`strip_url_notice`] removes it from the URL after the first render.
fn read_url_notice() -> Option<session::LoginNotice> {
    let (_, search, _) = current_location();
    session::take_param(&search, session::NOTICE_PARAM)
        .0
        .and_then(|v| session::LoginNotice::parse(&v))
}

/// Strips only `console_notice` from the URL, without a new history entry.
fn strip_url_notice() {
    let (path, search, hash) = current_location();
    if let (Some(_), rest) = session::take_param(&search, session::NOTICE_PARAM) {
        history_go(&format!("{path}{rest}{hash}"), true);
    }
}

/// Session snapshot plus the client clock when it arrived (for skew).
#[derive(Clone, PartialEq)]
enum SessionState {
    Loading,
    Ready {
        info: Rc<SessionInfo>,
        received_at: i64,
    },
    /// Non-401 failure, already shown in the flashbar.
    Failed,
}

fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

/// Loads `/api/session` on mount and when the tab becomes visible (so a
/// background tab notices a logout from another tab), and provides
/// keepalive / sign-out. A 401 navigates to login inside `api`.
#[hook]
fn use_session(
    flash: Callback<(AlertKind, AttrValue)>,
) -> (SessionState, SessionCtx, Option<AttrValue>) {
    let state = use_state(|| SessionState::Loading);
    let busy = use_state(|| false);
    let signing_out = use_state(|| false);
    let keepalive_error = use_state(|| None::<AttrValue>);
    // Explanation for the redirect that brought the user here (A11Y-7,
    // NIST-001); shown once the new session has loaded.
    let notice = use_mut_ref(read_url_notice);
    use_effect_with((), |_| strip_url_notice());
    let load = use_callback(
        (state.setter(), flash.clone(), notice),
        |(), (set, flash, notice)| {
            let (set, flash, notice) = (set.clone(), flash.clone(), notice.clone());
            yew::platform::spawn_local(async move {
                match api::get_session().await {
                    Ok(info) => {
                        set.set(SessionState::Ready {
                            info: Rc::new(info),
                            received_at: now_ms(),
                        });
                        if let Some(n) = notice.borrow_mut().take() {
                            flash.emit((AlertKind::Info, n.message().into()));
                        }
                    }
                    Err(e) => {
                        if let Some(m) = e.message("Session") {
                            flash.emit((AlertKind::Error, m.into()));
                            set.set(SessionState::Failed);
                        }
                    }
                }
            });
        },
    );
    {
        let load = load.clone();
        let flash = flash.clone();
        use_effect_with((), move |_| {
            use web_sys::wasm_bindgen::{JsCast, closure::Closure};
            load.emit(());
            let doc = web_sys::window().and_then(|w| w.document());
            let cb = Closure::<dyn Fn()>::new(move || {
                let visible = web_sys::window()
                    .and_then(|w| w.document())
                    .is_some_and(|d| d.visibility_state() == web_sys::VisibilityState::Visible);
                if visible {
                    load.emit(());
                }
            });
            let registered = doc.as_ref().is_some_and(|d| {
                d.add_event_listener_with_callback("visibilitychange", cb.as_ref().unchecked_ref())
                    .is_ok()
            });
            if !registered {
                flash.emit((
                    AlertKind::Warning,
                    "Session: sign-out in other tabs is detected only on the next request".into(),
                ));
            }
            move || {
                if let Some(d) = doc {
                    let _ = d.remove_event_listener_with_callback(
                        "visibilitychange",
                        cb.as_ref().unchecked_ref(),
                    );
                }
            }
        });
    }
    let retry = use_callback((state.setter(), load.clone()), |(), (set, load)| {
        set.set(SessionState::Loading);
        load.emit(());
    });
    let csrf = match &*state {
        SessionState::Ready { info, .. } => Some(AttrValue::from(info.csrf_token.clone())),
        _ => None,
    };
    let keepalive = use_callback(
        (
            csrf.clone(),
            state.setter(),
            busy.setter(),
            keepalive_error.setter(),
        ),
        |(), (csrf, set, set_busy, set_err)| {
            let Some(csrf) = csrf.clone() else { return };
            let (set, set_busy, set_err) = (set.clone(), set_busy.clone(), set_err.clone());
            set_busy.set(true);
            set_err.set(None);
            yew::platform::spawn_local(async move {
                match api::keepalive(&csrf).await {
                    Ok(info) => set.set(SessionState::Ready {
                        info: Rc::new(info),
                        received_at: now_ms(),
                    }),
                    // 401 (revoked, de-roled, expired): `api` is already
                    // navigating to login with a notice for the next load.
                    Err(e) => {
                        set_err.set(e.message("Could not extend your session").map(|m| {
                            AttrValue::from(format!("{m}. Try again, or save your work."))
                        }))
                    }
                }
                set_busy.set(false);
            });
        },
    );
    let sign_out = use_callback(
        (csrf, busy.setter(), signing_out.setter(), flash),
        |(), (csrf, set_busy, set_signing_out, flash)| {
            let Some(csrf) = csrf.clone() else {
                flash.emit((AlertKind::Error, "Sign out: session not loaded yet".into()));
                return;
            };
            let (set_busy, set_signing_out, flash) =
                (set_busy.clone(), set_signing_out.clone(), flash.clone());
            set_busy.set(true);
            set_signing_out.set(true);
            yew::platform::spawn_local(async move {
                match api::logout(&csrf).await {
                    Ok(r) => match (
                        session::accept_logout_redirect(
                            &r.redirect,
                            r.end_session_origin.as_deref(),
                            &own_origin(),
                        ),
                        web_sys::window(),
                    ) {
                        // `replace`: the end-session URL (with id_token_hint)
                        // stays out of session history (SEC-003).
                        (Some(url), Some(w)) => {
                            if w.location().replace(url).is_err() {
                                flash.emit((
                                    AlertKind::Error,
                                    "Sign out: could not open the sign-out page".into(),
                                ));
                            } else {
                                return;
                            }
                        }
                        _ => flash.emit((
                            AlertKind::Error,
                            "Sign out: the server returned an invalid redirect. You are signed out of the console; close the browser to end single sign-on.".into(),
                        )),
                    },
                    Err(e) => {
                        if let Some(m) = e.message("Sign out failed") {
                            flash.emit((AlertKind::Error, m.into()));
                        }
                    }
                }
                set_busy.set(false);
                set_signing_out.set(false);
            });
        },
    );
    let session = match &*state {
        SessionState::Ready { info, .. } => Some(info.clone()),
        _ => None,
    };
    let failed = matches!(*state, SessionState::Failed);
    let ctx = use_memo(
        (
            session,
            keepalive,
            sign_out,
            retry,
            *busy,
            *signing_out,
            failed,
        ),
        |(session, keepalive, sign_out, retry, busy, signing_out, failed)| SessionCtx {
            session: session.clone(),
            keepalive: keepalive.clone(),
            sign_out: sign_out.clone(),
            retry: retry.clone(),
            busy: *busy,
            signing_out: *signing_out,
            failed: *failed,
        },
    );
    ((*state).clone(), (*ctx).clone(), (*keepalive_error).clone())
}

#[function_component]
fn Shell() -> Html {
    let route = use_route::<Route>();
    let location = use_location();
    let flashes = use_reducer(Flashes::default);
    // Stable identities: deps are the reducer dispatcher (pointer-stable).
    let flash = use_callback(
        flashes.dispatcher(),
        |(kind, content): (AlertKind, AttrValue), d| d.dispatch(FlashAction::Push(kind, content)),
    );
    let on_dismiss = use_callback(flashes.dispatcher(), |id: FlashId, d| {
        d.dispatch(FlashAction::Dismiss(id))
    });

    // --- Scope: derived from the URL on every location change (incl. popstate).
    let (path, search, hash) = location
        .as_ref()
        .map(|l| {
            (
                l.path().to_owned(),
                l.query_str().to_owned(),
                l.hash().to_owned(),
            )
        })
        .unwrap_or_default();
    let (scope, scope_errors) = scope::parse_query(&search, demo::SITES);
    {
        let flash = flash.clone();
        let (path, search, hash) = (path.clone(), search.clone(), hash.clone());
        let clean = scope.clone();
        use_effect_with(scope_errors, move |errs| {
            if let Some(msg) = scope::error_message(errs) {
                flash.emit((AlertKind::Warning, msg.into()));
                // Drop the rejected keys from the URL without a new entry.
                let q = scope::to_query(&format!("{search}{hash}"), &clean);
                history_go(&format!("{path}{q}"), true);
            }
        });
    }
    // Reads the live URL when called, so it never needs rebuilding.
    let set_scope = use_callback((), |next: Scope, _| {
        let (path, search, hash) = current_location();
        let q = scope::to_query(&format!("{search}{hash}"), &next);
        history_go(&format!("{path}{q}"), false);
    });
    // Same value (and so no consumer re-render) unless the scope changes.
    let scope_ctx = use_memo(scope.clone(), |s| ScopeCtx {
        scope: s.clone(),
        set: set_scope,
    });

    // --- Page meta: stored with the route it was set for, so a route change
    // never shows stale crumbs (children's effects run before ours).
    let route_path: AttrValue = route
        .as_ref()
        .map(|r| r.to_path())
        .unwrap_or_default()
        .into();
    let page = use_state(|| (AttrValue::default(), PageMeta::simple("Home")));
    let set_page = use_callback(
        (route_path.clone(), page.setter()),
        |m: PageMeta, (rp, set)| set.set((rp.clone(), m)),
    );
    let meta = if page.0 == route_path {
        page.1.clone()
    } else {
        PageMeta::simple(page_title(route.as_ref()))
    };
    {
        let flash = flash.clone();
        let doc_title = meta
            .document_title
            .clone()
            .unwrap_or_else(|| meta.title.clone());
        use_effect_with(doc_title, move |t| {
            if let Err(e) = set_title(t) {
                flash.emit((AlertKind::Error, e.into()));
            }
        });
    }
    {
        let flash = flash.clone();
        let first = use_mut_ref(|| true);
        use_effect_with(route.clone(), move |_| {
            // Initial load: don't steal focus from the skip link.
            if !std::mem::replace(&mut *first.borrow_mut(), false)
                && let Err(e) = focus_main()
            {
                flash.emit((AlertKind::Error, e.into()));
            }
        });
    }

    // --- Side navigation collapse below 45rem (A11Y-008).
    let (session_state, session_ctx, keepalive_error) = use_session(flash.clone());
    let idle_watch = match &session_state {
        SessionState::Ready { info, received_at } => html! {
            <idle::IdleWatch session={info.clone()} received_at={*received_at}
                busy={session_ctx.busy} {keepalive_error}
                on_keepalive={session_ctx.keepalive.clone()}
                on_sign_out={session_ctx.sign_out.clone()} flash={flash.clone()} />
        },
        // Same (empty) live region, so it exists before any announcement.
        _ => html! {
            <div class="cui-visually-hidden" role="status" aria-live="polite" aria-atomic="true"
                data-testid="idle-announcer"></div>
        },
    };

    let wide = use_wide(flash.clone());
    let nav_open = use_state(|| false);
    let nav_hidden = !wide && !*nav_open;
    {
        let open = *nav_open && !wide;
        use_effect_with(open, |open| {
            if *open {
                focus_selector("#side-nav-region a");
            }
        });
    }
    let on_toggle = {
        let nav_open = nav_open.clone();
        Callback::from(move |_| nav_open.set(!*nav_open))
    };
    let on_nav_key = {
        let nav_open = nav_open.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Escape" && !wide && *nav_open {
                e.prevent_default();
                nav_open.set(false);
                focus_selector("[data-testid=\"nav-toggle\"]");
            }
        })
    };

    // Client-side navigation for compiled-in hrefs; keeps the scope query.
    let onfollow = use_callback(
        (scope::to_query("", &scope), nav_open.setter()),
        |(e, href): (MouseEvent, AttrValue), (q, set_open)| {
            let same_origin = href.starts_with('/') && !href.starts_with("//");
            if same_origin && Route::recognize(&href).is_some() {
                e.prevent_default();
                set_open.set(false);
                let path = href.split(['?', '#']).next().unwrap_or("/");
                history_go(&format!("{path}{q}"), false);
            }
        },
    );
    let brand_click = {
        let onfollow = onfollow.clone();
        Callback::from(move |e: MouseEvent| {
            if e.button() == 0 && !e.ctrl_key() && !e.meta_key() && !e.shift_key() && !e.alt_key() {
                onfollow.emit((e, AttrValue::Static("/")));
            }
        })
    };
    let active_tool: Option<String> = match &route {
        Some(Route::Tool { id } | Route::ToolPath { id, .. }) => Some(id.clone()),
        _ => None,
    };
    activity::use_activity_tracker(flash.clone());
    let session_ready = matches!(session_state, SessionState::Ready { .. });
    let tools_ctx = tools::use_tools(session_ready, flash.clone());
    let (theme_pref, set_theme) = use_theme(flash.clone());
    let theme_ctx = ThemeCtx { pref: theme_pref };
    let skip_to_tool = active_tool.is_some().then(|| {
        // Focus the frame and ask the tool for its first control (A11Y-001).
        let onclick = Callback::from(|e: MouseEvent| {
            e.prevent_default();
            if !tool_host::enter_tool(console_protocol::FocusEdge::First) {
                focus_selector(&format!("#{}", tool_host::FRAME_ID));
            }
        });
        html! {
            <a class="skip-link" href={format!("#{}", tool_host::FRAME_ID)} {onclick}
                data-testid="skip-to-tool">{ "Skip to tool content" }</a>
        }
    });
    let side_items = nav_items(&tools_ctx.state, active_tool.as_deref());
    let active_href: Option<AttrValue> = route.map(|r| r.to_path().into());
    let ctx = use_memo((flash.clone(), set_page), |(flash, set_page)| ShellCtx {
        flash: flash.clone(),
        set_page: set_page.clone(),
    });

    html! {
        <ContextProvider<ShellCtx> context={(*ctx).clone()}>
        <ContextProvider<SessionCtx> context={session_ctx}>
        <ContextProvider<ScopeCtx> context={(*scope_ctx).clone()}>
        <ContextProvider<ToolsCtx> context={tools_ctx}>
        <ContextProvider<ThemeCtx> context={theme_ctx}>
            <div class="app" id={APP_CHROME_ID}>
                <a class="skip-link" href="#main-content">{ "Skip to main content" }</a>
                { for skip_to_tool }
                <TopNavigation
                    brand={html! { <a href="/" onclick={brand_click}>{ "USG Ops Console" }</a> }}
                    start={html! {
                        <>
                            <span class="nav-toggle-slot">
                                <NavToggle expanded={!nav_hidden} controls={SIDE_NAV_REGION_ID} {on_toggle} />
                            </span>
                            <chrome::ServiceMenu onfollow={onfollow.clone()} />
                        </>
                    }}
                    search={html! { <chrome::GlobalSearch flash={flash.clone()} /> }}
                    utilities={html! {
                        <>
                            <ThemeToggle pref={theme_pref} on_change={set_theme} on_error={flash.clone()} />
                            <chrome::UserMenu flash={flash.clone()} />
                        </>
                    }}
                />
                <div class="scopebar">
                    <chrome::ScopePicker />
                    <span class="scopebar__meta">
                        { format!("SDK protocol v{}", console_protocol::PROTOCOL_VERSION) }
                    </span>
                </div>
                <div class="shell-flash" data-testid="shell-flashbar">
                    <Flashbar items={flashes.items.clone()} {on_dismiss} fallback_focus_id="main-content" />
                </div>
                <div class="layout">
                    <div class="sidenav-region" id={SIDE_NAV_REGION_ID} hidden={nav_hidden}
                        onkeydown={on_nav_key}>
                        <SideNavigation aria_label="Console" items={side_items}
                            {active_href} onfollow={Some(onfollow.clone())} />
                    </div>
                    <main class="content" id="main-content" tabindex="-1">
                        <div class="page-crumbs" data-testid="breadcrumbs">
                            <Breadcrumbs items={meta.breadcrumbs.clone()} onfollow={Some(onfollow)} />
                        </div>
                        <div class="page-header" data-testid="page-header">
                            <Header variant={HeaderVariant::H1} description={meta.description.clone()}>
                                { meta.title.clone() }
                            </Header>
                        </div>
                        <Switch<Route> render={switch} />
                    </main>
                </div>
            </div>
            <div id="modal-root" />
            { idle_watch }
        </ContextProvider<ThemeCtx>>
        </ContextProvider<ToolsCtx>>
        </ContextProvider<ScopeCtx>>
        </ContextProvider<SessionCtx>>
        </ContextProvider<ShellCtx>>
    }
}

#[function_component]
fn App() -> Html {
    html! {
        <BrowserRouter>
            <Shell />
        </BrowserRouter>
    }
}

fn main() {
    yew::Renderer::<App>::new().render();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_origin_url_collapses_leading_slashes() {
        assert_eq!(same_origin_url("/"), "/");
        assert_eq!(same_origin_url("/gallery?site=hq#x"), "/gallery?site=hq#x");
        assert_eq!(same_origin_url("//evil.example/?"), "/evil.example/?");
        assert_eq!(same_origin_url("///evil.example/a"), "/evil.example/a");
        assert_eq!(same_origin_url("/\\evil.example/"), "/evil.example/");
        assert_eq!(same_origin_url("\\\\evil.example"), "/evil.example");
        assert_eq!(same_origin_url("https://evil.example/"), "/");
        assert_eq!(same_origin_url("?site=hq"), "/");
        assert_eq!(same_origin_url(""), "/");
        for u in ["//a", "/\\a", "\\/a", "////a?x#y"] {
            let out = same_origin_url(u);
            assert!(
                out.starts_with('/') && !out[1..].starts_with(['/', '\\']),
                "{u} -> {out}"
            );
        }
    }

    #[test]
    fn tool_routes_recognise_deep_links() {
        let id = |r: Option<Route>| match r {
            Some(Route::Tool { id } | Route::ToolPath { id, .. }) => Some(id),
            _ => None,
        };
        for p in ["/tools/hello", "/tools/hello/", "/tools/hello/a/b"] {
            assert_eq!(id(Route::recognize(p)).as_deref(), Some("hello"), "{p}");
        }
        assert!(matches!(Route::recognize("/tools"), Some(Route::NotFound)));
        assert!(matches!(Route::recognize("/nope"), Some(Route::NotFound)));
        assert!(matches!(Route::recognize("/"), Some(Route::Home)));
        // Params are verbatim (no percent-decoding; PERF-001).
        assert!(matches!(Route::recognize("/tools/a%2Fb"),
            Some(Route::Tool { id }) if id == "a%2Fb"));
        for r in [
            Route::Home,
            Route::Tool { id: "hello".into() },
            Route::ToolPath {
                id: "hello".into(),
                path: "a/b".into(),
            },
            Route::NotFound,
        ] {
            assert_eq!(Route::recognize(&r.to_path()), Some(r.clone()), "{r:?}");
        }
    }

    #[test]
    fn flash_reducer_push_and_dismiss_assign_unique_ids() {
        let s = Rc::new(Flashes::default());
        let s = s.reduce(FlashAction::Push(AlertKind::Info, "a".into()));
        let s = s.reduce(FlashAction::Push(AlertKind::Error, "b".into()));
        let ids: Vec<u64> = s.items.iter().map(|f| f.id.0).collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        let first = s.items[0].id;
        let s = s.reduce(FlashAction::Dismiss(first));
        assert_eq!(s.items.len(), 1);
        assert!(s.items.iter().all(|f| f.id != first));
        // Ids keep increasing after a dismiss (no reuse).
        let s = s.reduce(FlashAction::Push(AlertKind::Info, "c".into()));
        assert!(s.items.iter().all(|f| f.id.0 <= 3) && s.next_id == 3);
    }
}
