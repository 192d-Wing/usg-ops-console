//! Tool-side SDK.
//!
//! - [`codec`] (always built, native-testable): message encode/decode with
//!   the v1 rules (origin pin, size cap, unknown types ignored).
//! - `transport` / `hooks` (feature `web`): browser `postMessage` bridge and
//!   Yew hooks for wasm32 tools.
//!
//! Tool backends should depend on `console-protocol` only.

pub mod codec;
#[cfg(feature = "web")]
pub mod hooks;
#[cfg(feature = "web")]
pub mod transport;

pub use codec::{SdkError, check_origin, decode, encode};
pub use console_protocol::{PROTOCOL_VERSION, ShellToTool, ToolToShell};
