//! Icon-rail state for wide layouts (plan section 4.1). Remembered per
//! browser profile in localStorage under [`STORAGE_KEY`]; only the two
//! allow-listed literals are ever written or accepted back. Narrow layouts
//! ignore it (the rail is a drawer behind `nav-toggle`, as in M2).

pub use console_ui::RailMode;

/// localStorage key for the wide-layout rail state.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const STORAGE_KEY: &str = console_ui::RAIL_STORAGE_KEY;

/// Default on first visit (brief decision: expanded).
pub const DEFAULT: RailMode = RailMode::Expanded;

/// Parse a stored value; anything but the two literals falls back to
/// [`DEFAULT`].
// NIST 800-53: SI-10 - stored value passes an allow-list.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn parse_stored(v: Option<&str>) -> RailMode {
    v.and_then(RailMode::parse).unwrap_or(DEFAULT)
}

/// The other state.
pub fn toggled(m: RailMode) -> RailMode {
    match m {
        RailMode::Expanded => RailMode::Collapsed,
        RailMode::Collapsed => RailMode::Expanded,
    }
}

/// Accessible name of `nav-toggle` at wide widths.
pub fn toggle_label(m: RailMode) -> &'static str {
    match m {
        RailMode::Expanded => "Collapse navigation",
        RailMode::Collapsed => "Expand navigation",
    }
}

#[cfg(target_arch = "wasm32")]
mod dom {
    use super::{RailMode, STORAGE_KEY, parse_stored};

    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }

    /// Read once at startup. Storage errors (private mode, disabled storage)
    /// fall back to the default silently: the value is a preference only.
    pub fn load() -> RailMode {
        let v = storage().and_then(|s| s.get_item(STORAGE_KEY).ok().flatten());
        parse_stored(v.as_deref())
    }

    /// Persist; failures are ignored on purpose (plan 4.1, no flash).
    pub fn save(m: RailMode) {
        if let Some(s) = storage() {
            let _ = s.set_item(STORAGE_KEY, m.as_str());
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use dom::{load, save};

#[cfg(not(target_arch = "wasm32"))]
pub fn load() -> RailMode {
    DEFAULT
}
#[cfg(not(target_arch = "wasm32"))]
pub fn save(_: RailMode) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_value_is_allow_listed() {
        assert_eq!(parse_stored(Some("collapsed")), RailMode::Collapsed);
        assert_eq!(parse_stored(Some("expanded")), RailMode::Expanded);
        for bad in [
            None,
            Some(""),
            Some("Collapsed"),
            Some("collapsed "),
            Some("\"><img>"),
            Some("hidden"),
        ] {
            assert_eq!(parse_stored(bad), DEFAULT, "{bad:?}");
        }
    }

    #[test]
    fn storage_key_is_stable() {
        assert_eq!(STORAGE_KEY, "usg-console.rail");
    }

    #[test]
    fn toggle_round_trips_and_labels() {
        for m in [RailMode::Expanded, RailMode::Collapsed] {
            assert_eq!(toggled(toggled(m)), m);
            assert_eq!(parse_stored(Some(m.as_str())), m);
        }
        assert_eq!(toggle_label(RailMode::Expanded), "Collapse navigation");
        assert_eq!(toggle_label(RailMode::Collapsed), "Expand navigation");
    }
}
