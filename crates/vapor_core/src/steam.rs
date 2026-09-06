//! Steam / SteamPipe deployment for the Vapor ecosystem.
//!
//! Vapor owns:
//!
//! - semantic deployment metadata;
//! - deterministic staging of the intended App payload;
//! - generation of native SteamPipe VDF scripts;
//! - orchestration of SteamCMD.
//!
//! Steam remains authoritative for authentication, authorization, depot
//! manifests, chunking, upload, Build IDs, and branch delivery.

use crate::{
    DevelopmentError, VaporInstallation, VaporWorkspace, build_workspace_deployment_inputs,
};
use serde::{Deserialize, Serialize};
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

pub const ROOT_DISTRIBUTION_MANIFEST_FILE_NAME: &str = "App-Source.vapor.toml";

pub const INSTALLED_ECOSYSTEM_METADATA_FILE_NAME: &str = "ecosystem.toml";

const ROOT_DISTRIBUTION_SCHEMA: u32 = 1;

const DEVELOPMENT_DIR: &str = "development";

const STEAM_DIR: &str = "steam";

const CONTENT_DIR: &str = "content";

const SCRIPTS_DIR: &str = "scripts";

const OUTPUT_DIR: &str = "output";

const METADATA_DIR: &str = "metadata";

const BIN_DIR: &str = "bin";

const RUSTUP_DIR: &str = "rustup/bin";

const STEAM_STATE_FILE_NAME: &str = "steam.toml";

const STEAM_ACCOUNT_ENV: &str = "VAPOR_STEAM_ACCOUNT";

#[derive(Debug, Clone, Deserialize)]
pub struct EcosystemDistributionManifest {
    pub schema: u32,
    pub root: EcosystemDistributionRoot,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EcosystemDistributionRoot {
    pub name: String,
    pub organization: String,
    pub version: String,
    pub repository: String,
    pub steam: EcosystemSteamDistribution,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EcosystemSteamDistribution {
    #[serde(rename = "app-id")]
    pub app_id: u32,

    #[serde(rename = "development-branch")]
    pub development_branch: String,

    pub depots: EcosystemSteamDepots,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EcosystemSteamDepots {
    pub common: EcosystemSteamDepot,
    pub linux: EcosystemSteamDepot,
    pub windows: EcosystemSteamDepot,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EcosystemSteamDepot {
    pub id: u32,
}

#[derive(Debug, Clone)]
pub struct SteamDeploymentOptions {
    pub preview: bool,
    pub account: Option<String>,
    pub steamcmd: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SteamDepotStage {
    pub id: u32,
    pub name: String,
    pub content_root: PathBuf,
    pub build_script: PathBuf,
}

#[derive(Debug)]
pub struct SteamDeploymentReport {
    pub preview: bool,
    pub app_id: u32,
    pub branch: String,
    pub account: String,
    pub steamcmd: PathBuf,

    pub stage_root: PathBuf,
    pub content_root: PathBuf,
    pub scripts_root: PathBuf,
    pub output_root: PathBuf,

    pub app_build_script: PathBuf,
    pub depots: Vec<SteamDepotStage>,

    pub exit_status: ExitStatus,
}

#[derive(Debug, Clone)]
struct LoadedDistribution {
    manifest_path: PathBuf,
    manifest: EcosystemDistributionManifest,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedSteamState {
    account: Option<String>,
}

pub fn deploy_ecosystem_to_steam(
    workspace: &VaporWorkspace,
    options: SteamDeploymentOptions,
) -> Result<SteamDeploymentReport, SteamDeploymentError> {
    let distribution = load_distribution(&workspace.root)?;

    validate_distribution(&distribution.manifest)?;

    let build =
        build_workspace_deployment_inputs(workspace).map_err(SteamDeploymentError::Development)?;

    let installation = VaporInstallation::for_workspace(workspace);

    let account = resolve_account(&installation, options.account)?;

    let steamcmd = resolve_steamcmd(&build.installation_root, options.steamcmd.as_deref())
        .ok_or_else(|| SteamDeploymentError::SteamCmdUnavailable)?;

    let stage = stage_distribution(&distribution, &build, options.preview)?;

    let exit_status = Command::new(&steamcmd)
        .arg("+login")
        .arg(&account)
        .arg("+run_app_build")
        .arg(&stage.app_build_script)
        .arg("+quit")
        .status()
        .map_err(|source| SteamDeploymentError::SteamCmdStart {
            path: steamcmd.clone(),
            source,
        })?;

    if !exit_status.success() {
        return Err(SteamDeploymentError::SteamCmdFailed {
            path: steamcmd,
            status: exit_status,
        });
    }

    Ok(SteamDeploymentReport {
        preview: options.preview,

        app_id: distribution.manifest.root.steam.app_id,

        branch: distribution.manifest.root.steam.development_branch.clone(),

        account,
        steamcmd,

        stage_root: stage.stage_root,
        content_root: stage.content_root,
        scripts_root: stage.scripts_root,
        output_root: stage.output_root,

        app_build_script: stage.app_build_script,

        depots: stage.depots,

        exit_status,
    })
}

struct SteamStage {
    stage_root: PathBuf,
    content_root: PathBuf,
    scripts_root: PathBuf,
    output_root: PathBuf,
    app_build_script: PathBuf,
    depots: Vec<SteamDepotStage>,
}

fn stage_distribution(
    distribution: &LoadedDistribution,
    build: &crate::EcosystemBuildReport,
    preview: bool,
) -> Result<SteamStage, SteamDeploymentError> {
    let steam = &distribution.manifest.root.steam;

    let (platform_name, platform_depot) = current_platform_depot(&steam.depots)?;

    let stage_root = build
        .installation_root
        .join(DEVELOPMENT_DIR)
        .join(STEAM_DIR)
        .join(steam.app_id.to_string());

    let content_root = stage_root.join(CONTENT_DIR);

    let scripts_root = stage_root.join(SCRIPTS_DIR);

    let output_root = stage_root.join(OUTPUT_DIR);

    // Content and scripts are deterministic realizations of the current
    // deployment. SteamPipe output/cache is deliberately preserved because
    // Steam can reuse it to accelerate subsequent uploads.
    reset_dir(&content_root)?;

    reset_dir(&scripts_root)?;

    fs::create_dir_all(&output_root).map_err(|source| SteamDeploymentError::Io {
        path: output_root.clone(),
        source,
    })?;

    let common_root = content_root.join("common");

    let platform_root = content_root.join(platform_name);

    let common_metadata_root = common_root.join(METADATA_DIR);

    fs::create_dir_all(&common_metadata_root).map_err(|source| SteamDeploymentError::Io {
        path: common_metadata_root.clone(),
        source,
    })?;

    copy_required(
        &build.toolchain_metadata,
        &common_metadata_root.join("toolchain.toml"),
    )?;

    copy_required(
        &distribution.manifest_path,
        &common_metadata_root.join(INSTALLED_ECOSYSTEM_METADATA_FILE_NAME),
    )?;

    let platform_bin_root = platform_root.join(BIN_DIR);

    fs::create_dir_all(&platform_bin_root).map_err(|source| SteamDeploymentError::Io {
        path: platform_bin_root.clone(),
        source,
    })?;

    for binary in &build.binaries {
        copy_required(
            &binary.source,
            &platform_bin_root.join(
                binary
                    .source
                    .file_name()
                    .ok_or_else(|| SteamDeploymentError::MissingInput(binary.source.clone()))?,
            ),
        )?;
    }

    copy_required(
        &build.activation_script,
        &platform_root.join(
            build.activation_script.file_name().ok_or_else(|| {
                SteamDeploymentError::MissingInput(build.activation_script.clone())
            })?,
        ),
    )?;

    let rustup_source = build
        .installation_root
        .join(RUSTUP_DIR)
        .join(executable_name("rustup"));

    let rustup_destination = platform_root
        .join(RUSTUP_DIR)
        .join(executable_name("rustup"));

    copy_required(&rustup_source, &rustup_destination)?;

    let common_script = scripts_root.join(format!("depot_build_{}.vdf", steam.depots.common.id,));

    let platform_script = scripts_root.join(format!("depot_build_{}.vdf", platform_depot.id,));

    write_depot_build_script(&common_script, steam.depots.common.id, "common")?;

    write_depot_build_script(&platform_script, platform_depot.id, platform_name)?;

    let app_build_script = scripts_root.join(format!("app_build_{}.vdf", steam.app_id,));

    write_app_build_script(
        &app_build_script,
        distribution,
        &content_root,
        &output_root,
        &[
            (steam.depots.common.id, &common_script),
            (platform_depot.id, &platform_script),
        ],
        preview,
    )?;

    Ok(SteamStage {
        stage_root,
        content_root,
        scripts_root,
        output_root,
        app_build_script,

        depots: vec![
            SteamDepotStage {
                id: steam.depots.common.id,
                name: "common".to_owned(),
                content_root: common_root,
                build_script: common_script,
            },
            SteamDepotStage {
                id: platform_depot.id,
                name: platform_name.to_owned(),
                content_root: platform_root,
                build_script: platform_script,
            },
        ],
    })
}

fn load_distribution(start: &Path) -> Result<LoadedDistribution, SteamDeploymentError> {
    let Some(manifest_path) = start
        .ancestors()
        .map(|root| root.join(ROOT_DISTRIBUTION_MANIFEST_FILE_NAME))
        .find(|path| path.is_file())
    else {
        return Err(SteamDeploymentError::DistributionManifestNotFound {
            start: start.to_path_buf(),
        });
    };

    let source = fs::read_to_string(&manifest_path).map_err(|source| SteamDeploymentError::Io {
        path: manifest_path.clone(),
        source,
    })?;

    let manifest = toml::from_str(&source).map_err(|error| {
        SteamDeploymentError::InvalidDistributionManifest {
            path: manifest_path.clone(),
            message: error.to_string(),
        }
    })?;

    Ok(LoadedDistribution {
        manifest_path,
        manifest,
    })
}

fn validate_distribution(
    manifest: &EcosystemDistributionManifest,
) -> Result<(), SteamDeploymentError> {
    if manifest.schema != ROOT_DISTRIBUTION_SCHEMA {
        return Err(SteamDeploymentError::UnsupportedDistributionSchema {
            found: manifest.schema,
            supported: ROOT_DISTRIBUTION_SCHEMA,
        });
    }

    let steam = &manifest.root.steam;

    for (name, id) in [
        ("App", steam.app_id),
        ("common depot", steam.depots.common.id),
        ("Linux depot", steam.depots.linux.id),
        ("Windows depot", steam.depots.windows.id),
    ] {
        if id == 0 {
            return Err(SteamDeploymentError::InvalidSteamId { name });
        }
    }

    if steam.development_branch.trim().is_empty() {
        return Err(SteamDeploymentError::MissingDevelopmentBranch);
    }

    if steam.development_branch == "default" {
        return Err(SteamDeploymentError::DefaultBranchCannotBeSetLive);
    }

    if manifest.root.repository.trim().is_empty() {
        return Err(SteamDeploymentError::MissingRootRepository);
    }

    Ok(())
}

fn current_platform_depot(
    depots: &EcosystemSteamDepots,
) -> Result<(&'static str, &EcosystemSteamDepot), SteamDeploymentError> {
    match env::consts::OS {
        "linux" => Ok(("linux", &depots.linux)),

        "windows" => Ok(("windows", &depots.windows)),

        platform => Err(SteamDeploymentError::UnsupportedPlatform {
            platform: platform.to_owned(),
        }),
    }
}

fn resolve_account(
    installation: &VaporInstallation,
    explicit: Option<String>,
) -> Result<String, SteamDeploymentError> {
    if let Some(account) = explicit {
        let account = account.trim();

        if account.is_empty() {
            return Err(SteamDeploymentError::MissingSteamAccount);
        }

        persist_account(installation, account)?;

        return Ok(account.to_owned());
    }

    if let Some(account) = env::var_os(STEAM_ACCOUNT_ENV).filter(|value| !value.is_empty()) {
        return Ok(account.to_string_lossy().into_owned());
    }

    let state_path = installation.state_root().join(STEAM_STATE_FILE_NAME);

    if state_path.is_file() {
        let source =
            fs::read_to_string(&state_path).map_err(|source| SteamDeploymentError::Io {
                path: state_path.clone(),
                source,
            })?;

        let state: PersistedSteamState =
            toml::from_str(&source).map_err(|error| SteamDeploymentError::InvalidSteamState {
                path: state_path.clone(),
                message: error.to_string(),
            })?;

        if let Some(account) = state.account
            && !account.trim().is_empty()
        {
            return Ok(account);
        }
    }

    Err(SteamDeploymentError::MissingSteamAccount)
}

fn persist_account(
    installation: &VaporInstallation,
    account: &str,
) -> Result<(), SteamDeploymentError> {
    let state_root = installation
        .ensure_state_root()
        .map_err(|error| SteamDeploymentError::Installation(error))?;

    let path = state_root.join(STEAM_STATE_FILE_NAME);

    let source = toml::to_string_pretty(&PersistedSteamState {
        account: Some(account.to_owned()),
    })
    .map_err(|error| SteamDeploymentError::EncodeSteamState {
        message: error.to_string(),
    })?;

    fs::write(&path, source).map_err(|source| SteamDeploymentError::Io { path, source })
}

fn resolve_steamcmd(installation_root: &Path, explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(explicit) = explicit {
        return explicit.is_file().then(|| explicit.to_path_buf());
    }

    for candidate in [
        installation_root.join("steam/steamcmd/steamcmd"),
        installation_root.join("steam/steamcmd/steamcmd.sh"),
    ] {
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let path = env::var_os("PATH")?;

    for root in env::split_paths(&path) {
        for candidate in steamcmd_candidates(&root) {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

fn steamcmd_candidates(root: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![root.join("steamcmd")];

    if !env::consts::EXE_SUFFIX.is_empty() {
        let mut value: OsString = root.join("steamcmd").into_os_string();

        value.push(env::consts::EXE_SUFFIX);

        candidates.push(PathBuf::from(value));
    }

    if cfg!(not(windows)) {
        candidates.push(root.join("steamcmd.sh"));
    }

    candidates
}

fn write_app_build_script(
    path: &Path,
    distribution: &LoadedDistribution,
    content_root: &Path,
    output_root: &Path,
    depots: &[(u32, &PathBuf)],
    preview: bool,
) -> Result<(), SteamDeploymentError> {
    let root = &distribution.manifest.root;

    let mut source = String::new();

    source.push_str("\"AppBuild\"\n{\n");

    source.push_str(&format!("    \"AppID\" \"{}\"\n", root.steam.app_id,));

    source.push_str(&format!(
        "    \"Desc\" \"{}\"\n",
        vdf_escape(&format!(
            "Vapor ecosystem {} {}",
            root.version,
            if preview { "preview" } else { "development" },
        ),),
    ));

    if preview {
        source.push_str("    \"Preview\" \"1\"\n");
    } else {
        source.push_str(&format!(
            "    \"SetLive\" \"{}\"\n",
            vdf_escape(&root.steam.development_branch,),
        ));
    }

    source.push_str(&format!(
        "    \"ContentRoot\" \"{}\"\n",
        vdf_escape(&content_root.display().to_string(),),
    ));

    source.push_str(&format!(
        "    \"BuildOutput\" \"{}\"\n",
        vdf_escape(&output_root.display().to_string(),),
    ));

    source.push_str("    \"Depots\"\n    {\n");

    for (depot_id, script) in depots {
        let file_name = script
            .file_name()
            .ok_or_else(|| SteamDeploymentError::MissingInput((*script).clone()))?;

        source.push_str(&format!(
            "        \"{}\" \"{}\"\n",
            depot_id,
            vdf_escape(&file_name.to_string_lossy(),),
        ));
    }

    source.push_str("    }\n}\n");

    fs::write(path, source).map_err(|source| SteamDeploymentError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn write_depot_build_script(
    path: &Path,
    depot_id: u32,
    source_directory: &str,
) -> Result<(), SteamDeploymentError> {
    let source = format!(
        "\"DepotBuild\"\n\
             {{\n\
             \x20   \"DepotID\" \"{depot_id}\"\n\
             \x20   \"FileMapping\"\n\
             \x20   {{\n\
             \x20       \"LocalPath\" \"{}/*\"\n\
             \x20       \"DepotPath\" \".\"\n\
             \x20       \"Recursive\" \"1\"\n\
             \x20   }}\n\
             }}\n",
        vdf_escape(source_directory,),
    );

    fs::write(path, source).map_err(|source| SteamDeploymentError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn reset_dir(path: &Path) -> Result<(), SteamDeploymentError> {
    if path.exists() {
        fs::remove_dir_all(path).map_err(|source| SteamDeploymentError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }

    fs::create_dir_all(path).map_err(|source| SteamDeploymentError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn copy_required(source: &Path, destination: &Path) -> Result<(), SteamDeploymentError> {
    if !source.is_file() {
        return Err(SteamDeploymentError::MissingInput(source.to_path_buf()));
    }

    let parent = destination
        .parent()
        .ok_or_else(|| SteamDeploymentError::MissingInput(destination.to_path_buf()))?;

    fs::create_dir_all(parent).map_err(|source| SteamDeploymentError::Io {
        path: parent.to_path_buf(),
        source,
    })?;

    fs::copy(source, destination).map_err(|source| SteamDeploymentError::Io {
        path: destination.to_path_buf(),
        source,
    })?;

    Ok(())
}

fn vdf_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn executable_name(stem: &str) -> String {
    format!("{stem}{}", env::consts::EXE_SUFFIX,)
}

#[derive(Debug)]
pub enum SteamDeploymentError {
    Development(DevelopmentError),

    Installation(crate::InstallationError),

    DistributionManifestNotFound { start: PathBuf },

    InvalidDistributionManifest { path: PathBuf, message: String },

    UnsupportedDistributionSchema { found: u32, supported: u32 },

    InvalidSteamId { name: &'static str },

    MissingDevelopmentBranch,

    DefaultBranchCannotBeSetLive,

    MissingRootRepository,

    UnsupportedPlatform { platform: String },

    MissingSteamAccount,

    InvalidSteamState { path: PathBuf, message: String },

    EncodeSteamState { message: String },

    SteamCmdUnavailable,

    SteamCmdStart { path: PathBuf, source: io::Error },

    SteamCmdFailed { path: PathBuf, status: ExitStatus },

    MissingInput(PathBuf),

    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for SteamDeploymentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Development(
                error,
            ) => {
                error.fmt(
                    formatter,
                )
            }

            Self::Installation(
                error,
            ) => {
                error.fmt(
                    formatter,
                )
            }

            Self::DistributionManifestNotFound {
                start,
            } => {
                write!(
                    formatter,
                    "could not find `{ROOT_DISTRIBUTION_MANIFEST_FILE_NAME}` from `{}`",
                    start.display(),
                )
            }

            Self::InvalidDistributionManifest {
                path,
                message,
            } => {
                write!(
                    formatter,
                    "invalid Vapor ecosystem distribution manifest `{}`: {message}",
                    path.display(),
                )
            }

            Self::UnsupportedDistributionSchema {
                found,
                supported,
            } => {
                write!(
                    formatter,
                    "unsupported ecosystem distribution schema {found}; this Vapor supports schema {supported}",
                )
            }

            Self::InvalidSteamId {
                name,
            } => {
                write!(
                    formatter,
                    "invalid Steam {name} ID 0",
                )
            }

            Self::MissingDevelopmentBranch => {
                formatter.write_str(
                    "Vapor ecosystem distribution declares no Steam development branch",
                )
            }

            Self::DefaultBranchCannotBeSetLive => {
                formatter.write_str(
                    "Steam's `default` branch cannot be used as Vapor's automatic development SetLive target",
                )
            }

            Self::MissingRootRepository => {
                formatter.write_str(
                    "Vapor ecosystem distribution declares no canonical root source repository",
                )
            }

            Self::UnsupportedPlatform {
                platform,
            } => {
                write!(
                    formatter,
                    "Steam ecosystem deployment is not yet implemented for host platform `{platform}`",
                )
            }

            Self::MissingSteamAccount => {
                write!(
                    formatter,
                    "no Steam build account is configured; pass `--account <ACCOUNT>` once or set `{STEAM_ACCOUNT_ENV}`",
                )
            }

            Self::InvalidSteamState {
                path,
                message,
            } => {
                write!(
                    formatter,
                    "invalid Vapor Steam state `{}`: {message}",
                    path.display(),
                )
            }

            Self::EncodeSteamState {
                message,
            } => {
                write!(
                    formatter,
                    "failed to encode Vapor Steam state: {message}",
                )
            }

            Self::SteamCmdUnavailable => {
                formatter.write_str(
                    "SteamCMD is unavailable; install/expose `steamcmd` on PATH or provide `--steamcmd <PATH>`",
                )
            }

            Self::SteamCmdStart {
                path,
                source,
            } => {
                write!(
                    formatter,
                    "failed to start SteamCMD `{}`: {source}",
                    path.display(),
                )
            }

            Self::SteamCmdFailed {
                path,
                status,
            } => {
                write!(
                    formatter,
                    "SteamCMD `{}` failed with {status}",
                    path.display(),
                )
            }

            Self::MissingInput(
                path,
            ) => {
                write!(
                    formatter,
                    "required Steam deployment input is missing at `{}`",
                    path.display(),
                )
            }

            Self::Io {
                path,
                source,
            } => {
                write!(
                    formatter,
                    "failed to access Steam deployment path `{}`: {source}",
                    path.display(),
                )
            }
        }
    }
}

impl std::error::Error for SteamDeploymentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Development(error) => Some(error),

            Self::Installation(error) => Some(error),

            Self::SteamCmdStart { source, .. } => Some(source),

            Self::Io { source, .. } => Some(source),

            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vdf_escape_handles_backslashes_and_quotes() {
        assert_eq!(
            vdf_escape(r#"C:\hello\"world""#,),
            r#"C:\\hello\\\"world\""#,
        );
    }
}
