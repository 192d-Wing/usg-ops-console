//! Tool registry view for the shell: `GET /api/tools` once after the session
//! loads, exposed as [`ToolsCtx`]. Drives the service menu and side nav
//! (replaces the static M2 list; closes POAM-010). Role filtering is done by
//! the gateway; the shell only shows what it was given.

use crate::api;
use crate::session::origin_of;
use console_protocol::{Scope, ToolSummary, valid_id, valid_path};
use console_ui::{AlertKind, DropdownItem, NavLink, SideNavItem};
use std::rc::Rc;
use yew::prelude::*;

/// Registry load state.
#[derive(Clone, PartialEq, Debug)]
pub enum ToolsState {
    Loading,
    Ready(Rc<Vec<ToolSummary>>),
    /// Non-401 failure, already shown in the flashbar.
    Failed,
}

/// Tools the signed-in user may open, plus a reload action.
#[derive(Clone, PartialEq)]
pub struct ToolsCtx {
    pub state: ToolsState,
    /// Re-fetch `/api/tools` (after a failure, or when a launch shows the
    /// list is stale).
    pub reload: Callback<()>,
}

impl ToolsCtx {
    pub fn find(&self, id: &str) -> Option<&ToolSummary> {
        match &self.state {
            ToolsState::Ready(t) => t.iter().find(|t| t.id == id),
            _ => None,
        }
    }
}

/// Dropdown action id for "Retry" in the service menu.
pub const RETRY_ACTION: &str = "tools-retry";

/// Longest tool title / nav label the chrome displays (chars).
pub const MAX_LABEL_CHARS: usize = 80;

/// Strips control characters, collapses surrounding whitespace and caps the
/// length. Used for every string a tool or the registry puts in the chrome.
/// Rendering is always as a text node; this only keeps layout sane.
pub fn clean_text(s: &str, max: usize) -> String {
    let t: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let t = t.trim();
    match t.char_indices().nth(max) {
        Some((i, _)) => format!("{}\u{2026}", t[..i].trim_end()),
        None => t.to_owned(),
    }
}

/// `valid_path` plus no backslash, whitespace or control characters, and a
/// length cap (launch bodies are limited to 2 KiB by the gateway).
pub fn safe_tool_path(p: &str) -> bool {
    p.len() <= 1024
        && valid_path(p)
        && !p
            .chars()
            .any(|c| c == '\\' || c.is_whitespace() || c.is_control())
}

/// Defensive client-side check of a registry entry. The gateway validates at
/// startup; this only stops a malformed entry from reaching the iframe or a
/// `postMessage` target origin. `own_origin` is the console origin, which a
/// tool must never share.
// NIST 800-53 AC-4, SC-39, SI-10: only exact https per-tool origins distinct
// from the console origin can become a frame src / postMessage target.
pub fn accept_tool(mut t: ToolSummary, own_origin: &str) -> Option<ToolSummary> {
    if !valid_id(&t.id) {
        return None;
    }
    // Exact, already-normalised https origin with no path.
    if origin_of(&t.origin).as_deref() != Some(t.origin.as_str())
        || !t.origin.starts_with("https://")
        || origin_of(own_origin).as_deref() == Some(t.origin.as_str())
    {
        return None;
    }
    t.title = clean_text(&t.title, MAX_LABEL_CHARS);
    if t.title.is_empty() {
        t.title = t.id.clone();
    }
    if !safe_tool_path(&t.entry) {
        t.entry = "/".into();
    }
    t.nav.retain(|n| safe_tool_path(&n.path));
    for n in &mut t.nav {
        n.label = clean_text(&n.label, MAX_LABEL_CHARS);
    }
    t.nav.retain(|n| !n.label.is_empty());
    t.category = t
        .category
        .map(|c| clean_text(&c, MAX_LABEL_CHARS))
        .filter(|c| !c.is_empty());
    Some(t)
}

/// Shell URL path for a tool-relative path: `/tools/<id><path>`.
pub fn tool_href(id: &str, path: &str) -> String {
    format!("/tools/{id}{path}")
}

/// Default menu category for tools without one.
const DEFAULT_CATEGORY: &str = "Tools";

/// Service menu: "Console" home first, then one group per tool category in
/// first-seen order.
pub fn service_items(state: &ToolsState) -> Vec<DropdownItem> {
    let mut out = vec![DropdownItem::Group {
        text: "Console".into(),
        items: vec![DropdownItem::Link {
            id: "home".into(),
            text: "Console home".into(),
            href: "/".into(),
            description: Some("Overview of the console".into()),
        }],
    }];
    match state {
        // Placeholders are non-interactive notes; only Retry is an action
        // (A11Y-006, WCAG 4.1.2).
        ToolsState::Loading => out.push(DropdownItem::Note {
            id: "tools-loading".into(),
            text: "Loading tools\u{2026}".into(),
        }),
        ToolsState::Failed => out.push(DropdownItem::Action {
            id: RETRY_ACTION.into(),
            text: "Tools unavailable. Retry".into(),
        }),
        ToolsState::Ready(tools) if tools.is_empty() => out.push(DropdownItem::Note {
            id: "tools-none".into(),
            text: "No tools available".into(),
        }),
        ToolsState::Ready(tools) => {
            let cat = |t: &ToolSummary| {
                t.category
                    .clone()
                    .unwrap_or_else(|| DEFAULT_CATEGORY.into())
            };
            let mut cats: Vec<String> = Vec::new();
            for t in tools.iter() {
                let c = cat(t);
                if !cats.contains(&c) {
                    cats.push(c);
                }
            }
            for c in cats {
                out.push(DropdownItem::Group {
                    text: c.clone().into(),
                    items: tools
                        .iter()
                        .filter(|t| cat(t) == c)
                        .map(|t| DropdownItem::Link {
                            id: format!("tool-{}", t.id).into(),
                            text: t.title.clone().into(),
                            href: tool_href(&t.id, &t.entry).into(),
                            description: None,
                        })
                        .collect(),
                });
            }
        }
    }
    out
}

/// Side-nav sections: all tools, plus the active tool's own nav entries.
pub fn nav_sections(state: &ToolsState, active: Option<&str>) -> Vec<SideNavItem> {
    let ToolsState::Ready(tools) = state else {
        return Vec::new();
    };
    if tools.is_empty() {
        return Vec::new();
    }
    let mut out = vec![SideNavItem::Section {
        text: "Tools".into(),
        items: tools
            .iter()
            .map(|t| NavLink::new(t.title.clone(), tool_href(&t.id, &t.entry)))
            .collect(),
    }];
    if let Some(t) = active.and_then(|id| tools.iter().find(|t| t.id == id))
        && !t.nav.is_empty()
    {
        out.push(SideNavItem::Section {
            text: t.title.clone().into(),
            items: t
                .nav
                .iter()
                .map(|n| NavLink::new(n.label.clone(), tool_href(&t.id, &n.path)))
                .collect(),
        });
    }
    out
}

/// Only the scope keys a tool honours (registry `scopes`) are sent to it.
// NIST 800-53 AC-4: least information flow to the tool.
pub fn scope_for(tool: &ToolSummary, scope: &Scope) -> Scope {
    let has = |k: &str| tool.scopes.iter().any(|s| s == k);
    Scope {
        site: scope.site.clone().filter(|_| has("site")),
        tenant: scope.tenant.clone().filter(|_| has("tenant")),
        environment: scope.environment.clone().filter(|_| has("environment")),
    }
}

/// Loads `/api/tools` once `enabled` (session ready) and on `reload`.
#[hook]
pub fn use_tools(enabled: bool, flash: Callback<(AlertKind, AttrValue)>) -> ToolsCtx {
    let state = use_state(|| ToolsState::Loading);
    let load = use_callback((state.setter(), flash), |(), (set, flash)| {
        let (set, flash) = (set.clone(), flash.clone());
        set.set(ToolsState::Loading);
        yew::platform::spawn_local(async move {
            match api::get_tools().await {
                Ok(r) => {
                    let own = crate::own_origin();
                    let tools: Vec<ToolSummary> = r
                        .tools
                        .into_iter()
                        .filter_map(|t| accept_tool(t, &own))
                        .collect();
                    set.set(ToolsState::Ready(Rc::new(tools)));
                }
                Err(e) => {
                    if let Some(m) = e.message("Tools") {
                        flash.emit((AlertKind::Error, m.into()));
                        set.set(ToolsState::Failed);
                    }
                }
            }
        });
    });
    {
        let load = load.clone();
        use_effect_with(enabled, move |on| {
            if *on {
                load.emit(());
            }
        });
    }
    let ctx = use_memo(((*state).clone(), load), |(state, reload)| ToolsCtx {
        state: state.clone(),
        reload: reload.clone(),
    });
    (*ctx).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_protocol::ToolNavItem;

    fn tool(id: &str, origin: &str) -> ToolSummary {
        ToolSummary {
            id: id.into(),
            title: "Hello Tool".into(),
            category: None,
            icon: None,
            origin: origin.into(),
            entry: "/".into(),
            nav: vec![
                ToolNavItem {
                    label: "Overview".into(),
                    path: "/".into(),
                },
                ToolNavItem {
                    label: "Bad".into(),
                    path: "//evil".into(),
                },
            ],
            scopes: vec!["site".into()],
        }
    }

    const OWN: &str = "https://console.ops.example.mil";

    #[test]
    fn accept_tool_rules() {
        let ok = accept_tool(tool("hello", "https://hello.tools.ops.example.mil"), OWN).unwrap();
        assert_eq!(ok.nav.len(), 1, "invalid nav path dropped");
        assert!(accept_tool(tool("hello", "https://localhost:9447"), OWN).is_some());
        for bad in [
            "http://hello.tools.example",
            "https://hello.tools.example/",
            "https://HELLO.tools.example",
            "https://hello.tools.example:443",
            "https://u@hello.tools.example",
            "javascript:alert(1)",
            "null",
            "",
            OWN,
        ] {
            assert!(accept_tool(tool("hello", bad), OWN).is_none(), "{bad}");
        }
        for bad_id in ["", "Hello", "1x", "a/b", "../x"] {
            assert!(
                accept_tool(tool(bad_id, "https://a.example"), OWN).is_none(),
                "{bad_id}"
            );
        }
        let mut t = tool("x", "https://a.example");
        t.entry = "/a b".into();
        t.title = "\u{7}  ".into();
        let t = accept_tool(t, OWN).unwrap();
        assert_eq!(t.entry, "/");
        assert_eq!(t.title, "x");
    }

    #[test]
    fn clean_text_caps_and_strips() {
        assert_eq!(clean_text(" a\nb\u{0}c ", 80), "a b c");
        assert_eq!(clean_text("abcdef", 3), "abc\u{2026}");
        assert_eq!(clean_text("ab", 2), "ab");
        assert_eq!(clean_text("\u{e9}\u{e9}\u{e9}", 2), "\u{e9}\u{e9}\u{2026}");
    }

    #[test]
    fn safe_paths() {
        for ok in ["/", "/x?y=1", "/a/b#c", "/%2e"] {
            assert!(safe_tool_path(ok), "{ok}");
        }
        for bad in [
            "",
            "x",
            "//e",
            "/a/../b",
            "/a:b",
            "/a\\b",
            "/a b",
            "/\n",
            "https://e/",
        ] {
            assert!(!safe_tool_path(bad), "{bad:?}");
        }
        assert!(!safe_tool_path(&format!("/{}", "a".repeat(1100))));
    }

    #[test]
    fn menu_groups_by_category() {
        let mut a = tool("a", "https://a.example");
        a.category = Some("Ops".into());
        let b = tool("b", "https://b.example");
        let items = service_items(&ToolsState::Ready(Rc::new(vec![a, b])));
        let groups: Vec<String> = items
            .iter()
            .filter_map(|i| match i {
                DropdownItem::Group { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(groups, ["Console", "Ops", "Tools"]);
        assert!(matches!(&items[1], DropdownItem::Group { items, .. }
            if matches!(&items[0], DropdownItem::Link { href, .. } if href == "/tools/a/")));
        assert!(matches!(
            service_items(&ToolsState::Failed).last(),
            Some(DropdownItem::Action { id, .. }) if id == RETRY_ACTION
        ));
        assert!(matches!(
            service_items(&ToolsState::Loading).last(),
            Some(DropdownItem::Note { .. })
        ));
        assert!(matches!(
            service_items(&ToolsState::Ready(Rc::new(vec![]))).last(),
            Some(DropdownItem::Note { .. })
        ));
    }

    #[test]
    fn nav_includes_active_tool_entries() {
        let t = tool("hello", "https://a.example");
        let st = ToolsState::Ready(Rc::new(vec![t]));
        assert_eq!(nav_sections(&st, None).len(), 1);
        assert_eq!(nav_sections(&st, Some("hello")).len(), 2);
        assert!(nav_sections(&ToolsState::Loading, None).is_empty());
    }

    #[test]
    fn scope_is_filtered_to_tool_keys() {
        let t = tool("hello", "https://a.example");
        let s = Scope {
            site: Some("hq".into()),
            tenant: Some("ops".into()),
            environment: Some("prod".into()),
        };
        assert_eq!(
            scope_for(&t, &s),
            Scope {
                site: Some("hq".into()),
                tenant: None,
                environment: None
            }
        );
    }
}
