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
pub struct BuiltBinary {
    pub name: String,
    pub source: PathBuf,
}

#[derive(Debug, Clone)]
pub struct EcosystemBuildReport {
    pub installation_root: PathBuf,
    pub binaries: Vec<BuiltBinary>,
    pub activation_script: PathBuf,
    pub toolchain_metadata: PathBuf,
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

/// Build the distributable Vapor command surfaces without choosing a
/// deployment target.
///
/// Build output belongs to Installation-managed development state. The
/// resulting binaries may subsequently be deployed locally, staged for Steam,
/// or consumed by another future deployment target.
///
/// This operation deliberately does not promote the binaries into
/// `<installation>/bin`.
pub fn build_workspace_deployment_inputs(
    workspace: &VaporWorkspace,
) -> Result<EcosystemBuildReport, DevelopmentError> {
    let toolchain =
        ManagedToolchain::for_workspace(workspace).map_err(DevelopmentError::Toolchain)?;

    if toolchain.installation_source == InstallationRootSource::WorkspaceBootstrap {
        return Err(DevelopmentError::DeploymentRequiresInstallation {
            bootstrap_root: toolchain.vapor_home,
        });
    }

    let toolchain_metadata = toolchain
        .persist_installation_metadata()
        .map_err(DevelopmentError::Toolchain)?;

    let installation_root = toolchain.vapor_home.clone();

    let mut binaries = Vec::new();

    for &binary in SELF_HOST_BINARIES {
        let target = find_binary_target(&toolchain, workspace, binary)?;

        build_binary(&toolchain, &target, binary)?;

        let source = development_target_dir(&toolchain, &target.project)
            .join(DEV_PROFILE_DIR)
            .join(executable_name(binary));

        if !source.is_file() {
            return Err(DevelopmentError::MissingBuiltExecutable {
                binary: binary.to_owned(),
                path: source,
            });
        }

        binaries.push(BuiltBinary {
            name: binary.to_owned(),
            source,
        });
    }

    let activation_script = write_activation_script(&installation_root, &toolchain)?;

    Ok(EcosystemBuildReport {
        installation_root,
        binaries,
        activation_script,
        toolchain_metadata,
    })
}

/// Build the current Vapor command surfaces and promote them into the active
/// local Vapor App Instance.
pub fn deploy_workspace(
    workspace: &VaporWorkspace,
) -> Result<EcosystemDeploymentReport, DevelopmentError> {
    let build = build_workspace_deployment_inputs(workspace)?;

    let bin_root = build.installation_root.join(BIN_DIR);

    fs::create_dir_all(&bin_root).map_err(|source| DevelopmentError::Io {
        path: bin_root.clone(),
        source,
    })?;

    let mut binaries = Vec::new();

    for binary in &build.binaries {
        let destination = bin_root.join(executable_name(&binary.name));

        promote_file(&binary.source, &destination)?;

        binaries.push(DeployedBinary {
            name: binary.name.clone(),
            source: binary.source.clone(),
            destination,
        });
    }

    Ok(EcosystemDeploymentReport {
        installation_root: build.installation_root,
        binaries,
        activation_script: build.activation_script,
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
                .map(|target| format!("{}/{}", target.project.name, target.package,))
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

fn managed_toolchain_bin_relative(
    installation_root: &Path,
    toolchain: &ManagedToolchain,
) -> Result<PathBuf, DevelopmentError> {
    let rust_bin =
        toolchain
            .rustc_path
            .parent()
            .ok_or_else(|| DevelopmentError::InvalidDeploymentPath {
                path: toolchain.rustc_path.clone(),
            })?;

    rust_bin
        .strip_prefix(installation_root)
        .map(Path::to_path_buf)
        .map_err(|_| DevelopmentError::InvalidDeploymentPath {
            path: rust_bin.to_path_buf(),
        })
}

#[cfg(not(windows))]
fn write_activation_script(
    installation_root: &Path,
    toolchain: &ManagedToolchain,
) -> Result<PathBuf, DevelopmentError> {
    let path = installation_root.join(activation_script_name());

    let rust_bin = managed_toolchain_bin_relative(installation_root, toolchain)?
        .to_string_lossy()
        .replace('\\', "/");

    let source = format!(
        r#"#!/usr/bin/env bash
# Generated by Vapor.
#
# Source this file to expose this App Instance's installed Vapor commands and
# managed pinned Rust toolchain.
#
# The script is intentionally relocatable. Steam may move or reacquire the App
# Instance at a different absolute path.

VAPOR_ROOT="$(
    CDPATH= cd -- "$(dirname -- "${{BASH_SOURCE[0]}}")" &&
    pwd
)"

unset VAPOR_HOME

VAPOR_BIN="$VAPOR_ROOT/bin"
VAPOR_RUST_BIN="$VAPOR_ROOT/{rust_bin}"

for VAPOR_PATH in "$VAPOR_RUST_BIN" "$VAPOR_BIN"; do
    case ":$PATH:" in
        *":$VAPOR_PATH:"*) ;;
        *) export PATH="$VAPOR_PATH${{PATH:+:$PATH}}" ;;
    esac
done

unset VAPOR_PATH
unset VAPOR_BIN
unset VAPOR_RUST_BIN
unset VAPOR_ROOT
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
fn write_activation_script(
    installation_root: &Path,
    toolchain: &ManagedToolchain,
) -> Result<PathBuf, DevelopmentError> {
    let path = installation_root.join(activation_script_name());

    let rust_bin = managed_toolchain_bin_relative(installation_root, toolchain)?
        .to_string_lossy()
        .replace('/', "\\");

    let source = format!(
        "@echo off\r\n\
             rem Generated by Vapor.\r\n\
             rem This script is intentionally relocatable with the App Instance.\r\n\
             set \"VAPOR_HOME=\"\r\n\
             set \"VAPOR_ROOT=%~dp0\"\r\n\
             set \"PATH=%VAPOR_ROOT%bin;%VAPOR_ROOT%{rust_bin};%PATH%\"\r\n\
             set \"VAPOR_ROOT=\"\r\n"
    );

    fs::write(&path, source).map_err(|source| DevelopmentError::Io {
        path: path.clone(),
        source,
    })?;

    Ok(path)
}

fn executable_name(stem: &str) -> String {
    format!("{stem}{}", env::consts::EXE_SUFFIX,)
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
