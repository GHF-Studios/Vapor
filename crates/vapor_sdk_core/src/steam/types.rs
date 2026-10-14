use std::path::PathBuf;
use std::process::ExitStatus;

/// SDK Steam workflows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SteamCommand {
    /// Inspect SteamCMD availability and Vapor Steam state paths.
    Status(SteamStatusRequest),
    /// Run SteamCMD login without Vapor seeing the password or Steam Guard code.
    Login(SteamLoginRequest),
}

/// Parameters for Steam status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamStatusRequest {
    /// SteamCMD executable path, or `steamcmd` to resolve through PATH.
    pub steamcmd: PathBuf,
}

/// Parameters for SteamCMD login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamLoginRequest {
    /// Steam account used by SteamCMD.
    pub account: String,
    /// SteamCMD executable path, or `steamcmd` to resolve through PATH.
    pub steamcmd: PathBuf,
}

/// Parameters for running one SteamPipe app build through SteamCMD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamRunAppBuildRequest {
    pub account: String,
    pub steamcmd: PathBuf,
    pub app_build_script: PathBuf,
}

/// Resolved SteamCMD and Vapor Steam state paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamStatusReport {
    pub app_id: u32,
    pub vapor_home: PathBuf,
    pub steam_home: PathBuf,
    pub steamcmd_state_root: PathBuf,
    pub requested_steamcmd: PathBuf,
    pub resolved_steamcmd: Option<PathBuf>,
    pub steamcmd_source: SteamCmdSource,
    pub auth_model: &'static str,
}

/// Report for SteamCMD login.
#[derive(Debug, Clone)]
pub struct SteamLoginReport {
    pub status: SteamStatusReport,
    pub account: String,
    pub steam_args: Vec<String>,
    pub exit_status: ExitStatus,
}

/// Report for SteamCMD app build upload.
#[derive(Debug, Clone)]
pub struct SteamRunAppBuildReport {
    pub status: SteamStatusReport,
    pub account: String,
    pub steam_args: Vec<String>,
    pub exit_status: ExitStatus,
}

/// How a SteamCMD path was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamCmdSource {
    ExplicitPath,
    PathLookup,
    Unavailable,
}

impl SteamCmdSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitPath => "explicit_path",
            Self::PathLookup => "path_lookup",
            Self::Unavailable => "unavailable",
        }
    }
}
