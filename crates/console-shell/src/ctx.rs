//! Contexts the shell provides to routed pages and the tool container.

use crate::session::SessionInfo;
use crate::theme::ThemePref;
use console_protocol::Scope;
use console_ui::{AlertKind, NavLink};
use std::rc::Rc;
use yew::prelude::*;

/// Page header, breadcrumbs and `document.title` for the current route.
#[derive(Clone, PartialEq, Debug)]
pub struct PageMeta {
    pub title: AttrValue,
    pub description: Option<AttrValue>,
    pub breadcrumbs: Vec<NavLink>,
    /// Page part of `document.title` when it differs from `title` (a tool's
    /// `SetTitle`, e.g. "Report - Hello Tool"). `None` uses `title`.
    pub document_title: Option<AttrValue>,
}

impl PageMeta {
    /// "Console / <title>" crumbs with no description.
    pub fn simple(title: impl Into<AttrValue>) -> Self {
        let title = title.into();
        Self {
            breadcrumbs: vec![
                NavLink::new("Console", "/"),
                NavLink::new(title.clone(), ""),
            ],
            title,
            description: None,
            document_title: None,
        }
    }
}

/// Services the shell offers to routed pages.
#[derive(Clone, PartialEq)]
pub struct ShellCtx {
    /// Show a flashbar notification (id is assigned by the shell).
    pub flash: Callback<(AlertKind, AttrValue)>,
    /// Set page header + breadcrumbs + document.title. Call from a
    /// `use_effect_with(())`; the shell resets to a route default on navigation.
    pub set_page: Callback<PageMeta>,
}

/// Current global scope. Separate from [`ShellCtx`] so flash-only consumers
/// do not re-render on scope changes. `set` pushes one history entry.
/// Scope is UI context only, never an authorization claim (plan section 5).
#[derive(Clone, PartialEq)]
pub struct ScopeCtx {
    pub scope: Scope,
    pub set: Callback<Scope>,
}

/// Gateway session (plan section 4, "Shell behaviour"). `session` is `None`
/// while `/api/session` is loading or after it failed (`failed`, already
/// flashed).
/// The CSRF token lives only here, in WASM memory.
#[derive(Clone, PartialEq)]
pub struct SessionCtx {
    pub session: Option<Rc<SessionInfo>>,
    /// `POST /api/session/keepalive` (the only way to extend the session).
    pub keepalive: Callback<()>,
    /// `POST /logout`, then follow the returned `redirect`.
    pub sign_out: Callback<()>,
    /// Reload `/api/session` after a failure.
    pub retry: Callback<()>,
    /// A keepalive or sign-out request is in flight.
    pub busy: bool,
    /// `POST /logout` is in flight (announced as "Signing out…").
    pub signing_out: bool,
    /// `/api/session` failed with a non-401 error.
    pub failed: bool,
}

/// Hook for pages: set the page meta once on mount.
#[hook]
pub fn use_page_meta(meta: PageMeta) {
    let ctx = use_context::<ShellCtx>();
    use_effect_with((), move |_| {
        if let Some(c) = ctx {
            c.set_page.emit(meta);
        }
    });
}

/// Current theme preference (the toggle in the top bar). Tools receive
/// `ThemePref::as_str()` ("light" | "dark" | "system") in `Init` /
/// `ThemeChanged`; "system" means follow `prefers-color-scheme`.
#[derive(Clone, Copy, PartialEq)]
pub struct ThemeCtx {
    pub pref: ThemePref,
}
