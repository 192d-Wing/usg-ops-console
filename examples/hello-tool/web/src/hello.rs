//! The normal tool. Everything identity-related shown here comes from the
//! backend (`api/whoami`, verified assertion), not from `Init.user`, which is
//! display-only; both are shown so E2E can compare them.

use std::rc::Rc;

use console_protocol::{Level, Scope};
use console_sdk::hooks::{
    ConsoleProvider, ConsoleStatus, console_fetch, notify, path_changed, set_breadcrumbs,
    set_title, use_console, use_console_navigate, use_console_scope, use_console_status,
    use_console_theme,
};
use console_ui::{
    AlertKind, Button, Container, Header, HeaderVariant, StatusIndicator, StatusKind, Variant,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{Request, RequestInit};
use yew::prelude::*;

use crate::util::{Flash, location_tool, response_json, use_flash};

const VISITS_KEY: &str = "hello-tool.visits";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Overview,
    Headers,
}

impl Page {
    fn from_path(p: &str) -> Self {
        match p.split(['?', '#']).next().unwrap_or("/") {
            "/headers" => Page::Headers,
            _ => Page::Overview,
        }
    }
    fn path(self) -> &'static str {
        match self {
            Page::Overview => "/",
            Page::Headers => "/headers",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Page::Overview => "Overview",
            Page::Headers => "Request headers",
        }
    }
}

#[function_component]
pub fn Root() -> Html {
    html! { <ConsoleProvider><Hello /></ConsoleProvider> }
}

/// Change the tool-internal route: update this frame's history and tell the
/// shell (`PathChanged`) so the console URL deep-links here.
fn go(id: &str, page: &UseStateHandle<Page>, to: Page, push: bool) {
    if push && let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
        let _ = h.push_state_with_url(
            &wasm_bindgen::JsValue::NULL,
            "",
            Some(&format!("/t/{id}{}", to.path())),
        );
    }
    page.set(to);
    path_changed(to.path());
}

#[function_component]
fn Hello() -> Html {
    let state = use_console();
    let status = use_console_status();
    let scope = use_console_scope();
    let theme = use_console_theme();
    let flash = use_flash();
    let (id, initial) = use_memo((), |_| location_tool()).as_ref().clone();
    let page = use_state(|| Page::from_path(&initial));

    // Shell -> tool navigation (back/forward or console nav link).
    {
        let page = page.clone();
        let id = id.clone();
        use_console_navigate(Callback::from(move |p: String| {
            go(&id, &page, Page::from_path(&p), true);
        }));
    }

    // In-frame back/forward.
    {
        let page = page.clone();
        use_effect_with((), move |_| {
            let cb = Closure::<dyn Fn()>::new(move || {
                let (_, p) = location_tool();
                let to = Page::from_path(&p);
                page.set(to);
                path_changed(to.path());
            });
            let w = web_sys::window();
            if let Some(w) = &w {
                let _ = w.add_event_listener_with_callback("popstate", cb.as_ref().unchecked_ref());
            }
            move || {
                if let Some(w) = w {
                    let _ = w.remove_event_listener_with_callback(
                        "popstate",
                        cb.as_ref().unchecked_ref(),
                    );
                }
            }
        });
    }

    // Title + breadcrumbs per page (after Init so the shell is listening).
    {
        let connected = state.is_some();
        use_effect_with((*page, connected), |(p, connected)| {
            if *connected {
                set_title(p.title());
                match p {
                    Page::Overview => set_breadcrumbs(&[("Overview", "")]),
                    Page::Headers => set_breadcrumbs(&[("Overview", "/"), ("Request headers", "")]),
                }
            }
        });
    }

    // Own-origin storage: a visit counter in this tool's localStorage.
    let visits = use_memo((), |_| bump_visits());

    let nav_link = |to: Page| {
        let current = (*page == to).then_some("page");
        let href = format!("/t/{}{}", id, to.path());
        let page = page.clone();
        let id = id.clone();
        let onclick = Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            go(&id, &page, to, true);
        });
        html! {
            <a class="hello-nav__link" {href} {onclick} aria-current={current}
               data-testid={format!("hello-nav-{}", to.title().to_lowercase().replace(' ', "-"))}>
                { to.title() }
            </a>
        }
    };

    html! {
        <main class="hello" data-testid="hello-root" data-page={page.path()}>
            { flash.view() }
            <nav class="hello-nav" aria-label="Hello tool">
                { nav_link(Page::Overview) }
                { nav_link(Page::Headers) }
            </nav>
            <Header variant={HeaderVariant::H1}>{ page.title() }</Header>
            <ConnectionStatus {status} />
            if *page == Page::Overview {
                <Overview scope={scope.clone()} theme={theme.clone()}
                    init_user={state.as_ref().map(|s| s.user.display_name.clone())}
                    visits={*visits} flash={flash.clone()} />
            } else {
                <Headers flash={flash.clone()} />
            }
        </main>
    }
}

fn bump_visits() -> u64 {
    let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
        return 0;
    };
    let n = s
        .get_item(VISITS_KEY)
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        + 1;
    let _ = s.set_item(VISITS_KEY, &n.to_string());
    n
}

#[derive(Properties, PartialEq)]
struct ConnectionProps {
    status: ConsoleStatus,
}

/// `data-status`: `connecting` | `connected` | `standalone` | `failed`.
#[function_component]
fn ConnectionStatus(p: &ConnectionProps) -> Html {
    let (key, kind, text): (&str, StatusKind, String) = match &p.status {
        ConsoleStatus::Connecting => (
            "connecting",
            StatusKind::Pending,
            "Waiting for console".into(),
        ),
        ConsoleStatus::Connected => (
            "connected",
            StatusKind::Success,
            "Connected to console".into(),
        ),
        ConsoleStatus::Standalone => (
            "standalone",
            StatusKind::Warning,
            "Not running inside the console".into(),
        ),
        ConsoleStatus::Failed(e) => (
            "failed",
            StatusKind::Error,
            format!("Console connection failed: {e}. Reopen this tool from the console."),
        ),
    };
    html! {
        <p data-testid="hello-connection" data-status={key}
           data-connected={(key == "connected").to_string()} role="status">
            <StatusIndicator {kind}>{ text }</StatusIndicator>
        </p>
    }
}

#[derive(Properties, PartialEq)]
struct OverviewProps {
    scope: Scope,
    theme: String,
    init_user: Option<String>,
    visits: u64,
    flash: Flash,
}

fn opt(v: &Option<String>) -> String {
    v.clone().unwrap_or_else(|| "(none)".into())
}

/// `GET` (or `POST` with the tool CSRF header) through the SDK fetch helper,
/// which adds `X-Console-Scope`.
async fn api(path: &str, post: bool) -> Result<serde_json::Value, String> {
    let init = RequestInit::new();
    if post {
        init.set_method("POST");
        let h = web_sys::Headers::new().map_err(crate::util::js_err)?;
        h.set("X-Hello-CSRF", "1").map_err(crate::util::js_err)?;
        init.set_headers(&h);
    }
    let req = Request::new_with_str_and_init(path, &init).map_err(crate::util::js_err)?;
    let resp = console_fetch(req).await.map_err(|e| e.to_string())?;
    response_json(resp).await
}

#[function_component]
fn Overview(p: &OverviewProps) -> Html {
    let whoami = use_state(|| None::<Rc<serde_json::Value>>);
    let count = use_state(|| None::<u64>);
    let busy = use_state(|| false);

    // Re-query the backend whenever the scope changes: the gateway puts the
    // scope into the assertion `ctx`, so this proves it reached the server.
    {
        let whoami = whoami.clone();
        let flash = p.flash.clone();
        use_effect_with(p.scope.clone(), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match api("api/whoami", false).await {
                    Ok(v) => whoami.set(Some(Rc::new(v))),
                    Err(e) => flash.error(format!("Could not load identity: {e}")),
                }
            });
        });
    }

    let on_notify = Callback::from(|_| notify(Level::Success, "Hello from Hello Tool"));
    let on_count = {
        let count = count.clone();
        let busy = busy.clone();
        let flash = p.flash.clone();
        Callback::from(move |_| {
            let count = count.clone();
            let busy = busy.clone();
            let flash = flash.clone();
            busy.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match api("api/counter", true).await {
                    Ok(v) => count.set(v["count"].as_u64()),
                    Err(e) => flash.error(format!("Counter update failed: {e}")),
                }
                busy.set(false);
            });
        })
    };

    let claims = whoami.as_ref().map(|w| w["claims"].clone());
    let claim = |k: &str| -> String {
        claims
            .as_ref()
            .and_then(|c| c[k].as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    let roles = claims
        .as_ref()
        .and_then(|c| c["roles"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r.as_str().map(str::to_owned))
        .collect::<Vec<_>>()
        .join(", ");
    let ctx = claims
        .as_ref()
        .map(|c| c["ctx"].to_string())
        .unwrap_or_default();

    html! {
        <>
        <Container testid="hello-identity"
            header={html!{ <Header description="From the backend's verified X-Console-Assertion">{ "Identity" }</Header> }}>
            <dl class="hello-kv">
                <dt>{ "Verified" }</dt>
                <dd data-testid="hello-verified">{ if whoami.is_some() { "yes" } else { "loading" } }</dd>
                <dt>{ "Name" }</dt><dd data-testid="hello-user-name">{ claim("name") }</dd>
                <dt>{ "Username" }</dt><dd data-testid="hello-user-username">{ claim("preferred_username") }</dd>
                <dt>{ "Subject" }</dt><dd data-testid="hello-user-sub">{ claim("sub") }</dd>
                <dt>{ "Roles" }</dt><dd data-testid="hello-user-roles">{ roles }</dd>
                <dt>{ "Audience" }</dt><dd data-testid="hello-claim-aud">{ claim("aud") }</dd>
                <dt>{ "Issuer" }</dt><dd data-testid="hello-claim-iss">{ claim("iss") }</dd>
                <dt>{ "Server-side scope (ctx)" }</dt><dd data-testid="hello-claim-ctx">{ ctx }</dd>
                <dt>{ "Shell Init user (display only)" }</dt>
                <dd data-testid="hello-init-user">{ opt(&p.init_user) }</dd>
            </dl>
        </Container>
        <Container testid="hello-context"
            header={html!{ <Header description="Pushed by the console (ScopeChanged / ThemeChanged)">{ "Console context" }</Header> }}>
            <dl class="hello-kv">
                <dt>{ "Site" }</dt><dd data-testid="hello-scope-site">{ opt(&p.scope.site) }</dd>
                <dt>{ "Tenant" }</dt><dd data-testid="hello-scope-tenant">{ opt(&p.scope.tenant) }</dd>
                <dt>{ "Environment" }</dt><dd data-testid="hello-scope-environment">{ opt(&p.scope.environment) }</dd>
                <dt>{ "Theme" }</dt><dd data-testid="hello-theme">{ p.theme.clone() }</dd>
                <dt>{ "Visits (this tool's localStorage)" }</dt>
                <dd data-testid="hello-storage-visits">{ p.visits }</dd>
            </dl>
        </Container>
        <Container testid="hello-actions" header={html!{ <Header>{ "Actions" }</Header> }}>
            <div class="hello-actions">
                <Button variant={Variant::Primary} onclick={on_notify} testid="hello-notify">
                    { "Send notification" }
                </Button>
                <Button onclick={on_count} loading={*busy} testid="hello-counter-increment">
                    { "Increment counter (POST)" }
                </Button>
                <span data-testid="hello-counter-value" aria-live="polite">
                    { count.map(|c| format!("Count: {c}")).unwrap_or_default() }
                </span>
            </div>
        </Container>
        </>
    }
}

#[derive(Properties, PartialEq)]
struct HeadersProps {
    flash: Flash,
}

/// What the backend actually received (E2E: spoofed identity headers absent).
#[function_component]
fn Headers(p: &HeadersProps) -> Html {
    let rows = use_state(|| None::<Vec<(String, String)>>);
    {
        let rows = rows.clone();
        let flash = p.flash.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match api("api/headers", false).await {
                    Ok(v) => rows.set(Some(
                        v["headers"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                            .iter()
                            .map(|h| {
                                (
                                    h["name"].as_str().unwrap_or("").to_owned(),
                                    h["value"].as_str().unwrap_or("").to_owned(),
                                )
                            })
                            .collect(),
                    )),
                    Err(e) => flash.push(AlertKind::Error, format!("Could not load headers: {e}")),
                }
            });
        });
    }
    html! {
        <Container testid="hello-headers"
            header={html!{ <Header>{ "Headers received by the backend" }</Header> }}>
            if let Some(rows) = &*rows {
                <div class="cui-table-wrap">
                    <table class="cui-table" data-testid="hello-headers-table">
                        <caption class="cui-visually-hidden">{ "Request headers" }</caption>
                        <thead><tr><th scope="col">{ "Name" }</th><th scope="col">{ "Value" }</th></tr></thead>
                        <tbody>
                        { for rows.iter().map(|(n, v)| html! {
                            <tr data-testid={format!("hello-header-{n}")}>
                                <td class="hello-mono">{ n.clone() }</td>
                                <td class="hello-mono hello-wrap">{ v.clone() }</td>
                            </tr>
                        }) }
                        </tbody>
                    </table>
                </div>
            } else {
                <console_ui::Spinner label="Loading headers" />
            }
        </Container>
    }
}
