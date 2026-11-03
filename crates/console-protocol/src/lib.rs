//! Wire types shared by the console shell, gateway, and tools.
//!
//! Any change here is a contract change: tools upgrade on their own schedule,
//! so new fields must be optional and enum variants additive within a
//! [`PROTOCOL_VERSION`].

pub mod manifest;
pub mod message;

pub use manifest::{Manifest, ManifestError, NavItem};
pub use message::{Level, Scope, ShellToTool, ToolToShell, User};

/// SDK protocol version spoken by this build of the shell.
pub const PROTOCOL_VERSION: u32 = 1;
