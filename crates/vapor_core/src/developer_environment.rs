//! Role-owned Vapor developer environment.
//!
//! Developer roles declare complete local capabilities. Feature commands consume
//! those capabilities; they do not invent tool-specific setup flows.

use crate::{
    InstallationError, ManagedToolchain, ManagedTracyToolset, ToolchainError,
    TracyError, VaporInstallation,
};
use std::fmt;

#[derive(Debug, Clone)]
pub struct DeveloperEnvironmentStatus {
    pub rust_installed: bool,
    pub tracy_profiler_installed: bool,
    pub tracy_analysis_installed: bool,
}

impl DeveloperEnvironmentStatus {
    pub const fn is_ready(&self) -> bool {
        self.rust_installed
            && self.tracy_profiler_installed
            && self.tracy_analysis_installed
    }
}

#[derive(Debug, Clone)]
pub struct DeveloperEnvironmentRepair {
    pub rust_installed: bool,
    pub tracy_profiler_installed: bool,
    pub tracy_analysis_installed: bool,
    pub status: DeveloperEnvironmentStatus,
}

impl DeveloperEnvironmentRepair {
    pub const fn changed(&self) -> bool {
        self.rust_installed
            || self.tracy_profiler_installed
            || self.tracy_analysis_installed
    }
}

#[derive(Debug, Clone)]
pub struct DeveloperEnvironment {
    pub rust: ManagedToolchain,
    pub tracy: ManagedTracyToolset,
}

impl DeveloperEnvironment {
    pub fn discover() -> Result<Self, DeveloperEnvironmentError> {
        let installation =
            VaporInstallation::discover().map_err(DeveloperEnvironmentError::Installation)?;
        let rust = ManagedToolchain::discover().map_err(DeveloperEnvironmentError::Toolchain)?;
        let tracy = ManagedTracyToolset::discover(&installation.user_data_root())
            .map_err(DeveloperEnvironmentError::Tracy)?;
        Ok(Self { rust, tracy })
    }

    pub fn status(&self) -> DeveloperEnvironmentStatus {
        DeveloperEnvironmentStatus {
            rust_installed: self.rust.is_installed(),
            tracy_profiler_installed: self.tracy.profiler_installed(),
            tracy_analysis_installed: self.tracy.analysis_installed(),
        }
    }

    /// Reconcile every tool required by a Vapor developer role.
    ///
    /// Role promotion, toolchain install/repair and broad installation repair all
    /// converge on this operation. Runtime/Devtools commands never install pieces.
    pub fn reconcile(&mut self) -> Result<DeveloperEnvironmentRepair, DeveloperEnvironmentError> {
        let mut rust_installed = false;
        if !self.rust.is_installed() {
            self.rust.install().map_err(DeveloperEnvironmentError::Toolchain)?;
            rust_installed = true;
        }

        let tracy = self
            .tracy
            .install_missing()
            .map_err(DeveloperEnvironmentError::Tracy)?;

        Ok(DeveloperEnvironmentRepair {
            rust_installed,
            tracy_profiler_installed: tracy.profiler_installed,
            tracy_analysis_installed: tracy.analysis_installed,
            status: self.status(),
        })
    }

    pub fn launch_tracy(&self) -> Result<(), DeveloperEnvironmentError> {
        self.tracy.launch().map_err(DeveloperEnvironmentError::Tracy)
    }
}

#[derive(Debug)]
pub enum DeveloperEnvironmentError {
    Installation(InstallationError),
    Toolchain(ToolchainError),
    Tracy(TracyError),
}

impl fmt::Display for DeveloperEnvironmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installation(error) => error.fmt(formatter),
            Self::Toolchain(error) => error.fmt(formatter),
            Self::Tracy(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for DeveloperEnvironmentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Installation(error) => Some(error),
            Self::Toolchain(error) => Some(error),
            Self::Tracy(error) => Some(error),
        }
    }
}
