//! Vapor-managed development operations.
//!
//! Development happens against external authored source while build outputs
//! and managed tooling belong operationally to the active Vapor Installation.

use crate::{
    InstallationRootSource, ManagedToolchain, ToolchainError, VaporProject, VaporWorkspace,
};
use serde::Deserialize;
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

const DEVELOPMENT_DIR: &str = "development";
const TARGET_DIR: &str = "target";
const DEV_PROFILE_DIR: &str = "debug";
const BIN_DIR: &str = "bin";

const SELF_HOST_BINARIES: &[&str] = &["vapor", "vapor-installer"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevelopmentOperation {
    Build,
    Test,
}

impl DevelopmentOperation {
    fn cargo_subcommand(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Test => "test",
        }
    }
}

impl fmt::Display for DevelopmentOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.cargo_subcommand())
    }
}

#[derive(Debug, Clone)]
pub struct DeployedBinary {
    pub name: String,
    pub source: PathBuf,
    pub destination: PathBuf,
}

#[derive(Debug, Clone)]
pub struct EcosystemDeploymentReport {
    pub installation_root: PathBuf,
    pub binaries: Vec<DeployedBinary>,
    pub activation_script: PathBuf,
}

pub fn run_workspace_operation(
    workspace: &VaporWorkspace,
    operation: DevelopmentOperation,
) -> Result<(), DevelopmentError> {
    let toolchain =
        ManagedToolchain::for_workspace(workspace).map_err(DevelopmentError::Toolchain)?;

    for project in &workspace.projects {
        run_project_operation(&toolchain, project, operation)?;
    }

    Ok(())
}

/// Build the current Vapor command surfaces through the active Installation's
/// managed toolchain and promote them into that Installation.
///
/// A source-local rewrite-bootstrap `.vapor` directory is deliberately not a
/// valid deployment target. The first real deployment must select the actual
/// Vapor App Instance through `VAPOR_HOME`; afterward the installed executable
/// discovers that same Installation automatically.
pub fn deploy_workspace(
    workspace: &VaporWorkspace,
) -> Result<EcosystemDeploymentReport, DevelopmentError> {
    let toolchain =
        ManagedToolchain::for_workspace(workspace).map_err(DevelopmentError::Toolchain)?;

    if toolchain.installation_source == InstallationRootSource::WorkspaceBootstrap {
        return Err(DevelopmentError::DeploymentRequiresInstallation {
            bootstrap_root: toolchain.vapor_home,
        });
    }

    toolchain
        .persist_installation_metadata()
        .map_err(DevelopmentError::Toolchain)?;

    let installation_root = toolchain.vapor_home.clone();
    let bin_root = installation_root.join(BIN_DIR);

    fs::create_dir_all(&bin_root).map_err(|source| DevelopmentError::Io {
        path: bin_root.clone(),
        source,
    })?;

    let mut binaries = Vec::new();

    for &binary in SELF_HOST_BINARIES {
        let target = find_binary_target(&toolchain, workspace, binary)?;

        build_binary(&toolchain, &target, binary)?;

        let target_dir = development_target_dir(&toolchain, &target.project);

        let source = target_dir
            .join(DEV_PROFILE_DIR)
            .join(executable_name(binary));

        if !source.is_file() {
            return Err(DevelopmentError::MissingBuiltExecutable {
                binary: binary.to_owned(),
                path: source,
            });
        }

        let destination = bin_root.join(executable_name(binary));

        promote_file(&source, &destination)?;

        binaries.push(DeployedBinary {
            name: binary.to_owned(),
            source,
            destination,
        });
    }

    let activation_script = write_activation_script(&installation_root)?;

    Ok(EcosystemDeploymentReport {
        installation_root,
        binaries,
        activation_script,
    })
}

pub fn development_target_dir(toolchain: &ManagedToolchain, project: &VaporProject) -> PathBuf {
    toolchain
        .vapor_home
        .join(DEVELOPMENT_DIR)
        .join(&project.name)
        .join(TARGET_DIR)
}

fn run_project_operation(
    toolchain: &ManagedToolchain,
    project: &VaporProject,
    operation: DevelopmentOperation,
) -> Result<(), DevelopmentError> {
    let target_dir = development_target_dir(toolchain, project);

    let status = toolchain
        .cargo_command()
        .map_err(DevelopmentError::Toolchain)?
        .arg(operation.cargo_subcommand())
        .arg("--workspace")
        .arg("--manifest-path")
        .arg("Cargo.toml")
        .env("CARGO_TARGET_DIR", target_dir)
        .current_dir(&project.root)
        .status()
        .map_err(|source| DevelopmentError::CargoStart {
            project: project.name.clone(),
            operation,
            source,
        })?;

    if !status.success() {
        return Err(DevelopmentError::CargoFailed {
            project: project.name.clone(),
            operation,
            status,
        });
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct BinaryTarget {
    project: VaporProject,
    package: String,
}

fn find_binary_target(
    toolchain: &ManagedToolchain,
    workspace: &VaporWorkspace,
    binary: &str,
) -> Result<BinaryTarget, DevelopmentError> {
    let mut matches = Vec::new();

    for project in &workspace.projects {
        let metadata = cargo_metadata(toolchain, project)?;

        for package in metadata.packages {
            if package
                .targets
                .iter()
                .any(|target| target.name == binary && target.kind.iter().any(|kind| kind == "bin"))
            {
                matches.push(BinaryTarget {
                    project: project.clone(),
                    package: package.name,
                });
            }
        }
    }

    match matches.len() {
        0 => Err(DevelopmentError::BinaryTargetNotFound {
            binary: binary.to_owned(),
        }),

        1 => Ok(matches.remove(0)),

        _ => Err(DevelopmentError::BinaryTargetAmbiguous {
            binary: binary.to_owned(),
            matches: matches
                .into_iter()
                .map(|target| format!("{}/{}", target.project.name, target.package))
                .collect(),
        }),
    }
}

fn build_binary(
    toolchain: &ManagedToolchain,
    target: &BinaryTarget,
    binary: &str,
) -> Result<(), DevelopmentError> {
    let target_dir = development_target_dir(toolchain, &target.project);

    let status = toolchain
        .cargo_command()
        .map_err(DevelopmentError::Toolchain)?
        .arg("build")
        .arg("--package")
        .arg(&target.package)
        .arg("--bin")
        .arg(binary)
        .arg("--manifest-path")
        .arg(&target.project.cargo_manifest_path)
        .env("CARGO_TARGET_DIR", target_dir)
        .current_dir(&target.project.root)
        .status()
        .map_err(|source| DevelopmentError::BinaryBuildStart {
            binary: binary.to_owned(),
            source,
        })?;

    if !status.success() {
        return Err(DevelopmentError::BinaryBuildFailed {
            binary: binary.to_owned(),
            status,
        });
    }

    Ok(())
}

fn cargo_metadata(
    toolchain: &ManagedToolchain,
    project: &VaporProject,
) -> Result<CargoMetadataDocument, DevelopmentError> {
    let output = toolchain
        .cargo_command()
        .map_err(DevelopmentError::Toolchain)?
        .arg("metadata")
        .args(["--format-version", "1", "--no-deps"])
        .arg("--manifest-path")
        .arg(&project.cargo_manifest_path)
        .current_dir(&project.root)
        .output()
        .map_err(|source| DevelopmentError::CargoMetadataStart {
            project: project.name.clone(),
            source,
        })?;

    if !output.status.success() {
        return Err(DevelopmentError::CargoMetadataFailed {
            project: project.name.clone(),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    serde_json::from_slice(&output.stdout).map_err(|source| {
        DevelopmentError::InvalidCargoMetadata {
            project: project.name.clone(),
            source,
        }
    })
}

#[derive(Debug, Deserialize)]
struct CargoMetadataDocument {
    packages: Vec<CargoMetadataPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataPackage {
    name: String,

    #[serde(default)]
    targets: Vec<CargoMetadataTarget>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataTarget {
    name: String,

    #[serde(default)]
    kind: Vec<String>,
}

fn promote_file(source: &Path, destination: &Path) -> Result<(), DevelopmentError> {
    let parent = destination
        .parent()
        .ok_or_else(|| DevelopmentError::InvalidDeploymentPath {
            path: destination.to_path_buf(),
        })?;

    fs::create_dir_all(parent).map_err(|source| DevelopmentError::Io {
        path: parent.to_path_buf(),
        source,
    })?;

    let temporary = destination.with_file_name(format!(
        ".{}.tmp-{}",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("vapor"),
        std::process::id(),
    ));

    if temporary.exists() {
        fs::remove_file(&temporary).map_err(|source| DevelopmentError::Io {
            path: temporary.clone(),
            source,
        })?;
    }

    fs::copy(source, &temporary).map_err(|source| DevelopmentError::Io {
        path: temporary.clone(),
        source,
    })?;

    if destination.exists() {
        fs::remove_file(destination).map_err(|source| DevelopmentError::Io {
            path: destination.to_path_buf(),
            source,
        })?;
    }

    fs::rename(&temporary, destination).map_err(|source| DevelopmentError::Io {
        path: destination.to_path_buf(),
        source,
    })?;

    Ok(())
}

fn activation_script_name() -> &'static str {
    if cfg!(windows) {
        "vapor-env.cmd"
    } else {
        "vapor-env.sh"
    }
}

#[cfg(not(windows))]
fn write_activation_script(installation_root: &Path) -> Result<PathBuf, DevelopmentError> {
    let path = installation_root.join(activation_script_name());

    let root = shell_quote(installation_root);

    let source = format!(
        r#"# Generated by `vapor ecosystem deploy`.
#
# Source this file to expose this Installation's Vapor commands.
#
# VAPOR_HOME is intentionally cleared: once Vapor is installed beneath bin/,
# the executable itself is authoritative for discovering its Installation.

unset VAPOR_HOME

VAPOR_BIN={root}/bin

case ":$PATH:" in
    *":$VAPOR_BIN:"*) ;;
    *) export PATH="$VAPOR_BIN${{PATH:+:$PATH}}" ;;
esac

unset VAPOR_BIN
"#
    );

    fs::write(&path, source).map_err(|source| DevelopmentError::Io {
        path: path.clone(),
        source,
    })?;

    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(&path)
        .map_err(|source| DevelopmentError::Io {
            path: path.clone(),
            source,
        })?
        .permissions();

    permissions.set_mode(0o755);

    fs::set_permissions(&path, permissions).map_err(|source| DevelopmentError::Io {
        path: path.clone(),
        source,
    })?;

    Ok(path)
}

#[cfg(windows)]
fn write_activation_script(installation_root: &Path) -> Result<PathBuf, DevelopmentError> {
    let path = installation_root.join(activation_script_name());

    let root = installation_root.display();

    let source = format!(
        "@echo off\r\n\
         rem Generated by `vapor ecosystem deploy`.\r\n\
         rem Installed Vapor discovers its Installation from its executable.\r\n\
         set \"VAPOR_HOME=\"\r\n\
         set \"PATH={root}\\bin;%PATH%\"\r\n"
    );

    fs::write(&path, source).map_err(|source| DevelopmentError::Io {
        path: path.clone(),
        source,
    })?;

    Ok(path)
}

#[cfg(not(windows))]
fn shell_quote(path: &Path) -> String {
    let value = path.to_string_lossy();

    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn executable_name(stem: &str) -> String {
    format!("{stem}{}", env::consts::EXE_SUFFIX)
}

#[derive(Debug)]
pub enum DevelopmentError {
    Toolchain(ToolchainError),

    DeploymentRequiresInstallation {
        bootstrap_root: PathBuf,
    },

    BinaryTargetNotFound {
        binary: String,
    },

    BinaryTargetAmbiguous {
        binary: String,
        matches: Vec<String>,
    },

    CargoStart {
        project: String,
        operation: DevelopmentOperation,
        source: io::Error,
    },

    CargoFailed {
        project: String,
        operation: DevelopmentOperation,
        status: ExitStatus,
    },

    CargoMetadataStart {
        project: String,
        source: io::Error,
    },

    CargoMetadataFailed {
        project: String,
        status: ExitStatus,
        stderr: String,
    },

    InvalidCargoMetadata {
        project: String,
        source: serde_json::Error,
    },

    BinaryBuildStart {
        binary: String,
        source: io::Error,
    },

    BinaryBuildFailed {
        binary: String,
        status: ExitStatus,
    },

    MissingBuiltExecutable {
        binary: String,
        path: PathBuf,
    },

    InvalidDeploymentPath {
        path: PathBuf,
    },

    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for DevelopmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toolchain(error) => error.fmt(formatter),

            Self::DeploymentRequiresInstallation { bootstrap_root } => {
                write!(
                    formatter,
                    "`vapor ecosystem deploy` refuses the rewrite-bootstrap \
                     Installation `{}`; select the real Vapor Steam/App Instance \
                     through VAPOR_HOME for the first deployment",
                    bootstrap_root.display()
                )
            }

            Self::BinaryTargetNotFound { binary } => {
                write!(
                    formatter,
                    "no Cargo binary target named `{binary}` exists in this Vapor Workspace"
                )
            }

            Self::BinaryTargetAmbiguous { binary, matches } => {
                write!(
                    formatter,
                    "Cargo binary target `{binary}` is ambiguous across Vapor Projects: {}",
                    matches.join(", ")
                )
            }

            Self::CargoStart {
                project,
                operation,
                source,
            } => {
                write!(
                    formatter,
                    "failed to start Cargo {operation} for Vapor Project `{project}`: {source}"
                )
            }

            Self::CargoFailed {
                project,
                operation,
                status,
            } => {
                write!(
                    formatter,
                    "Cargo {operation} failed for Vapor Project `{project}` with {status}"
                )
            }

            Self::CargoMetadataStart { project, source } => {
                write!(
                    formatter,
                    "failed to inspect Cargo Project `{project}`: {source}"
                )
            }

            Self::CargoMetadataFailed {
                project,
                status,
                stderr,
            } => {
                write!(
                    formatter,
                    "Cargo metadata failed for Vapor Project `{project}` with {status}"
                )?;

                if !stderr.is_empty() {
                    write!(formatter, ": {stderr}")?;
                }

                Ok(())
            }

            Self::InvalidCargoMetadata { project, source } => {
                write!(
                    formatter,
                    "Cargo returned invalid metadata for Vapor Project `{project}`: {source}"
                )
            }

            Self::BinaryBuildStart { binary, source } => {
                write!(
                    formatter,
                    "failed to start Cargo while building `{binary}`: {source}"
                )
            }

            Self::BinaryBuildFailed { binary, status } => {
                write!(formatter, "Cargo build for `{binary}` failed with {status}")
            }

            Self::MissingBuiltExecutable { binary, path } => {
                write!(
                    formatter,
                    "Cargo reported success for `{binary}` but no executable exists at `{}`",
                    path.display()
                )
            }

            Self::InvalidDeploymentPath { path } => {
                write!(
                    formatter,
                    "invalid Vapor deployment path `{}`",
                    path.display()
                )
            }

            Self::Io { path, source } => {
                write!(formatter, "failed to access `{}`: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for DevelopmentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Toolchain(error) => Some(error),

            Self::CargoStart { source, .. } => Some(source),

            Self::CargoMetadataStart { source, .. } => Some(source),

            Self::InvalidCargoMetadata { source, .. } => Some(source),

            Self::BinaryBuildStart { source, .. } => Some(source),

            Self::Io { source, .. } => Some(source),

            _ => None,
        }
    }
}
