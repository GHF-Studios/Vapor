//! Vapor-managed development operations.
//!
//! A Vapor development operation acts on semantic Vapor Projects while using
//! the Workspace's pinned managed toolchain.

use crate::{ManagedToolchain, ToolchainError, VaporProject, VaporWorkspace};
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

const DEVELOPMENT_DIR: &str = "development";
const TARGET_DIR: &str = "target";

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

#[derive(Debug)]
pub enum DevelopmentError {
    Toolchain(ToolchainError),

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
}

impl fmt::Display for DevelopmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toolchain(error) => error.fmt(formatter),

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
        }
    }
}

impl std::error::Error for DevelopmentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Toolchain(error) => Some(error),
            Self::CargoStart { source, .. } => Some(source),
            Self::CargoFailed { .. } => None,
        }
    }
}
