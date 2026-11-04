//! Yew integration: wrap the tool in [`ConsoleProvider`] once, then use the
//! hooks anywhere below it (plan 4.8).
//!
//! ```ignore
//! html! { <ConsoleProvider><App /></ConsoleProvider> }
//! // in App:
//! let scope = use_console_scope();
//! let theme = use_console_theme(); // also sets <html data-theme>
//! use_console_navigate(Callback::from(|path: String| router.push(path)));
//! set_breadcrumbs(&[("Reports", "/reports")]);
//! ```

use crate::transport::{ConsoleBridge, ConsoleState, Status, Subscription};
use console_protocol::{Scope, ShellToTool, User};
use std::rc::Rc;
use yew::prelude::*;

pub use crate::transport::{
    console_fetch, navigate_console, notify, path_changed, request_session_refresh, set_badge,
    set_breadcrumbs, set_title,
};

/// Connection status as seen by the UI.
#[derive(Clone, Debug, PartialEq)]
pub enum ConsoleStatus {
    Connecting,
    Connected,
    /// Not embedded in the console (or it did not answer).
    Standalone,
    /// `/_console/context` failed (e.g. tool session expired): reopen from
    /// the console.
    Failed(String),
}

/// Context value provided by [`ConsoleProvider`].
#[derive(Clone, Debug, PartialEq)]
pub struct ConsoleHandle {
    pub bridge: Option<ConsoleBridge>,
    pub state: Option<Rc<ConsoleState>>,
    pub status: ConsoleStatus,
}

#[derive(Properties, PartialEq)]
pub struct ConsoleProviderProps {
    #[prop_or_default]
    pub children: Html,
}

/// Connects once (`/_console/context` + handshake) and provides
/// [`ConsoleHandle`] to the tree.
#[function_component]
pub fn ConsoleProvider(p: &ConsoleProviderProps) -> Html {
    let bridge = use_state(|| None::<ConsoleBridge>);
    let failed = use_state(|| None::<String>);
    let rerender = use_force_update();
    // Lives as long as the provider; dropping it unsubscribes.
    let sub = use_mut_ref(|| None::<Subscription>);
    {
        let (bridge, failed, sub) = (bridge.setter(), failed.setter(), sub.clone());
        use_effect_with((), move |_| {
            yew::platform::spawn_local(async move {
                match ConsoleBridge::connect().await {
                    Ok(b) => {
                        *sub.borrow_mut() = Some(b.watch(move || rerender.force_update()));
                        bridge.set(Some(b));
                    }
                    Err(e) => failed.set(Some(e.to_string())),
                }
            });
        });
    }
    let handle = ConsoleHandle {
        state: bridge.as_ref().and_then(|b| b.state()),
        status: match (&*failed, bridge.as_ref().map(|b| b.status())) {
            (Some(e), _) => ConsoleStatus::Failed(e.clone()),
            (None, None) | (None, Some(Status::Connecting)) => ConsoleStatus::Connecting,
            (None, Some(Status::Connected)) => ConsoleStatus::Connected,
            (None, Some(Status::Standalone)) => ConsoleStatus::Standalone,
        },
        bridge: (*bridge).clone(),
    };
    html! {
        <ContextProvider<ConsoleHandle> context={handle}>
            { p.children.clone() }
        </ContextProvider<ConsoleHandle>>
    }
}

/// Full shell state after `Init` (`None` while connecting or standalone).
#[hook]
pub fn use_console() -> Option<Rc<ConsoleState>> {
    use_context::<ConsoleHandle>().and_then(|h| h.state)
}

#[hook]
pub fn use_console_status() -> ConsoleStatus {
    use_context::<ConsoleHandle>().map_or(ConsoleStatus::Standalone, |h| h.status)
}

#[hook]
pub fn use_console_bridge() -> Option<ConsoleBridge> {
    use_context::<ConsoleHandle>().and_then(|h| h.bridge)
}

/// Current console scope (empty when standalone). Only keys the registry
/// entry honours are filled in.
#[hook]
pub fn use_console_scope() -> Scope {
    use_console().map(|s| s.scope.clone()).unwrap_or_default()
}

/// Display identity from `Init`. Never use it for authorization: verify the
/// gateway's `X-Console-Assertion` on the server.
#[hook]
pub fn use_console_user() -> Option<User> {
    use_console().map(|s| s.user.clone())
}

/// `<html data-theme>` value for a console theme: `Some("light"|"dark")`, or
/// `None` to follow `prefers-color-scheme` ("system" and unknown values).
pub fn data_theme(theme: &str) -> Option<&'static str> {
    match theme {
        "light" => Some("light"),
        "dark" => Some("dark"),
        _ => None,
    }
}

/// Console theme ("light" | "dark" | "system"); also applies it to
/// `<html data-theme>` (allow-listed values only), matching the shell.
#[hook]
pub fn use_console_theme() -> String {
    let theme = use_console().map_or_else(|| "system".to_owned(), |s| s.theme.clone());
    use_effect_with(theme.clone(), |t| {
        let Some(el) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        else {
            return;
        };
        let r = match data_theme(t) {
            Some(v) => el.set_attribute("data-theme", v),
            None => el.remove_attribute("data-theme"),
        };
        if r.is_err() {
            web_sys::console::warn_1(&"console-sdk: could not apply theme".into());
        }
    });
    theme
}

/// Calls `f(path)` for each `Navigate` from the shell (back/forward or a
/// console link into this tool). Route the tool there, then the tool's
/// router should call [`path_changed`] as usual.
#[hook]
pub fn use_console_navigate(f: Callback<String>) {
    let bridge = use_console_bridge();
    use_effect_with((bridge, f), |(bridge, f)| {
        let f = f.clone();
        let sub = bridge.as_ref().map(|b| {
            b.subscribe(Callback::from(move |m: ShellToTool| {
                if let ShellToTool::Navigate { path } = m {
                    f.emit(path);
                }
            }))
        });
        move || drop(sub)
    });
}

#[cfg(test)]
mod tests {
    use super::data_theme;

    #[test]
    fn data_theme_allow_list() {
        assert_eq!(data_theme("light"), Some("light"));
        assert_eq!(data_theme("dark"), Some("dark"));
        for other in ["system", "", "Dark", "\"><x", "high-contrast"] {
            assert_eq!(data_theme(other), None, "{other}");
        }
    }
}
