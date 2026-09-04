//! Minimal human-authored Vapor Content manifest model.
//!
//! This schema exists to exercise Vertical Slice 0. It is not yet a promise
//! that the final Vapor manifest format will have exactly this shape.

use crate::{ContentKind, DependencySpec, VaporId};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Minimal authored description of one Vapor Content artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentManifest {
    pub content: ContentHeader,

    /// Local dependency binding name -> dependency requirement.
    ///
    /// Different aliases may therefore refer to different versions of the
    /// same Vapor ID after resolution.
    #[serde(default)]
    pub dependencies: BTreeMap<String, DependencySpec>,
}

/// Identity and kind shared by every Vapor Content artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentHeader {
    pub id: VaporId,
    pub version: Version,
    pub kind: ContentKind,
}

/// Parse one human-authored Vapor Content manifest.
pub fn parse_content_manifest(source: &str) -> Result<ContentManifest, ManifestError> {
    Ok(toml::from_str(source)?)
}

/// Error produced while parsing a Vapor Content manifest.
#[derive(Debug)]
pub struct ManifestError(toml::de::Error);

impl From<toml::de::Error> for ManifestError {
    fn from(error: toml::de::Error) -> Self {
        Self(error)
    }
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "failed to parse Vapor Content manifest: {}",
            self.0
        )
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_content_manifest() {
        let manifest = parse_content_manifest(
            r#"
[content]
id = "ghf-studios/example/game"
version = "0.1.0"
kind = "game"

[dependencies.engine]
id = "ghf-studios/example/engine"
version = "^0.1"
"#,
        )
        .unwrap();

        assert_eq!(manifest.content.kind, ContentKind::Game);
        assert!(manifest.dependencies.contains_key("engine"));
    }
}
