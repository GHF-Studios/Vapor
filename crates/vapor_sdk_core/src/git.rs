//! SDK-owned source build used to produce Vapor's pinned static Linux Git candidate.

use std::fmt;
use vapor_core::{
    GitNativeBuildReport, GitWindowsArchiveReport, VaporInstallation, build_linux_shadow,
    inspect_windows_mingit,
};

/// Native Git source-build operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitCommand {
    /// Build upstream Linux Git and pinned static dependencies in the disposable build cache.
    Build,
    /// Download and inventory the pinned official Windows MinGit archive without activation.
    InspectWindows,
}

/// Build upstream Git using Vapor-managed Zig into a non-authoritative cache.
pub fn git_build() -> Result<GitNativeBuildReport, GitBuildError> {
    let installation = VaporInstallation::discover().map_err(|error| GitBuildError(error.to_string()))?;
    build_linux_shadow(&installation).map_err(|error| GitBuildError(error.to_string()))
}

/// Acquire, verify, and inventory the pinned official x64 Windows MinGit archive.
pub fn git_inspect_windows() -> Result<GitWindowsArchiveReport, GitBuildError> {
    let installation = VaporInstallation::discover().map_err(|error| GitBuildError(error.to_string()))?;
    inspect_windows_mingit(&installation).map_err(|error| GitBuildError(error.to_string()))
}

#[derive(Debug, Clone)]
pub struct GitBuildError(pub String);

impl fmt::Display for GitBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for GitBuildError {}
