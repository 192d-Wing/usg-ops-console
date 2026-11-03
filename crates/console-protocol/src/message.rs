//! SDK messages exchanged over `postMessage` between shell and tool iframe.
//!
//! JSON shape: `{"type": "<Variant>", ...fields}`. No credentials ever cross
//! this bridge; auth is the gateway session cookie.

use serde::{Deserialize, Serialize};

/// Console-wide selection the tool should filter by (site, tenant, ...).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub environment: Option<String>,
}

/// Display identity only; tools authorize from gateway-supplied headers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ShellToTool {
    Init {
        protocol: u32,
        theme: String,
        locale: String,
        scope: Scope,
        user: User,
    },
    ScopeChanged {
        scope: Scope,
    },
    ThemeChanged {
        theme: String,
    },
    Navigate {
        path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolToShell {
    Ready { protocol: u32 },
    SetTitle { title: String },
    SetBreadcrumbs { items: Vec<(String, String)> },
    Notify { level: Level, message: String },
    SetBadge { key: String, count: Option<u32> },
    NavigateConsole { tool: String, path: String },
    RequestSessionRefresh,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagged_json_shape_is_stable() {
        let m = ToolToShell::Notify {
            level: Level::Warning,
            message: "x".into(),
        };
        assert_eq!(
            serde_json::to_string(&m).unwrap(),
            r#"{"type":"Notify","level":"warning","message":"x"}"#
        );
        let back: ToolToShell =
            serde_json::from_str(r#"{"type":"RequestSessionRefresh"}"#).unwrap();
        assert_eq!(back, ToolToShell::RequestSessionRefresh);
    }

    #[test]
    fn scope_fields_are_optional() {
        let s: Scope = serde_json::from_str("{}").unwrap();
        assert_eq!(s, Scope::default());
    }
}
