//! Vapor manifest parsing for small, human-authored intent files.
//!
//! `Vapor.toml` names what a repository/project is and records decisions a human
//! must explicitly make. It is not the place for resolved download URLs, hashes,
//! install receipts, or other generated state.

use crate::toolchain::CanonicalToolchain;
use serde::Deserialize;
use std::error::Error;
use std::fmt;

/// The canonical root manifest embedded into `vapor_core`.
pub const CANONICAL_VAPOR_MANIFEST_TEXT: &str = include_str!("../../../Vapor.toml");

/// Parsed `Vapor.toml` data.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaporManifest {
    pub schema: Option<u32>,
    pub project: Option<ProjectIdentity>,
    pub workspace: Option<WorkspaceIdentity>,
    pub toolchain: ToolchainIntent,
}

/// Identity for the thing described by a `Vapor.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectIdentity {
    pub kind: ProjectKind,
    pub id: String,
}

/// Identity for the canonical Vapor workspace manifest.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct WorkspaceIdentity {
    pub name: String,
    pub organization: String,
    pub version: Option<String>,
    pub repository: Option<String>,
}

/// Manifest kind. More kinds can be added as real authoring surfaces appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectKind {
    Core,
    Sdk,
    Launcher,
    CustomContent,
    Shell,
}

/// Human-selected Rust toolchain intent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolchainIntent {
    pub channel: String,
    pub version: Option<String>,
    pub date: String,
}

/// Parse a `Vapor.toml` string.
pub fn parse_manifest(source: &str) -> Result<VaporManifest, ManifestError> {
    Ok(toml::from_str(source)?)
}

/// Parse the embedded canonical Vapor root manifest.
pub fn canonical_manifest() -> Result<VaporManifest, ManifestError> {
    parse_manifest(CANONICAL_VAPOR_MANIFEST_TEXT)
}

/// Return the canonical ecosystem toolchain inferred from the root manifest.
pub fn canonical_toolchain() -> Result<CanonicalToolchain, ManifestError> {
    Ok(CanonicalToolchain::from_intent(
        canonical_manifest()?.toolchain,
    ))
}

/// Error returned when `Vapor.toml` cannot be parsed.
#[derive(Debug)]
pub struct ManifestError {
    source: toml::de::Error,
}

impl From<toml::de::Error> for ManifestError {
    fn from(source: toml::de::Error) -> Self {
        Self { source }
    }
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "failed to parse embedded Vapor.toml: {}",
            self.source
        )
    }
}

impl Error for ManifestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

#[cfg(test)]
mod tests {
    use super::canonical_manifest;

    #[test]
    fn canonical_manifest_uses_workspace_identity_and_stable_toolchain() {
        let manifest = canonical_manifest().unwrap();

        assert_eq!(manifest.workspace.unwrap().name, "vapor");
        assert_eq!(manifest.toolchain.channel, "stable");
        assert_eq!(manifest.toolchain.version.as_deref(), Some("1.97.0"));
        assert_eq!(manifest.toolchain.date, "2026-07-09");
    }
}
