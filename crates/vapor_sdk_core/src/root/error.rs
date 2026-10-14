use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use crate::steam::error::SteamCommandError;
use crate::toolchain::ToolchainStatusError;

/// Error returned by root package or SteamPipe publish workflows.
#[derive(Debug)]
pub enum RootCommandError {
    ToolchainStatus(ToolchainStatusError),
    Steam(SteamCommandError),
    Io(std::io::Error),
    InvalidAppId(u32),
    InvalidDepotId(u32),
    DefaultBranchCannotBeSetLive,
    MissingPackageInput(PathBuf),
    SymlinkHasNoParent(PathBuf),
}

impl From<ToolchainStatusError> for RootCommandError {
    fn from(error: ToolchainStatusError) -> Self {
        Self::ToolchainStatus(error)
    }
}

impl From<std::io::Error> for RootCommandError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<SteamCommandError> for RootCommandError {
    fn from(error: SteamCommandError) -> Self {
        Self::Steam(error)
    }
}

impl fmt::Display for RootCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ToolchainStatus(error) => write!(formatter, "{error}"),
            Self::Steam(error) => write!(formatter, "{error}"),
            Self::Io(error) => write!(formatter, "root package command failed: {error}"),
            Self::InvalidAppId(app_id) => {
                write!(formatter, "Steam AppID must be non-zero, found `{app_id}`")
            }
            Self::InvalidDepotId(depot_id) => {
                write!(
                    formatter,
                    "Steam DepotID must be non-zero, found `{depot_id}`"
                )
            }
            Self::DefaultBranchCannotBeSetLive => write!(
                formatter,
                "SteamPipe cannot automatically set the default branch live; upload first, then set default live in Steamworks"
            ),
            Self::MissingPackageInput(path) => {
                write!(
                    formatter,
                    "root package input is missing: `{}`",
                    path.display()
                )
            }
            Self::SymlinkHasNoParent(path) => {
                write!(
                    formatter,
                    "symlink path has no parent: `{}`",
                    path.display()
                )
            }
        }
    }
}

impl Error for RootCommandError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ToolchainStatus(error) => Some(error),
            Self::Steam(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::InvalidAppId(_)
            | Self::InvalidDepotId(_)
            | Self::DefaultBranchCannotBeSetLive
            | Self::MissingPackageInput(_)
            | Self::SymlinkHasNoParent(_) => None,
        }
    }
}
