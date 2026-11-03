//! Console shell entry point: chrome, routing, flash notifications, and theme.
//! Tool registry and iframe container arrive in M2/M4.

#[cfg(feature = "gallery")]
mod gallery;
mod theme;

use console_ui::{
    AlertKind, Container, FlashId, FlashItem, Flashbar, NavLink, SideNavItem, SideNavigation,
    flash_dismiss, flash_push,
};
use yew::prelude::*;
use yew_router::prelude::*;

#[derive(Clone, Routable, PartialEq)]
pub enum Route {
    #[at("/")]
    Home,
    #[cfg(feature = "gallery")]
    #[at("/gallery")]
    Gallery,
    #[not_found]
    #[at("/404")]
    NotFound,
}

/// Services the shell offers to routed pages.
#[derive(Clone, PartialEq)]
pub struct ShellCtx {
    /// Show a flashbar notification (id is assigned by the shell).
    pub flash: Callback<(AlertKind, AttrValue)>,
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
        #[cfg(feature = "gallery")]
        Some(Route::Gallery) => "Component gallery",
        Some(Route::NotFound) => "Not found",
    }
}

/// On client-side navigation, update the title and move focus to `<main>` so
/// screen-reader users hear the new page (WCAG 2.4.2, 2.4.3).
fn announce_route(route: Option<&Route>, move_focus: bool) -> Result<(), &'static str> {
    let doc = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("Document not available")?;
    doc.set_title(&format!("{} - USG Ops Console", page_title(route)));
    if move_focus {
        use web_sys::wasm_bindgen::JsCast;
        let main = doc
            .get_element_by_id("main-content")
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
            .ok_or("Main content region not found")?;
        main.focus()
            .map_err(|_| "Could not move focus to main content")?;
    }
    Ok(())
}

fn switch(route: Route) -> Html {
    match route {
        Route::Home => html! {
            <Container header={html! { <h2>{ "Welcome" }</h2> }}>
                <p>{ "No tools registered yet." }</p>
            </Container>
        },
        #[cfg(feature = "gallery")]
        Route::Gallery => html! { <gallery::Gallery /> },
        Route::NotFound => html! {
            <Container header={html! { <h2>{ "Not found" }</h2> }}>
                <p>{ "This page does not exist." }</p>
            </Container>
        },
    }
}

#[derive(Properties, PartialEq)]
struct ThemeToggleProps {
    on_error: Callback<(AlertKind, AttrValue)>,
}

#[function_component]
fn ThemeToggle(props: &ThemeToggleProps) -> Html {
    let pref = use_state(theme::load);
    {
        let on_error = props.on_error.clone();
        use_effect_with(*pref, move |p| {
            if let Err(e) = theme::apply(*p) {
                on_error.emit((AlertKind::Error, format!("Theme: {e}").into()));
            }
        });
    }
    let onclick = {
        let pref = pref.clone();
        let on_error = props.on_error.clone();
        Callback::from(move |_: MouseEvent| {
            let next = theme::next(*pref);
            if let Err(e) = theme::save(next) {
                on_error.emit((AlertKind::Warning, format!("Theme not saved: {e}").into()));
            }
            pref.set(next);
        })
    };
    html! {
        <button type="button" class="topbar__theme" data-testid="theme-toggle"
            title="Cycle theme: system, light, dark" {onclick}>
            { theme::label(*pref) }
        </button>
    }
}

fn nav_items() -> Vec<SideNavItem> {
    #[allow(unused_mut)]
    let mut items = vec![SideNavItem::Link(NavLink::new("Home", "/"))];
    #[cfg(feature = "gallery")]
    items.push(SideNavItem::Section {
        text: "Developer".into(),
        items: vec![NavLink::new("Component gallery", "/gallery")],
    });
    items
}

#[function_component]
fn Shell() -> Html {
    let navigator = use_navigator();
    let route = use_route::<Route>();
    let flashes = use_state(Vec::<FlashItem>::new);
    let next_id = use_mut_ref(|| 0u64);

    let flash = {
        let flashes = flashes.clone();
        Callback::from(move |(kind, content): (AlertKind, AttrValue)| {
            let mut id = next_id.borrow_mut();
            *id += 1;
            let mut v = (*flashes).clone();
            flash_push(
                &mut v,
                FlashItem {
                    id: FlashId(*id),
                    kind,
                    header: None,
                    content,
                    dismissible: true,
                },
            );
            flashes.set(v);
        })
    };
    let on_dismiss = {
        let flashes = flashes.clone();
        Callback::from(move |id: FlashId| {
            let mut v = (*flashes).clone();
            flash_dismiss(&mut v, id);
            flashes.set(v);
        })
    };
    {
        let flash = flash.clone();
        let first = use_mut_ref(|| true);
        use_effect_with(route.clone(), move |r| {
            // Initial load: title only; don't steal focus from the skip link.
            let move_focus = !std::mem::replace(&mut *first.borrow_mut(), false);
            if let Err(e) = announce_route(r.as_ref(), move_focus) {
                flash.emit((AlertKind::Error, e.into()));
            }
        });
    }
    let onfollow = Callback::from(move |(e, href): (MouseEvent, AttrValue)| {
        if let (Some(nav), Some(r)) = (navigator.as_ref(), Route::recognize(&href)) {
            e.prevent_default();
            nav.push(&r);
        }
    });
    let active_href: Option<AttrValue> = route.map(|r| r.to_path().into());
    let ctx = ShellCtx {
        flash: flash.clone(),
    };

    html! {
        <ContextProvider<ShellCtx> context={ctx}>
            <div class="app" id={APP_CHROME_ID}>
                <a class="skip-link" href="#main-content">{ "Skip to main content" }</a>
                <header class="topbar" role="banner">
                    <span class="topbar__brand">{ "USG Ops Console" }</span>
                    <span class="topbar__spacer" />
                    <span class="topbar__meta">
                        { format!("SDK protocol v{}", console_protocol::PROTOCOL_VERSION) }
                    </span>
                    <ThemeToggle on_error={flash} />
                </header>
                <div class="shell-flash" data-testid="shell-flashbar">
                    <Flashbar items={(*flashes).clone()} {on_dismiss} fallback_focus_id="main-content" />
                </div>
                <div class="layout">
                    <div class="sidenav-region">
                        <SideNavigation aria_label="Console" items={nav_items()}
                            {active_href} onfollow={Some(onfollow)} />
                    </div>
                    <main class="content" id="main-content" tabindex="-1">
                        <Switch<Route> render={switch} />
                    </main>
                </div>
            </div>
            <div id="modal-root" />
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
