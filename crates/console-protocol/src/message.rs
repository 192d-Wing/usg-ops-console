//! SDK messages exchanged over `postMessage` between shell and tool iframe.
//!
//! JSON shape: `{"type": "<Variant>", ...fields}`, sent as a string via
//! `postMessage`. No credentials ever cross this bridge; auth is the gateway
//! session cookie on the tool's own origin.
//!
//! Compatibility rules for protocol v1 (frozen fixtures in
//! `tests/fixtures/sdk_v1/`):
//! - Receivers ignore unknown `type` values ([`ShellToTool::from_json`] and
//!   [`ToolToShell::from_json`] return `Ok(None)` for a JSON object whose
//!   `type` is unknown, missing or not a string) and unknown fields (no
//!   `deny_unknown_fields` on messages).
//! - Anything that is not a JSON object (`"x"`, `1`, `null`, `[1]`, invalid
//!   JSON) is `Err(Malformed)`, as is a known `type` with a bad body.
//! - New fields are `Option`/`#[serde(default)]`; variants and fields are never
//!   removed or renamed within v1. A breaking change needs protocol 2,
//!   negotiated through `Ready.protocol` / `Init.protocol`.
//! - Messages larger than [`MAX_MESSAGE_BYTES`] are dropped by both sides.

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

/// Display identity only; tools authorize from the gateway's signed
/// `X-Console-Assertion`, never from this.
///
/// `roles` is **reserved and always empty** in protocol v1: the shell never
/// fills it (INT-007). Tools that need roles read the assertion `roles`
/// claim (filtered by the registry `forward_roles`) on their backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub display_name: String,
    /// Reserved; always `[]` in v1 (see the type docs).
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

/// Upper bound on a serialized message; larger ones are dropped unparsed.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024;

/// Upper bound on `Notify.message`, in chars; the shell truncates beyond it.
pub const MAX_NOTIFY_CHARS: usize = 500;

/// Which end of the tool's tab order should receive focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FocusEdge {
    First,
    Last,
}

/// Errors from [`ShellToTool::from_json`] / [`ToolToShell::from_json`].
#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    #[error("message is {0} bytes, over the {MAX_MESSAGE_BYTES} byte limit")]
    TooLarge(usize),
    #[error("malformed message: {0}")]
    Malformed(#[from] serde_json::Error),
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
        /// Deep-link path under `/t/<id>` the tool should show (v1 addition).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// Registry id of the receiving tool (v1 addition).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_id: Option<String>,
    },
    ScopeChanged {
        scope: Scope,
    },
    ThemeChanged {
        theme: String,
    },
    /// Tool-relative path.
    Navigate {
        path: String,
    },
    /// Answer to `RequestSessionRefresh` (v1 addition).
    SessionRefreshed {
        ok: bool,
        /// RFC 3339 UTC, as in `SessionInfo.idle_expires_at`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idle_expires_at: Option<String>,
    },
    /// Keyboard focus is entering the iframe at `edge` (v1 addition).
    Focus {
        edge: FocusEdge,
    },
}

/// Known `type` tags per direction; anything else is ignored by receivers.
const SHELL_TO_TOOL_TYPES: &[&str] = &[
    "Init",
    "ScopeChanged",
    "ThemeChanged",
    "Navigate",
    "SessionRefreshed",
    "Focus",
];
const TOOL_TO_SHELL_TYPES: &[&str] = &[
    "Ready",
    "SetTitle",
    "SetBreadcrumbs",
    "Notify",
    "SetBadge",
    "NavigateConsole",
    "RequestSessionRefresh",
    "PathChanged",
    "FocusShell",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolToShell {
    Ready {
        protocol: u32,
    },
    SetTitle {
        title: String,
    },
    SetBreadcrumbs {
        items: Vec<(String, String)>,
    },
    Notify {
        level: Level,
        message: String,
    },
    SetBadge {
        key: String,
        count: Option<u32>,
    },
    NavigateConsole {
        tool: String,
        path: String,
    },
    RequestSessionRefresh,
    /// Tool-internal route changed; the shell mirrors it in its URL (v1 addition).
    PathChanged {
        path: String,
    },
    /// Keyboard user is leaving the iframe (F6 / Ctrl+F6) (v1 addition).
    FocusShell {
        #[serde(default)]
        reverse: bool,
    },
}

/// Lenient decode: size cap first; non-object JSON is `Err(Malformed)`; an
/// object with an unknown, missing or non-string `type` is `Ok(None)`; a
/// known type with a bad body is `Err(Malformed)`.
fn decode_lenient<T: serde::de::DeserializeOwned>(
    data: &str,
    known: &[&str],
) -> Result<Option<T>, MessageError> {
    if data.len() > MAX_MESSAGE_BYTES {
        return Err(MessageError::TooLarge(data.len()));
    }
    let value: serde_json::Value = serde_json::from_str(data)?;
    let serde_json::Value::Object(map) = &value else {
        return Err(MessageError::Malformed(
            <serde_json::Error as serde::de::Error>::custom("message is not a JSON object"),
        ));
    };
    match map.get("type") {
        Some(serde_json::Value::String(t)) if known.contains(&t.as_str()) => {
            Ok(Some(serde_json::from_value(value)?))
        }
        _ => Ok(None),
    }
}

impl ShellToTool {
    /// Decode per the v1 rules (unknown type → `Ok(None)`).
    pub fn from_json(data: &str) -> Result<Option<Self>, MessageError> {
        decode_lenient(data, SHELL_TO_TOOL_TYPES)
    }

    pub fn to_json(&self) -> Result<String, MessageError> {
        Ok(serde_json::to_string(self)?)
    }
}

impl ToolToShell {
    /// Decode per the v1 rules (unknown type → `Ok(None)`).
    pub fn from_json(data: &str) -> Result<Option<Self>, MessageError> {
        decode_lenient(data, TOOL_TO_SHELL_TYPES)
    }

    pub fn to_json(&self) -> Result<String, MessageError> {
        Ok(serde_json::to_string(self)?)
    }
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
