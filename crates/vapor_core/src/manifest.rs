//! Vapor manifest parsing for small, human-authored intent files.
//!
//! Role-specific `*.vapor.toml` files name what a repository or artifact is and
//! record decisions a human must explicitly make. They are not the place for
//! resolved download URLs, hashes, install receipts, or other generated state.

use crate::toolchain::CanonicalToolchain;
use serde::Deserialize;
use std::error::Error;
use std::fmt;

/// The canonical workspace manifest embedded into `vapor_core`.
pub const CANONICAL_WORKSPACE_MANIFEST_TEXT: &str = include_str!("../../../Workspace.vapor.toml");

/// Parsed workspace manifest data.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaporManifest {
    pub schema: Option<u32>,
    pub workspace: Option<WorkspaceIdentity>,
    pub toolchain: ToolchainIntent,
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

/// Human-selected Rust toolchain intent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolchainIntent {
    pub channel: String,
    pub version: Option<String>,
    pub date: String,
}

/// Parse a role-specific Vapor manifest string.
pub fn parse_manifest(source: &str) -> Result<VaporManifest, ManifestError> {
    Ok(toml::from_str(source)?)
}

/// Parse the embedded canonical Vapor workspace manifest.
pub fn canonical_manifest() -> Result<VaporManifest, ManifestError> {
    parse_manifest(CANONICAL_WORKSPACE_MANIFEST_TEXT)
}

/// Return the canonical ecosystem toolchain inferred from the root manifest.
pub fn canonical_toolchain() -> Result<CanonicalToolchain, ManifestError> {
    Ok(CanonicalToolchain::from_intent(
        canonical_manifest()?.toolchain,
    ))
}

/// Error returned when a Vapor manifest cannot be parsed.
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
            "failed to parse embedded Workspace.vapor.toml: {}",
            self.source
        )
    }
}

impl Error for ManifestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}
