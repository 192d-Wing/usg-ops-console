//! Console component library ("calm neutral" design) shared by the shell and
//! tools.
//!
//! Styling lives in two static stylesheets shipped with this crate:
//! `assets/tokens.css` (CSS custom properties, light + dark) and
//! `assets/components.css` (`.cui-*` rules using only tokens). Components never
//! emit inline styles, so pages work under `style-src 'self'`.

pub mod components;
pub mod id;
pub mod logic;

pub use components::*;
pub use logic::{RAIL_STORAGE_KEY, RailMode, Theme};

/// Design tokens stylesheet, for tools that copy it into their own dist.
pub const TOKENS_CSS: &str = include_str!("../assets/tokens.css");
/// Component stylesheet (requires [`TOKENS_CSS`]).
pub const COMPONENTS_CSS: &str = include_str!("../assets/components.css");
