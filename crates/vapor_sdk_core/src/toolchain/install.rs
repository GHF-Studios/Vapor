//! Toolchain installation through Rustup using Vapor-owned state roots.

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use super::plan::{ToolchainInstallPlan, ToolchainPlanError, toolchain_install_plan};

/// Result of installing the canonical Vapor toolchain.
#[derive(Debug, Clone)]
pub struct ToolchainInstallReport {
    pub plan: ToolchainInstallPlan,
    pub installed_root: PathBuf,
    pub exit_status: ExitStatus,
}

/// Install the canonical Vapor toolchain through Rustup.
pub fn toolchain_install() -> Result<ToolchainInstallReport, ToolchainInstallError> {
    let plan = toolchain_install_plan()?;
    let installed_root = plan.status.toolchain_root.clone();

    if installed_root.exists() {
        return Err(ToolchainInstallError::AlreadyInstalled(installed_root));
    }

    fs::create_dir_all(&plan.status.rustup_home)?;
    fs::create_dir_all(&plan.status.cargo_home)?;

    let exit_status = Command::new(&plan.rustup_path)
        .args(&plan.rustup_args)
        .env("RUSTUP_HOME", &plan.status.rustup_home)
        .env("CARGO_HOME", &plan.status.cargo_home)
        .env("RUSTUP_TOOLCHAIN", plan.status.toolchain.identifier())
        .status()?;

    if !exit_status.success() {
        return Err(ToolchainInstallError::RustupFailed {
            rustup: plan.rustup_path.clone(),
            status: exit_status,
        });
    }

    verify_rustup_toolchain(&plan.status.toolchain_root)?;
    verify_rustup_toolchain_binary(&plan.status.cargo_path)?;
    verify_rustup_toolchain_binary(&plan.status.rustc_path)?;

    Ok(ToolchainInstallReport {
        plan,
        installed_root,
        exit_status,
    })
}

fn verify_rustup_toolchain(toolchain_root: &Path) -> Result<(), ToolchainInstallError> {
    if toolchain_root.is_dir() {
        Ok(())
    } else {
        Err(ToolchainInstallError::MissingInstalledToolchain(
            toolchain_root.to_path_buf(),
        ))
    }
}

fn verify_rustup_toolchain_binary(path: &Path) -> Result<(), ToolchainInstallError> {
    if path.is_file() {
        Ok(())
    } else {
        Err(ToolchainInstallError::MissingToolchainBinary(
            path.to_path_buf(),
        ))
    }
}

/// Error returned while installing the canonical Vapor toolchain.
#[derive(Debug)]
pub enum ToolchainInstallError {
    Plan(ToolchainPlanError),
    Io(std::io::Error),
    AlreadyInstalled(PathBuf),
    RustupFailed { rustup: PathBuf, status: ExitStatus },
    MissingInstalledToolchain(PathBuf),
    MissingToolchainBinary(PathBuf),
}

impl From<ToolchainPlanError> for ToolchainInstallError {
    fn from(error: ToolchainPlanError) -> Self {
        Self::Plan(error)
    }
}

impl From<std::io::Error> for ToolchainInstallError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl fmt::Display for ToolchainInstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plan(error) => write!(formatter, "{error}"),
            Self::Io(error) => write!(formatter, "toolchain install I/O failed: {error}"),
            Self::AlreadyInstalled(path) => write!(
                formatter,
                "toolchain is already installed at `{}`",
                path.display()
            ),
            Self::RustupFailed { rustup, status } => write!(
                formatter,
                "Rustup `{}` failed with {status}",
                rustup.display()
            ),
            Self::MissingInstalledToolchain(path) => write!(
                formatter,
                "Rustup did not create expected toolchain root `{}`",
                path.display()
            ),
            Self::MissingToolchainBinary(path) => write!(
                formatter,
                "Rustup did not create expected toolchain binary `{}`",
                path.display()
            ),
        }
    }
}

impl Error for ToolchainInstallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Plan(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::AlreadyInstalled(_)
            | Self::RustupFailed { .. }
            | Self::MissingInstalledToolchain(_)
            | Self::MissingToolchainBinary(_) => None,
        }
    }
}
