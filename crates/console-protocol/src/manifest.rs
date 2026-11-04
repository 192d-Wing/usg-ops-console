//! Tool manifest (`console-tool.toml`): presentation metadata a tool publishes.
//!
//! The gateway registry, not the manifest, decides whether a tool is allowed
//! and where its upstream lives; a manifest cannot register a tool.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    pub version: String,
    pub sdk_protocol: u32,
    /// Path under `/t/<id>/` the container loads first.
    #[serde(default = "default_entry")]
    pub entry: String,
    #[serde(default)]
    pub required_roles: Vec<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub nav: Vec<NavItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavItem {
    pub label: String,
    pub path: String,
    #[serde(default)]
    pub badge: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid tool id {0:?}: use 1-32 chars of [a-z0-9-], starting with a letter")]
    InvalidId(String),
    #[error("path {0:?} must start with '/' and must not contain '..' or '//'")]
    InvalidPath(String),
    #[error("tool requires SDK protocol {0}, shell supports {1}")]
    UnsupportedProtocol(u32, u32),
}

fn default_entry() -> String {
    "/".into()
}

impl Manifest {
    /// Parse and validate a manifest.
    pub fn from_toml(src: &str) -> Result<Self, ManifestError> {
        let m: Manifest = toml::from_str(src)?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        if !valid_id(&self.id) {
            return Err(ManifestError::InvalidId(self.id.clone()));
        }
        if self.sdk_protocol == 0 || self.sdk_protocol > crate::PROTOCOL_VERSION {
            return Err(ManifestError::UnsupportedProtocol(
                self.sdk_protocol,
                crate::PROTOCOL_VERSION,
            ));
        }
        for p in std::iter::once(&self.entry).chain(self.nav.iter().map(|n| &n.path)) {
            if !valid_path(p) {
                return Err(ManifestError::InvalidPath(p.clone()));
            }
        }
        Ok(())
    }
}

/// Tool id rule shared by the manifest, registry, and routes:
/// 1-32 chars of `[a-z0-9-]`, starting with a letter.
pub fn valid_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Tool-relative path rule: starts with `/`; no `..`, `//`, or `:`.
pub fn valid_path(p: &str) -> bool {
    p.starts_with('/') && !p.contains("..") && !p.contains("//") && !p.contains(':')
}

#[cfg(test)]
mod tests {
    use super::*;

    const DCIM: &str = r#"
id = "dcim"
title = "Data Center (DCIM)"
category = "Infrastructure"
version = "1.4.0"
sdk_protocol = 1
required_roles = ["dcim:read"]
scopes = ["site"]

[[nav]]
label = "Racks"
path = "/racks"
"#;

    #[test]
    fn parses_valid_manifest() {
        let m = Manifest::from_toml(DCIM).unwrap();
        assert_eq!(m.id, "dcim");
        assert_eq!(m.entry, "/");
        assert_eq!(m.nav[0].path, "/racks");
    }

    #[test]
    fn rejects_bad_ids() {
        for id in ["", "DCIM", "1dcim", "dc_im", "../x", &"a".repeat(33)] {
            let src = DCIM.replace("\"dcim\"", &format!("{id:?}"));
            assert!(
                matches!(Manifest::from_toml(&src), Err(ManifestError::InvalidId(_))),
                "{id}"
            );
        }
    }

    #[test]
    fn rejects_bad_paths() {
        for p in ["racks", "/../etc", "//evil.example", "javascript:alert(1)"] {
            let src = DCIM.replace("\"/racks\"", &format!("{p:?}"));
            assert!(
                matches!(
                    Manifest::from_toml(&src),
                    Err(ManifestError::InvalidPath(_))
                ),
                "{p}"
            );
        }
    }

    #[test]
    fn rejects_future_protocol() {
        let src = DCIM.replace("sdk_protocol = 1", "sdk_protocol = 99");
        assert!(matches!(
            Manifest::from_toml(&src),
            Err(ManifestError::UnsupportedProtocol(99, _))
        ));
    }

    #[test]
    fn rejects_unknown_fields() {
        let src = format!("{DCIM}\nupstream = \"http://evil\"\n");
        assert!(matches!(
            Manifest::from_toml(&src),
            Err(ManifestError::Parse(_))
        ));
    }
}
