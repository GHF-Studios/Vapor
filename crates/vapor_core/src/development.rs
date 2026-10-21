//! Vapor-managed development operations.
//!
//! Development happens against external authored source while build outputs
//! and managed tooling belong operationally to the active Vapor Installation.

use crate::{
    ManagedToolchain, OperationContext, OperationPhase, Progress, ToolchainError,
    VaporInstallation, VaporProject, VaporSuperworkspace, VaporWorkspace,
    install_ecosystem_bootstrap, reconcile_existing_development_environment,
    resolve_source_context,
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
const CLIENT_DISTRIBUTION_MANIFEST_FILE_NAME: &str = "Vapor-Client.vapor.toml";
const CLIENT_WORKSPACE_REPOSITORY: &str = "Vapor-Client/Vapor";
const CLIENT_WORKSPACE_ORGANIZATION: &str = "ghf-studios";
const CLIENT_WORKSPACE_NAME: &str = "vapor";
const LOCAL_DEPLOYMENT_AUTHORITY_ENV: &str = "VAPOR_LOCAL_DEPLOYMENT_AUTHORITY";

const DISTRIBUTION_BINARIES: &[&str] = &[
    "vapor",
    "vapor-installer",
    "vapor-entrypoint",
    "vapor-gui",
    "vapor-launcher",
    "vapor-studio",
    "vapor-ide",
    "vapor-installer-gui",
    "vapor-ide-host",
    "vapor-client-service",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevelopmentOperation {
    Fmt,
    Check,
    Build,
    Test,
    Clean,
}

impl DevelopmentOperation {
    fn cargo_subcommand(self) -> &'static str {
        match self {
            Self::Fmt => "fmt",
            Self::Check => "check",
            Self::Build => "build",
            Self::Test => "test",
            Self::Clean => "clean",
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
    pub user_data_root: PathBuf,
    pub binaries: Vec<BuiltBinary>,
    pub launch_script: PathBuf,
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
    pub launch_script: PathBuf,
    pub activation_script: PathBuf,
}

#[derive(Debug, Clone)]
pub struct LocalDeploymentAuthority {
    pub executable: PathBuf,
    pub installation_root: PathBuf,
}

#[derive(Debug, Clone)]
pub enum LocalClientDeploymentOutcome {
    AuthorityDelegated {
        executable: PathBuf,
        installation_root: PathBuf,
    },
    Deployed {
        report: EcosystemDeploymentReport,
        source_bootstrap: PathBuf,
        synchronization_warning: Option<String>,
    },
}

/// Deploy the active Vapor Client workspace into the local Vapor App Instance.
///
/// The first invocation builds and transfers authority to the source-built CLI;
/// that CLI re-enters this operation and performs the promotion. This keeps
/// the deployed source, rather than the currently installed binary, in charge.
pub fn deploy_client_locally() -> Result<LocalClientDeploymentOutcome, DevelopmentError> {
    let workspace = client_workspace()?;

    if !local_deployment_authority_active()? {
        let authority = build_workspace_deployment_authority(&workspace)?;
        run_workspace_deployment_authority(&workspace, &authority)?;
        return Ok(LocalClientDeploymentOutcome::AuthorityDelegated {
            executable: authority.executable,
            installation_root: authority.installation_root,
        });
    }

    let installation = VaporInstallation::discover()
        .map_err(|error| DevelopmentError::Workspace(error.to_string()))?;
    let authority_executable = env::current_exe().map_err(DevelopmentError::CurrentExecutable)?;
    let report = deploy_workspace_from_authority(&workspace, &authority_executable)?;
    let source_bootstrap = install_ecosystem_bootstrap(&installation, &workspace.root)
        .map_err(|error| DevelopmentError::Workspace(error.to_string()))?;
    let synchronization_warning = reconcile_existing_development_environment(&workspace.root)
        .err()
        .map(|error| error.to_string());

    Ok(LocalClientDeploymentOutcome::Deployed {
        report,
        source_bootstrap,
        synchronization_warning,
    })
}

fn client_workspace() -> Result<VaporWorkspace, DevelopmentError> {
    if let Ok(workspace) = VaporWorkspace::discover()
        && workspace.manifest.workspace.organization == CLIENT_WORKSPACE_ORGANIZATION
        && workspace.manifest.workspace.name == CLIENT_WORKSPACE_NAME
    {
        return Ok(workspace);
    }

    let superworkspace = first_party_superworkspace()?;
    let root = superworkspace
        .repositories
        .iter()
        .find(|repository| repository.name == CLIENT_WORKSPACE_REPOSITORY)
        .map(|repository| repository.root.clone())
        .ok_or_else(|| {
            DevelopmentError::Workspace(format!(
                "first-party Vapor source `{CLIENT_WORKSPACE_REPOSITORY}` is not present in Superworkspace `{}`",
                superworkspace.root.display()
            ))
        })?;

    VaporWorkspace::load(&root).map_err(|error| {
        DevelopmentError::Workspace(format!(
            "failed to load first-party Vapor Client Workspace `{}`: {error}",
            root.display()
        ))
    })
}

fn first_party_superworkspace() -> Result<VaporSuperworkspace, DevelopmentError> {
    if let Ok(current_directory) = env::current_dir()
        && let Ok(superworkspace) = VaporSuperworkspace::discover_from(&current_directory)
    {
        return Ok(superworkspace);
    }

    let installation = VaporInstallation::discover()
        .map_err(|error| DevelopmentError::Workspace(error.to_string()))?;
    let source = resolve_source_context(&installation, None)
        .map_err(|error| DevelopmentError::Workspace(error.to_string()))?;

    VaporSuperworkspace::discover_from(&source.root).map_err(|error| {
        DevelopmentError::Workspace(format!(
            "failed to resolve Vapor Superworkspace from source `{}`: {error}",
            source.root.display()
        ))
    })
}

pub fn run_workspace_operation(
    workspace: &VaporWorkspace,
    operation: DevelopmentOperation,
) -> Result<(), DevelopmentError> {
    run_workspace_operation_inner(workspace, operation, None)
}

/// Run one workspace operation while reporting resolution and per-project
/// progress through the shared frontend operation context.
pub fn run_workspace_operation_with_context(
    workspace: &VaporWorkspace,
    operation: DevelopmentOperation,
    context: &OperationContext,
) -> Result<(), DevelopmentError> {
    run_workspace_operation_inner(workspace, operation, Some(context))
}

fn run_workspace_operation_inner(
    workspace: &VaporWorkspace,
    operation: DevelopmentOperation,
    context: Option<&OperationContext>,
) -> Result<(), DevelopmentError> {
    let toolchain =
        ManagedToolchain::for_workspace(workspace).map_err(DevelopmentError::Toolchain)?;

    let total = workspace.projects.len() as u64;
    for (index, project) in workspace.projects.iter().enumerate() {
        if let Some(context) = context {
            context.set_phase(OperationPhase::Building);
        }
        run_project_operation(&toolchain, project, operation)?;
        if let Some(context) = context {
            context.report_progress(Progress {
                completed: index as u64 + 1,
                total: Some(total),
                unit: "projects".to_owned(),
                message: None,
            });
        }
    }

    Ok(())
}

/// Run one Vapor-managed development operation for the workspace containing
/// `workspace_root`. GUI and CLI callers use the same implementation; callers
/// that have an explicit project selection should pass that root instead of
/// changing process-wide current-directory state.
pub fn run_workspace_operation_at(
    workspace_root: &Path,
    operation: DevelopmentOperation,
) -> Result<VaporWorkspace, DevelopmentError> {
    let workspace = VaporWorkspace::discover_from(workspace_root)
        .map_err(|error| DevelopmentError::Workspace(error.to_string()))?;
    run_workspace_operation(&workspace, operation)?;
    Ok(workspace)
}

/// Context-reporting form of [`run_workspace_operation_at`].
pub fn run_workspace_operation_at_with_context(
    workspace_root: &Path,
    operation: DevelopmentOperation,
    context: &OperationContext,
) -> Result<VaporWorkspace, DevelopmentError> {
    context.set_phase(OperationPhase::Resolving);
    let workspace = VaporWorkspace::discover_from(workspace_root)
        .map_err(|error| DevelopmentError::Workspace(error.to_string()))?;
    context.set_phase(OperationPhase::Building);
    run_workspace_operation_with_context(&workspace, operation, context)?;
    Ok(workspace)
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
    build_workspace_deployment_inputs_with_authority(workspace, None)
}

fn build_workspace_deployment_inputs_with_authority(
    workspace: &VaporWorkspace,
    authority_executable: Option<&Path>,
) -> Result<EcosystemBuildReport, DevelopmentError> {
    let toolchain =
        ManagedToolchain::for_workspace(workspace).map_err(DevelopmentError::Toolchain)?;

    let toolchain_metadata = toolchain
        .persist_installation_metadata()
        .map_err(DevelopmentError::Toolchain)?;

    let installation_root = toolchain.vapor_home.clone();
    let user_data_root = toolchain.user_data_root.clone();

    let mut binaries = Vec::new();
    let projects = deployment_projects(workspace);

    for &binary in DISTRIBUTION_BINARIES {
        let source = if binary == "vapor" {
            match authority_executable {
                Some(authority) => {
                    if !authority.is_file() {
                        return Err(DevelopmentError::MissingBuiltExecutable {
                            binary: binary.to_owned(),
                            path: authority.to_path_buf(),
                        });
                    }
                    authority.to_path_buf()
                }
                None => build_distribution_binary(&toolchain, &projects, binary)?,
            }
        } else {
            build_distribution_binary(&toolchain, &projects, binary)?
        };

        binaries.push(BuiltBinary {
            name: binary.to_owned(),
            source,
        });
    }

    let launch_script = client_launch_script(workspace)?;
    let activation_script = write_activation_script(&installation_root, &toolchain)?;

    Ok(EcosystemBuildReport {
        installation_root,
        user_data_root,
        binaries,
        launch_script,
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
    promote_workspace_build(build)
}

pub fn deploy_workspace_from_authority(
    workspace: &VaporWorkspace,
    authority_executable: &Path,
) -> Result<EcosystemDeploymentReport, DevelopmentError> {
    let build =
        build_workspace_deployment_inputs_with_authority(workspace, Some(authority_executable))?;
    promote_workspace_build(build)
}

fn promote_workspace_build(
    build: EcosystemBuildReport,
) -> Result<EcosystemDeploymentReport, DevelopmentError> {
    let bin_root = build
        .installation_root
        .join(BIN_DIR)
        .join(current_host_target()?);

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

    // Remove retired executables so the app root contains only current launch surfaces.
    for legacy in [
        "vapor-devtools",
        "vapor-studio-service",
        "vapor-monitor",
        "vapor-profile",
    ] {
        let path = bin_root.join(executable_name(legacy));
        if path.is_file() {
            fs::remove_file(&path).map_err(|source| DevelopmentError::Io { path, source })?;
        }
    }

    let launch_script =
        build
            .installation_root
            .join(BIN_DIR)
            .join(build.launch_script.file_name().ok_or_else(|| {
                DevelopmentError::InvalidDeploymentPath {
                    path: build.launch_script.clone(),
                }
            })?);
    promote_file(&build.launch_script, &launch_script)?;

    Ok(EcosystemDeploymentReport {
        installation_root: build.installation_root,
        binaries,
        launch_script,
        activation_script: build.activation_script,
    })
}

pub fn development_target_dir(toolchain: &ManagedToolchain, project: &VaporProject) -> PathBuf {
    toolchain
        .user_data_root
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

    // Development targets are Vapor-owned disposable state. Clean them
    // directly rather than asking Cargo to infer a target directory from the
    // authored Workspace. This is both cheaper and exactly matches `vapor run`.
    if operation == DevelopmentOperation::Clean {
        return match fs::remove_dir_all(&target_dir) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(DevelopmentError::Io {
                path: target_dir,
                source,
            }),
        };
    }

    let mut command = toolchain
        .cargo_command()
        .map_err(DevelopmentError::Toolchain)?;
    command.arg(operation.cargo_subcommand());
    if operation == DevelopmentOperation::Fmt {
        command.arg("--all");
    } else {
        command.arg("--workspace");
    }
    let status = command
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

fn deployment_projects(workspace: &VaporWorkspace) -> Vec<VaporProject> {
    let mut projects = VaporSuperworkspace::discover_from(&workspace.root)
        .map(|superworkspace| {
            superworkspace
                .projects
                .into_iter()
                .map(|project| project.project)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|_| workspace.projects.clone());

    projects.sort_by(|left, right| left.cargo_manifest_path.cmp(&right.cargo_manifest_path));
    projects.dedup_by(|left, right| left.cargo_manifest_path == right.cargo_manifest_path);
    projects
}

fn client_launch_script(workspace: &VaporWorkspace) -> Result<PathBuf, DevelopmentError> {
    let client_root = workspace
        .root
        .ancestors()
        .find(|root| root.join(CLIENT_DISTRIBUTION_MANIFEST_FILE_NAME).is_file())
        .ok_or_else(|| DevelopmentError::ClientDistributionNotFound {
            start: workspace.root.clone(),
        })?;

    let relative = if cfg!(windows) {
        Path::new("resources/vapor/shell-scripts/windows/vapor-launch.cmd")
    } else {
        Path::new("resources/vapor/shell-scripts/linux/vapor-launch.sh")
    };
    let path = client_root.join(relative);

    if !path.is_file() {
        return Err(DevelopmentError::MissingLaunchScript { path });
    }

    Ok(path)
}

#[derive(Debug, Clone)]
struct BinaryTarget {
    project: VaporProject,
    package: String,
}

fn find_binary_target(
    toolchain: &ManagedToolchain,
    projects: &[VaporProject],
    binary: &str,
) -> Result<BinaryTarget, DevelopmentError> {
    let mut matches = Vec::new();

    for project in projects {
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

fn build_distribution_binary(
    toolchain: &ManagedToolchain,
    projects: &[VaporProject],
    binary: &str,
) -> Result<PathBuf, DevelopmentError> {
    let target = find_binary_target(toolchain, projects, binary)?;

    build_binary(toolchain, &target, binary)?;

    let source = development_target_dir(toolchain, &target.project)
        .join(DEV_PROFILE_DIR)
        .join(executable_name(binary));

    if !source.is_file() {
        return Err(DevelopmentError::MissingBuiltExecutable {
            binary: binary.to_owned(),
            path: source,
        });
    }

    Ok(source)
}

pub fn build_workspace_deployment_authority(
    workspace: &VaporWorkspace,
) -> Result<LocalDeploymentAuthority, DevelopmentError> {
    let toolchain =
        ManagedToolchain::for_workspace(workspace).map_err(DevelopmentError::Toolchain)?;

    let projects = deployment_projects(workspace);
    let executable = build_distribution_binary(&toolchain, &projects, "vapor")?;

    Ok(LocalDeploymentAuthority {
        executable,
        installation_root: toolchain.vapor_home,
    })
}

pub fn local_deployment_authority_active() -> Result<bool, DevelopmentError> {
    let Some(expected) = env::var_os(LOCAL_DEPLOYMENT_AUTHORITY_ENV) else {
        return Ok(false);
    };

    let expected_path = PathBuf::from(expected);
    let expected = fs::canonicalize(&expected_path).map_err(|source| DevelopmentError::Io {
        path: expected_path,
        source,
    })?;

    let current_path = env::current_exe().map_err(DevelopmentError::CurrentExecutable)?;
    let current = fs::canonicalize(&current_path).map_err(|source| DevelopmentError::Io {
        path: current_path,
        source,
    })?;

    if current != expected {
        return Err(DevelopmentError::DeploymentAuthorityMismatch { expected, current });
    }

    Ok(true)
}

pub fn run_workspace_deployment_authority(
    workspace: &VaporWorkspace,
    authority: &LocalDeploymentAuthority,
) -> Result<(), DevelopmentError> {
    let canonical_authority =
        fs::canonicalize(&authority.executable).map_err(|source| DevelopmentError::Io {
            path: authority.executable.clone(),
            source,
        })?;

    let status = crate::managed_process::background_command(&canonical_authority)
        .args(["client", "deploy", "local"])
        .current_dir(&workspace.root)
        .env(crate::VAPOR_HOME_ENV, &authority.installation_root)
        .env(LOCAL_DEPLOYMENT_AUTHORITY_ENV, &canonical_authority)
        .status()
        .map_err(|source| DevelopmentError::DeploymentAuthorityStart {
            executable: canonical_authority.clone(),
            source,
        })?;

    if status.success() {
        Ok(())
    } else {
        Err(DevelopmentError::DeploymentAuthorityFailed {
            executable: canonical_authority,
            status,
        })
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

fn managed_toolchain_bin_user_data_relative(
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
        .strip_prefix(&toolchain.user_data_root)
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

    let rust_bin = managed_toolchain_bin_user_data_relative(toolchain)?
        .to_string_lossy()
        .replace('\\', "/");
    let host = current_host_target()?;

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

if [[ -n "${{XDG_DATA_HOME:-}}" ]]; then
    VAPOR_USER_DATA="$XDG_DATA_HOME/vapor"
else
    VAPOR_USER_DATA="$HOME/.local/share/vapor"
fi

VAPOR_BIN="$VAPOR_ROOT/bin/{host}"
VAPOR_RUST_BIN="$VAPOR_USER_DATA/{rust_bin}"

for VAPOR_PATH in "$VAPOR_RUST_BIN" "$VAPOR_BIN"; do
    case ":$PATH:" in
        *":$VAPOR_PATH:"*) ;;
        *) export PATH="$VAPOR_PATH${{PATH:+:$PATH}}" ;;
    esac
done

unset VAPOR_PATH
unset VAPOR_BIN
unset VAPOR_RUST_BIN
unset VAPOR_USER_DATA
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

    let rust_bin = managed_toolchain_bin_user_data_relative(toolchain)?
        .to_string_lossy()
        .replace('/', "\\");
    let host = current_host_target()?;

    let source = format!(
        "@echo off\r\n\
             rem Generated by Vapor.\r\n\
             rem This script is intentionally relocatable with the App Instance.\r\n\
             set \"VAPOR_HOME=\"\r\n\
             set \"VAPOR_ROOT=%~dp0\"\r\n\
             if defined LOCALAPPDATA (set \"VAPOR_USER_DATA=%LOCALAPPDATA%\\Vapor\") else (set \"VAPOR_USER_DATA=%USERPROFILE%\\AppData\\Local\\Vapor\")\r\n\
             set \"PATH=%VAPOR_ROOT%bin\\{host};%VAPOR_USER_DATA%\\{rust_bin};%PATH%\"\r\n\
             set \"VAPOR_USER_DATA=\"\r\n\
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

fn current_host_target() -> Result<&'static str, DevelopmentError> {
    crate::HostTarget::current()
        .map(|target| target.triple())
        .map_err(|_| DevelopmentError::UnsupportedHost)
}

#[derive(Debug)]
pub enum DevelopmentError {
    Toolchain(ToolchainError),

    Workspace(String),

    CurrentExecutable(io::Error),

    DeploymentAuthorityMismatch {
        expected: PathBuf,
        current: PathBuf,
    },

    DeploymentAuthorityStart {
        executable: PathBuf,
        source: io::Error,
    },

    DeploymentAuthorityFailed {
        executable: PathBuf,
        status: ExitStatus,
    },

    UnsupportedHost,

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

    ClientDistributionNotFound {
        start: PathBuf,
    },

    MissingLaunchScript {
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

            Self::Workspace(error) => formatter.write_str(error),

            Self::CurrentExecutable(error) => {
                write!(
                    formatter,
                    "failed to resolve the running Vapor executable: {error}"
                )
            }

            Self::DeploymentAuthorityMismatch { expected, current } => {
                write!(
                    formatter,
                    "local deployment authority mismatch: expected `{}`, running `{}`",
                    expected.display(),
                    current.display(),
                )
            }

            Self::DeploymentAuthorityStart { executable, source } => {
                write!(
                    formatter,
                    "failed to start source deployment authority `{}`: {source}",
                    executable.display(),
                )
            }

            Self::DeploymentAuthorityFailed { executable, status } => {
                write!(
                    formatter,
                    "source deployment authority `{}` failed with {status}",
                    executable.display(),
                )
            }

            Self::UnsupportedHost => {
                formatter.write_str("this host is not yet supported by Vapor Client deployment")
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

            Self::ClientDistributionNotFound { start } => write!(
                formatter,
                "could not find `{CLIENT_DISTRIBUTION_MANIFEST_FILE_NAME}` above Vapor Workspace `{}`",
                start.display(),
            ),

            Self::MissingLaunchScript { path } => write!(
                formatter,
                "required Vapor Client launch wrapper is missing at `{}`",
                path.display(),
            ),

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

            Self::CurrentExecutable(error) => Some(error),

            Self::DeploymentAuthorityStart { source, .. } => Some(source),

            Self::CargoStart { source, .. } => Some(source),

            Self::CargoMetadataStart { source, .. } => Some(source),

            Self::InvalidCargoMetadata { source, .. } => Some(source),

            Self::BinaryBuildStart { source, .. } => Some(source),

            Self::Io { source, .. } => Some(source),

            _ => None,
        }
    }
}

#[cfg(test)]
mod operation_tests {
    use super::{DevelopmentError, DevelopmentOperation, run_workspace_operation_at};
    use std::path::Path;

    #[test]
    fn explicit_workspace_root_is_used_for_discovery() {
        let root = Path::new("/definitely-not-a-vapor-workspace");

        let result = run_workspace_operation_at(root, DevelopmentOperation::Check);

        assert!(
            matches!(result, Err(DevelopmentError::Workspace(message)) if message.contains("workspace"))
        );
    }
}
