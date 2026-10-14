use std::path::PathBuf;
use std::process::ExitStatus;

/// Steam AppID for the first-party Loo Cast/Vapor root application.
pub const ROOT_STEAM_APP_ID: u32 = 2_122_620;

/// Default Steam DepotID for the first-party root application files.
pub const ROOT_STEAM_DEPOT_ID: u32 = 2_122_621;

/// Root SDK workflows for the Steam-distributed app itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootCommand {
    /// Assemble the Steam redistributable root package and SteamPipe scripts.
    Package(RootPackageRequest),
    /// Assemble the root package and invoke SteamCMD with `+run_app_build`.
    Publish(RootPublishRequest),
}

/// Parameters for root packaging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootPackageRequest {
    /// Report what would be packaged without mutating the package output.
    pub plan: bool,
    /// Steam AppID receiving the root package.
    pub app_id: u32,
    /// Steam DepotID receiving the root package files.
    pub depot_id: u32,
    /// Internal Steamworks build description.
    pub description: String,
    /// Optional beta branch to set live after upload. Steam does not allow
    /// automatically setting the default branch live from the build script.
    pub set_live: Option<String>,
}

/// Parameters for root publishing through SteamCMD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootPublishRequest {
    /// Report what would be packaged and uploaded without mutating or invoking SteamCMD.
    pub plan: bool,
    /// Steam AppID receiving the root package.
    pub app_id: u32,
    /// Steam DepotID receiving the root package files.
    pub depot_id: u32,
    /// Internal Steamworks build description.
    pub description: String,
    /// Steam account used by SteamCMD. SteamCMD prompts for password/Steam Guard
    /// when the local login token is missing.
    pub account: String,
    /// SteamCMD executable path, or `steamcmd` to resolve through PATH.
    pub steamcmd: PathBuf,
    /// Optional beta branch to set live after upload. Steam does not allow
    /// automatically setting the default branch live from the build script.
    pub set_live: Option<String>,
}

/// Report for root package assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootPackageReport {
    pub planned: bool,
    pub app_id: u32,
    pub depot_id: u32,
    pub host_triple: String,
    pub description: String,
    pub set_live: Option<String>,
    pub vapor_home: PathBuf,
    pub package_root: PathBuf,
    pub content_root: PathBuf,
    pub scripts_root: PathBuf,
    pub steam_build_root: PathBuf,
    pub app_build_script: PathBuf,
    pub included_roots: Vec<PathBuf>,
    pub copied_files: usize,
}

/// Report for a SteamCMD root publish attempt.
#[derive(Debug, Clone)]
pub struct RootPublishReport {
    pub planned: bool,
    pub package: RootPackageReport,
    pub steamcmd: PathBuf,
    pub account: String,
    pub steam_args: Vec<String>,
    pub status: Option<ExitStatus>,
    pub set_live: Option<String>,
}
