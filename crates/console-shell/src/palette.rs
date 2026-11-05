//! Command palette sources and actions for the shell (plan section 4.2).
//!
//! The generic `console_ui::CommandPalette` filters an in-memory list; this
//! module builds that list from what the shell already has (routes, the
//! gateway-filtered `/api/tools` result, the scope, the theme, the session)
//! and maps a selected item id back to a closed [`Action`]. Item ids are
//! opaque; a tool id only becomes a URL by lookup in the registry list plus
//! [`crate::tools::tool_href`], never from typed text. The query itself is
//! never seen here (it stays inside the component).

use crate::theme::ThemePref;
use crate::tools::{ToolsState, tool_href};
use console_protocol::Scope;
use console_ui::{PaletteGroup, PaletteItem};
use yew::AttrValue;

const GROUP_NAV: PaletteGroup = PaletteGroup::Navigation;
const GROUP_TOOLS: PaletteGroup = PaletteGroup::Tools;
const GROUP_SCOPE: PaletteGroup = PaletteGroup::Scope;
const GROUP_PREFS: PaletteGroup = PaletteGroup::Preferences;
const GROUP_ACCOUNT: PaletteGroup = PaletteGroup::Account;

/// Scope breadcrumb segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seg {
    Site,
    Tenant,
    Env,
}

impl Seg {
    pub const ALL: [Seg; 3] = [Seg::Site, Seg::Tenant, Seg::Env];

    /// Short key used in ids and testids (`scope-<key>-trigger`).
    pub fn key(self) -> &'static str {
        match self {
            Seg::Site => "site",
            Seg::Tenant => "tenant",
            Seg::Env => "env",
        }
    }
}

/// What a palette item does. Closed set: nothing here takes free text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Same-origin shell path (a compiled-in route or a registry tool href).
    Navigate(String),
    /// Close the palette and open this scope segment's popover.
    OpenScope(Seg),
    Theme(ThemePref),
    SignOut,
    /// Re-fetch `/api/tools` (was the service menu's Retry).
    RetryTools,
}

/// One palette row before conversion to the console-ui item type.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub label: String,
    pub group: PaletteGroup,
    pub keywords: Vec<String>,
    pub hint: Option<String>,
    /// Informational row ("Tools are loading"); not selectable.
    pub disabled: bool,
}

impl Entry {
    fn new(id: impl Into<String>, label: impl Into<String>, group: PaletteGroup) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            group,
            keywords: Vec::new(),
            hint: None,
            disabled: false,
        }
    }
    fn kw(mut self, k: &[&str]) -> Self {
        self.keywords = k.iter().map(|s| (*s).to_owned()).collect();
        self
    }
    fn hint(mut self, h: impl Into<String>) -> Self {
        self.hint = Some(h.into());
        self
    }
}

const NAV_HOME: &str = "nav-home";
#[cfg(feature = "gallery")]
const NAV_GALLERY: &str = "nav-gallery";
const TOOL_PREFIX: &str = "tool-";
const SCOPE_PREFIX: &str = "scope-";
const THEME_PREFIX: &str = "theme-";
const SIGN_OUT: &str = "sign-out";
pub const TOOLS_LOADING: &str = "tools-loading";
pub const TOOLS_RETRY: &str = "tools-retry";

const THEMES: [ThemePref; 3] = [ThemePref::Light, ThemePref::Dark, ThemePref::System];

/// What the shell knows when the palette opens.
pub struct Sources<'a> {
    pub tools: &'a ToolsState,
    pub scope: &'a Scope,
    pub theme: ThemePref,
    pub signed_in: bool,
}

/// Every palette row, in group order.
pub fn entries(s: &Sources<'_>) -> Vec<Entry> {
    let mut out = vec![Entry::new(NAV_HOME, "Home", GROUP_NAV).kw(&["overview", "console"])];
    #[cfg(feature = "gallery")]
    out.push(Entry::new(NAV_GALLERY, "Component gallery", GROUP_NAV).kw(&["developer"]));

    match s.tools {
        ToolsState::Loading => out.push(Entry {
            disabled: true,
            ..Entry::new(TOOLS_LOADING, "Tools are loading", GROUP_TOOLS)
        }),
        ToolsState::Failed => out
            .push(Entry::new(TOOLS_RETRY, "Tools unavailable. Retry", GROUP_TOOLS).kw(&["reload"])),
        ToolsState::Ready(tools) => {
            for t in tools.iter() {
                let mut e = Entry::new(
                    format!("{TOOL_PREFIX}{}", t.id),
                    t.title.clone(),
                    GROUP_TOOLS,
                );
                e.keywords = vec![t.id.clone()];
                if let Some(c) = &t.category {
                    e.keywords.push(c.clone());
                    e.hint = Some(c.clone());
                }
                out.push(e);
            }
        }
    }

    for seg in Seg::ALL {
        let (label, enabled) = match seg {
            Seg::Site => ("Change site\u{2026}", true),
            Seg::Tenant => ("Change tenant\u{2026}", s.scope.site.is_some()),
            Seg::Env => ("Change environment\u{2026}", s.scope.tenant.is_some()),
        };
        if enabled {
            out.push(
                Entry::new(format!("{SCOPE_PREFIX}{}", seg.key()), label, GROUP_SCOPE)
                    .kw(&["scope", "switch"]),
            );
        }
    }

    for t in THEMES {
        let mut e = Entry::new(
            format!("{THEME_PREFIX}{}", t.as_str()),
            crate::theme::label(t),
            GROUP_PREFS,
        )
        .kw(&["appearance", "color", "mode"]);
        if t == s.theme {
            e = e.hint("Current");
        }
        out.push(e);
    }

    if s.signed_in {
        out.push(Entry::new(SIGN_OUT, "Sign out", GROUP_ACCOUNT).kw(&["log out", "logout"]));
    }
    out
}

/// Converts to the console-ui item type (text only; rendered as text nodes).
pub fn to_items(entries: Vec<Entry>) -> Vec<PaletteItem> {
    entries
        .into_iter()
        .map(|e| PaletteItem {
            id: e.id.into(),
            label: e.label.into(),
            group: e.group,
            keywords: e.keywords.into_iter().map(AttrValue::from).collect(),
            hint: e.hint.map(AttrValue::from),
            disabled: e.disabled,
        })
        .collect()
}

/// Maps a selected id to its action. Tool ids resolve only against the
/// registry list the gateway returned; unknown ids (or a disabled row)
/// return `None`.
pub fn resolve(id: &str, tools: &ToolsState) -> Option<Action> {
    match id {
        NAV_HOME => return Some(Action::Navigate("/".into())),
        #[cfg(feature = "gallery")]
        NAV_GALLERY => return Some(Action::Navigate("/gallery".into())),
        SIGN_OUT => return Some(Action::SignOut),
        TOOLS_RETRY => return Some(Action::RetryTools),
        _ => {}
    }
    if let Some(tid) = id.strip_prefix(TOOL_PREFIX) {
        let ToolsState::Ready(list) = tools else {
            return None;
        };
        return list
            .iter()
            .find(|t| t.id == tid)
            .map(|t| Action::Navigate(tool_href(&t.id, &t.entry)));
    }
    if let Some(k) = id.strip_prefix(SCOPE_PREFIX) {
        return Seg::ALL
            .into_iter()
            .find(|s| s.key() == k)
            .map(Action::OpenScope);
    }
    if let Some(v) = id.strip_prefix(THEME_PREFIX) {
        return ThemePref::parse(v).map(Action::Theme);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_protocol::ToolSummary;
    use std::rc::Rc;

    fn tool(id: &str) -> ToolSummary {
        ToolSummary {
            id: id.into(),
            title: format!("{id} tool"),
            category: Some("Ops".into()),
            icon: None,
            origin: "https://a.example".into(),
            entry: "/start".into(),
            nav: vec![],
            scopes: vec![],
        }
    }

    fn ids(e: &[Entry]) -> Vec<&str> {
        e.iter().map(|e| e.id.as_str()).collect()
    }

    #[test]
    fn sources_follow_state() {
        let tools = ToolsState::Ready(Rc::new(vec![tool("hello")]));
        let scope = Scope::default();
        let e = entries(&Sources {
            tools: &tools,
            scope: &scope,
            theme: ThemePref::Dark,
            signed_in: true,
        });
        let i = ids(&e);
        assert!(i.contains(&"tool-hello") && i.contains(&"scope-site"));
        assert!(!i.contains(&"scope-tenant") && !i.contains(&"scope-env"));
        assert!(i.contains(&"sign-out"));
        let cur: Vec<_> = e
            .iter()
            .filter(|e| e.hint.as_deref() == Some("Current"))
            .collect();
        assert_eq!(cur.len(), 1);
        assert_eq!(cur[0].id, "theme-dark");

        let scope = Scope {
            site: Some("hq".into()),
            tenant: Some("ops".into()),
            environment: None,
        };
        let e = entries(&Sources {
            tools: &ToolsState::Loading,
            scope: &scope,
            theme: ThemePref::System,
            signed_in: false,
        });
        let i = ids(&e);
        assert!(i.contains(&"scope-tenant") && i.contains(&"scope-env"));
        assert!(!i.contains(&"sign-out"));
        let loading = e.iter().find(|e| e.id == TOOLS_LOADING).unwrap();
        assert!(loading.disabled);
        assert!(e.iter().filter(|e| e.disabled).count() == 1);

        let e = entries(&Sources {
            tools: &ToolsState::Failed,
            scope: &scope,
            theme: ThemePref::System,
            signed_in: true,
        });
        let retry = e.iter().find(|e| e.id == TOOLS_RETRY).unwrap();
        assert!(!retry.disabled);
        assert_eq!(
            resolve(TOOLS_RETRY, &ToolsState::Failed),
            Some(Action::RetryTools)
        );
    }

    #[test]
    fn resolve_is_closed() {
        let tools = ToolsState::Ready(Rc::new(vec![tool("hello")]));
        assert_eq!(
            resolve("nav-home", &tools),
            Some(Action::Navigate("/".into()))
        );
        assert_eq!(
            resolve("tool-hello", &tools),
            Some(Action::Navigate("/tools/hello/start".into()))
        );
        assert_eq!(
            resolve("scope-env", &tools),
            Some(Action::OpenScope(Seg::Env))
        );
        assert_eq!(
            resolve("theme-light", &tools),
            Some(Action::Theme(ThemePref::Light))
        );
        assert_eq!(resolve("sign-out", &tools), Some(Action::SignOut));
        for bad in [
            "",
            "tool-",
            "tool-denied",
            "tool-../x",
            "tool-hello/../../evil",
            "scope-",
            "scope-x",
            "theme-",
            "theme-Dark",
            TOOLS_LOADING,
            "//evil.example",
        ] {
            assert_eq!(resolve(bad, &tools), None, "{bad:?}");
        }
        assert_eq!(resolve("tool-hello", &ToolsState::Loading), None);
    }

    #[test]
    fn every_enabled_entry_resolves() {
        let tools = ToolsState::Ready(Rc::new(vec![tool("hello"), tool("b")]));
        let scope = Scope {
            site: Some("hq".into()),
            tenant: Some("ops".into()),
            environment: None,
        };
        for e in entries(&Sources {
            tools: &tools,
            scope: &scope,
            theme: ThemePref::Light,
            signed_in: true,
        }) {
            assert_eq!(resolve(&e.id, &tools).is_some(), !e.disabled, "{}", e.id);
        }
    }
}
