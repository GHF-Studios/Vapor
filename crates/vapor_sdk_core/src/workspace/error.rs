use std::env;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::process::ExitStatus;

use crate::toolchain::{ToolchainInstallState, ToolchainStatusError};

/// Error returned while running a Vapor-managed workspace command.
#[derive(Debug)]
pub enum WorkspaceCommandError {
    ToolchainStatus(ToolchainStatusError),
    Io(std::io::Error),
    JoinPaths(env::JoinPathsError),
    ManifestToml(toml::de::Error),
    UnsupportedHost(String),
    ToolchainNotInstalled(ToolchainInstallState),
    CargoPathHasNoParent(PathBuf),
    MissingWorkspaceManifest(PathBuf),
    InvalidWorkspaceManifest(String),
    WrongWorkspaceName { expected: String, actual: String },
    MissingContentGraph,
    InvalidContentGraph(String),
    UnmanagedGeneratedFile(PathBuf),
    GeneratedPathIsNotFile(PathBuf),
    BuildFailedBeforeDeploy(ExitStatus),
    MissingBuiltExecutable(PathBuf),
    ExecutableHasNoFileName(PathBuf),
}

impl From<ToolchainStatusError> for WorkspaceCommandError {
    fn from(error: ToolchainStatusError) -> Self {
        Self::ToolchainStatus(error)
    }
}

impl From<std::io::Error> for WorkspaceCommandError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<env::JoinPathsError> for WorkspaceCommandError {
    fn from(error: env::JoinPathsError) -> Self {
        Self::JoinPaths(error)
    }
}

impl From<toml::de::Error> for WorkspaceCommandError {
    fn from(error: toml::de::Error) -> Self {
        Self::ManifestToml(error)
    }
}

impl fmt::Display for WorkspaceCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ToolchainStatus(error) => write!(formatter, "{error}"),
            Self::Io(error) => write!(formatter, "workspace command failed: {error}"),
            Self::JoinPaths(error) => write!(formatter, "failed to build Vapor PATH: {error}"),
            Self::ManifestToml(error) => {
                write!(
                    formatter,
                    "failed to parse workspace Workspace.vapor.toml: {error}"
                )
            }
            Self::UnsupportedHost(host) => write!(
                formatter,
                "host triple `{host}` is not supported by this Vapor toolchain pin"
            ),
            Self::ToolchainNotInstalled(state) => write!(
                formatter,
                "Vapor toolchain is not installed: {}",
                state.as_str()
            ),
            Self::CargoPathHasNoParent(path) => write!(
                formatter,
                "Vapor cargo path has no parent directory: `{}`",
                path.display()
            ),
            Self::MissingWorkspaceManifest(path) => write!(
                formatter,
                "no Workspace.vapor.toml found at or above `{}`",
                path.display()
            ),
            Self::InvalidWorkspaceManifest(reason) => {
                write!(formatter, "invalid Workspace.vapor.toml: {reason}")
            }
            Self::WrongWorkspaceName { expected, actual } => write!(
                formatter,
                "workspace name must be `{expected}`, found `{actual}`"
            ),
            Self::MissingContentGraph => write!(
                formatter,
                "Workspace.vapor.toml must declare at least one [[workspace.projects]] entry"
            ),
            Self::InvalidContentGraph(reason) => {
                write!(formatter, "invalid content graph: {reason}")
            }
            Self::UnmanagedGeneratedFile(path) => write!(
                formatter,
                "refusing to overwrite unmanaged generated file `{}`",
                path.display()
            ),
            Self::GeneratedPathIsNotFile(path) => write!(
                formatter,
                "generated path exists but is not a file: `{}`",
                path.display()
            ),
            Self::BuildFailedBeforeDeploy(status) => write!(
                formatter,
                "Vapor-managed cargo build failed before deploy with {status}"
            ),
            Self::MissingBuiltExecutable(path) => {
                write!(formatter, "missing built executable `{}`", path.display())
            }
            Self::ExecutableHasNoFileName(path) => write!(
                formatter,
                "deployed executable path has no file name: `{}`",
                path.display()
            ),
        }
    }
}

impl Error for WorkspaceCommandError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ToolchainStatus(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::JoinPaths(error) => Some(error),
            Self::ManifestToml(error) => Some(error),
            Self::UnsupportedHost(_)
            | Self::ToolchainNotInstalled(_)
            | Self::CargoPathHasNoParent(_)
            | Self::MissingWorkspaceManifest(_)
            | Self::InvalidWorkspaceManifest(_)
            | Self::WrongWorkspaceName { .. }
            | Self::MissingContentGraph
            | Self::InvalidContentGraph(_)
            | Self::UnmanagedGeneratedFile(_)
            | Self::GeneratedPathIsNotFile(_)
            | Self::BuildFailedBeforeDeploy(_)
            | Self::MissingBuiltExecutable(_)
            | Self::ExecutableHasNoFileName(_) => None,
        }
    }
}
