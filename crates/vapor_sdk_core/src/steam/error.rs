use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use crate::toolchain::ToolchainStatusError;

/// Error returned by SteamCMD tooling workflows.
#[derive(Debug)]
pub enum SteamCommandError {
    ToolchainStatus(ToolchainStatusError),
    SteamCmdIo(std::io::Error),
    SteamCmdUnavailable(PathBuf),
    MissingAccount,
}

impl From<ToolchainStatusError> for SteamCommandError {
    fn from(error: ToolchainStatusError) -> Self {
        Self::ToolchainStatus(error)
    }
}

impl fmt::Display for SteamCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ToolchainStatus(error) => write!(formatter, "{error}"),
            Self::SteamCmdIo(error) => write!(formatter, "failed to run SteamCMD: {error}"),
            Self::SteamCmdUnavailable(path) => {
                write!(
                    formatter,
                    "SteamCMD is not available at `{}`",
                    path.display()
                )
            }
            Self::MissingAccount => write!(formatter, "Steam account must not be empty"),
        }
    }
}

impl Error for SteamCommandError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ToolchainStatus(error) => Some(error),
            Self::SteamCmdIo(error) => Some(error),
            Self::SteamCmdUnavailable(_) | Self::MissingAccount => None,
        }
    }
}
