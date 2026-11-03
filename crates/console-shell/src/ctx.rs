//! Contexts the shell provides to routed pages (and, in M4, the SDK bridge).

use console_protocol::Scope;
use console_ui::{AlertKind, NavLink};
use yew::prelude::*;

/// Page header, breadcrumbs and `document.title` for the current route.
#[derive(Clone, PartialEq, Debug)]
pub struct PageMeta {
    pub title: AttrValue,
    pub description: Option<AttrValue>,
    pub breadcrumbs: Vec<NavLink>,
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
