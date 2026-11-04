//! Wire types shared by the console shell, gateway, and tools.
//!
//! Any change here is a contract change: tools upgrade on their own schedule,
//! so new fields must be optional and enum variants additive within a
//! [`PROTOCOL_VERSION`].

pub mod manifest;
pub mod message;
pub mod session;
pub mod tools;

pub use manifest::{Manifest, ManifestError, NavItem, valid_id, valid_path};
pub use message::{
    FocusEdge, Level, MAX_MESSAGE_BYTES, MAX_NOTIFY_CHARS, MessageError, Scope, ShellToTool,
    ToolToShell, User,
};
pub use session::{
    AuthErrorBody, AuthErrorKind, AuthErrorReason, LogoutResponse, SessionInfo, SessionUser,
};
pub use tools::{
    AssertionClaims, LaunchRequest, LaunchResponse, ToolContext, ToolNavItem, ToolSummary,
    ToolsResponse,
};

/// SDK protocol version spoken by this build of the shell.
pub const PROTOCOL_VERSION: u32 = 1;
