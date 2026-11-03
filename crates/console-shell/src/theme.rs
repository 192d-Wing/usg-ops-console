//! Theme selection and persistence. The chosen theme is applied as the
//! `data-theme` attribute on `<html>` (an attribute, not an inline style, so it
//! is compatible with `style-src 'self'`). Only allow-listed values are ever
//! written to the DOM or read back from storage.

/// localStorage key holding the user's theme preference.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const STORAGE_KEY: &str = "cui-theme";

pub use console_ui::Theme as ThemePref;

/// Cycle order for the single toggle button: system -> light -> dark -> system.
pub fn next(t: ThemePref) -> ThemePref {
    match t {
        ThemePref::System => ThemePref::Light,
        ThemePref::Light => ThemePref::Dark,
        ThemePref::Dark => ThemePref::System,
    }
}

/// Visible button label (also its accessible name).
pub fn label(t: ThemePref) -> &'static str {
    match t {
        ThemePref::Light => "Theme: light",
        ThemePref::Dark => "Theme: dark",
        ThemePref::System => "Theme: system",
    }
}

#[cfg(target_arch = "wasm32")]
mod dom {
    use super::{STORAGE_KEY, ThemePref};

    fn storage() -> Result<web_sys::Storage, String> {
        web_sys::window()
            .ok_or("no window")?
            .local_storage()
            .map_err(|_| "localStorage unavailable".to_string())?
            .ok_or_else(|| "localStorage unavailable".to_string())
    }

    /// Read the stored preference. Missing/invalid values fall back to System.
    // NIST 800-53: SI-10 - stored value passes the ThemePref::parse allow-list.
    pub fn load() -> ThemePref {
        storage()
            .ok()
            .and_then(|s| s.get_item(STORAGE_KEY).ok().flatten())
            .and_then(|v| ThemePref::parse(&v))
            .unwrap_or_default()
    }

    /// Apply to `<html data-theme>`.
    pub fn apply(pref: ThemePref) -> Result<(), String> {
        let el = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
            .ok_or("no document element")?;
        match pref.data_theme() {
            Some(v) => el
                .set_attribute("data-theme", v)
                .map_err(|_| "could not set data-theme".to_string()),
            None => el
                .remove_attribute("data-theme")
                .map_err(|_| "could not clear data-theme".to_string()),
        }
    }

    /// Persist the preference.
    pub fn save(pref: ThemePref) -> Result<(), String> {
        storage()?
            .set_item(STORAGE_KEY, pref.as_str())
            .map_err(|_| "could not save theme preference".to_string())
    }
}

#[cfg(target_arch = "wasm32")]
pub use dom::{apply, load, save};

#[cfg(not(target_arch = "wasm32"))]
pub fn load() -> ThemePref {
    ThemePref::default()
}
#[cfg(not(target_arch = "wasm32"))]
pub fn apply(_: ThemePref) -> Result<(), String> {
    Ok(())
}
#[cfg(not(target_arch = "wasm32"))]
pub fn save(_: ThemePref) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_allow_list_only() {
        assert_eq!(ThemePref::parse("light"), Some(ThemePref::Light));
        assert_eq!(ThemePref::parse("dark"), Some(ThemePref::Dark));
        assert_eq!(ThemePref::parse("system"), Some(ThemePref::System));
        for bad in ["", "Dark", "dark ", "\"><script>", "high-contrast"] {
            assert_eq!(ThemePref::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn round_trip_and_cycle() {
        let mut p = ThemePref::System;
        for _ in 0..3 {
            assert_eq!(ThemePref::parse(p.as_str()), Some(p));
            p = next(p);
        }
        assert_eq!(p, ThemePref::System);
    }

    #[test]
    fn system_clears_attribute() {
        assert_eq!(ThemePref::System.data_theme(), None);
        assert_eq!(ThemePref::Dark.data_theme(), Some("dark"));
    }
}
