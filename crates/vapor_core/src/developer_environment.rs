//! Role-owned Vapor developer environment.
//!
//! Developer roles declare complete local capabilities. Feature commands consume
//! those capabilities; they do not invent tool-specific setup flows.

use crate::{
    CrossTargetToolchainError, InstallationError, ManagedToolchain, ManagedTracyToolset,
    OperationContext, OperationOutput, OperationPhase, Progress, ToolchainError, TracyError,
    VaporInstallation,
};
use std::fmt;

#[derive(Debug, Clone)]
pub struct DeveloperEnvironmentStatus {
    pub rust_installed: bool,
    pub rust_analyzer_installed: bool,
    pub windows_gnu_target_installed: bool,
    pub windows_gnu_linker_installed: bool,
    pub tracy_profiler_installed: bool,
    pub tracy_analysis_installed: bool,
}

impl DeveloperEnvironmentStatus {
    pub const fn is_ready(&self) -> bool {
        self.rust_installed
            && self.rust_analyzer_installed
            && self.windows_gnu_target_installed
            && self.windows_gnu_linker_installed
            && self.tracy_profiler_installed
            && self.tracy_analysis_installed
    }
}

#[derive(Debug, Clone)]
pub struct DeveloperEnvironmentRepair {
    pub rust_installed: bool,
    pub rust_analyzer_installed: bool,
    pub windows_gnu_target_installed: bool,
    pub windows_gnu_linker_installed: bool,
    pub tracy_profiler_installed: bool,
    pub tracy_analysis_installed: bool,
    pub status: DeveloperEnvironmentStatus,
}

impl DeveloperEnvironmentRepair {
    pub const fn changed(&self) -> bool {
        self.rust_installed
            || self.rust_analyzer_installed
            || self.windows_gnu_target_installed
            || self.windows_gnu_linker_installed
            || self.tracy_profiler_installed
            || self.tracy_analysis_installed
    }
}

#[derive(Debug, Clone)]
pub struct DeveloperEnvironment {
    pub installation: VaporInstallation,
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
        Ok(Self {
            installation,
            rust,
            tracy,
        })
    }

    /// Discover developer tools for a specific Vapor Installation.
    pub fn discover_for_installation(
        installation: &VaporInstallation,
    ) -> Result<Self, DeveloperEnvironmentError> {
        let rust = ManagedToolchain::discover_for_installation(installation)
            .map_err(DeveloperEnvironmentError::Toolchain)?;
        let tracy = ManagedTracyToolset::discover(&installation.user_data_root())
            .map_err(DeveloperEnvironmentError::Tracy)?;
        Ok(Self {
            installation: installation.clone(),
            rust,
            tracy,
        })
    }

    pub fn status(&self) -> DeveloperEnvironmentStatus {
        DeveloperEnvironmentStatus {
            rust_installed: self.rust.is_installed(),
            rust_analyzer_installed: self.rust.rust_analyzer_path.is_file(),
            windows_gnu_target_installed: self
                .rust
                .target_is_installed(crate::toolchain::WINDOWS_GNU_TARGET),
            windows_gnu_linker_installed: crate::windows_gnu_linker_installed(&self.installation),
            tracy_profiler_installed: self.tracy.profiler_installed(),
            tracy_analysis_installed: self.tracy.analysis_installed(),
        }
    }

    /// Reconcile every tool required by a Vapor developer role.
    ///
    /// Role promotion, toolchain install/repair and broad installation repair all
    /// converge on this operation. Runtime/Devtools commands never install pieces.
    pub fn reconcile(&mut self) -> Result<DeveloperEnvironmentRepair, DeveloperEnvironmentError> {
        self.reconcile_inner(None)
    }

    /// Reconcile the developer environment while reporting named stages and
    /// live output from managed toolchain processes to the owning operation.
    pub fn reconcile_with_context(
        &mut self,
        context: &OperationContext,
    ) -> Result<DeveloperEnvironmentRepair, DeveloperEnvironmentError> {
        context.set_phase(OperationPhase::Installing);
        self.reconcile_inner(Some(context))
    }

    fn reconcile_inner(
        &mut self,
        context: Option<&OperationContext>,
    ) -> Result<DeveloperEnvironmentRepair, DeveloperEnvironmentError> {
        let rust_installed = !self.rust.is_installed();
        let rust_analyzer_installed = !self.rust.rust_analyzer_path.is_file();
        let windows_gnu_target_installed = !self
            .rust
            .target_is_installed(crate::toolchain::WINDOWS_GNU_TARGET);
        let windows_gnu_linker_installed = !crate::windows_gnu_linker_installed(&self.installation);
        if rust_installed || rust_analyzer_installed || windows_gnu_target_installed {
            report_stage(
                context,
                0,
                3,
                "Installing pinned Rust toolchain and components",
            );
            self.rust
                .install_with_output(|stream, process, text| {
                    if let Some(context) = context {
                        context.report_output(OperationOutput {
                            stream,
                            level: crate::ConsoleLevel::Info,
                            process: Some(process.to_owned()),
                            diagnostic: None,
                            raw_payload: None,
                            text: text.to_owned(),
                        });
                    }
                })
                .map_err(DeveloperEnvironmentError::Toolchain)?;
        }

        if windows_gnu_linker_installed {
            report_stage(context, 1, 3, "Installing Windows GNU linker");
            crate::install_windows_gnu_linker(&self.installation)
                .map_err(DeveloperEnvironmentError::CrossTargetToolchain)?;
        }

        report_stage(context, 2, 3, "Installing Tracy tools");
        let tracy = self
            .tracy
            .install_missing()
            .map_err(DeveloperEnvironmentError::Tracy)?;
        report_stage(context, 3, 3, "Developer setup complete");

        Ok(DeveloperEnvironmentRepair {
            rust_installed,
            rust_analyzer_installed,
            windows_gnu_target_installed,
            windows_gnu_linker_installed,
            tracy_profiler_installed: tracy.profiler_installed,
            tracy_analysis_installed: tracy.analysis_installed,
            status: self.status(),
        })
    }

    pub fn launch_tracy(&self) -> Result<(), DeveloperEnvironmentError> {
        self.tracy
            .launch()
            .map_err(DeveloperEnvironmentError::Tracy)
    }
}

fn report_stage(context: Option<&OperationContext>, completed: u64, total: u64, message: &str) {
    let Some(context) = context else { return };
    context.report_progress(Progress {
        completed,
        total: Some(total),
        unit: "setup stage".to_owned(),
        message: Some(message.to_owned()),
    });
}

#[derive(Debug)]
pub enum DeveloperEnvironmentError {
    Installation(InstallationError),
    CrossTargetToolchain(CrossTargetToolchainError),
    Toolchain(ToolchainError),
    Tracy(TracyError),
}

impl fmt::Display for DeveloperEnvironmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installation(error) => error.fmt(formatter),
            Self::CrossTargetToolchain(error) => error.fmt(formatter),
            Self::Toolchain(error) => error.fmt(formatter),
            Self::Tracy(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for DeveloperEnvironmentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Installation(error) => Some(error),
            Self::CrossTargetToolchain(error) => Some(error),
            Self::Toolchain(error) => Some(error),
            Self::Tracy(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DeveloperEnvironmentStatus;

    fn status(rust_analyzer_installed: bool) -> DeveloperEnvironmentStatus {
        DeveloperEnvironmentStatus {
            rust_installed: true,
            rust_analyzer_installed,
            windows_gnu_target_installed: true,
            windows_gnu_linker_installed: true,
            tracy_profiler_installed: true,
            tracy_analysis_installed: true,
        }
    }

    #[test]
    fn developer_environment_is_not_ready_without_rust_analyzer() {
        assert!(!status(false).is_ready());
    }

    #[test]
    fn developer_environment_is_ready_with_rust_analyzer() {
        assert!(status(true).is_ready());
    }
}
