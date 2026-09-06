//! Cross-cutting Vapor managed-state diagnosis and repair.
//!
//! Diagnosis and repair operate on regeneratable or Vapor-managed operational
//! state. They do not own or destructively rewrite authored source.
//!
//! Development-environment integration, including external IDE integration,
//! is conceptually SDK-owned. The universal `vapor` surface may nevertheless
//! invoke the same underlying operations as part of broad diagnosis, repair,
//! source opening, and ecosystem deployment.

use crate::ide::{IdeError, IdeStatus, inspect_ide, repair_ide};
use crate::installation::{InstallationError, InstallationRootSource, VaporInstallation};
use crate::source::{SourceContextSource, SourceError, resolve_source_context};
use crate::superworkspace::{
    SuperworkspaceError, SuperworkspaceRepositoryKind, VaporSuperworkspace,
};
use crate::toolchain::{ManagedToolchain, ToolchainError};
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct MaintenanceIssue {
    pub name: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct MaintenanceStatus {
    pub installation_root: PathBuf,
    pub installation_source: InstallationRootSource,

    pub toolchain_version: String,
    pub toolchain_installed: bool,

    pub source_root: PathBuf,
    pub source_context_source: SourceContextSource,

    pub superworkspace_root: Option<PathBuf>,
    pub current_workspaces: usize,
    pub current_projects: usize,

    pub incompatible_workspaces: Vec<MaintenanceIssue>,

    pub jetbrains_present: bool,
    pub ide_status: Option<IdeStatus>,
}

impl MaintenanceStatus {
    pub fn is_healthy(&self) -> bool {
        if !self.toolchain_installed {
            return false;
        }

        if !self.jetbrains_present {
            return true;
        }

        self.ide_status.as_ref().is_some_and(IdeStatus::is_current)
    }
}

#[derive(Debug, Clone)]
pub struct MaintenanceRepairReport {
    pub toolchain_installed: bool,
    pub development_changes: Vec<PathBuf>,
    pub status: MaintenanceStatus,
}

/// Inspect the currently resolvable Vapor environment.
///
/// Legacy/incompatible checked-out Workspaces remain diagnostic information.
/// They are not active Vapor Projects and do not make the environment unhealthy
/// merely by existing.
pub fn diagnose_managed_state() -> Result<MaintenanceStatus, MaintenanceError> {
    let installation = VaporInstallation::discover().map_err(MaintenanceError::Installation)?;

    let toolchain = ManagedToolchain::discover().map_err(MaintenanceError::Toolchain)?;

    let source_context =
        resolve_source_context(&installation, None).map_err(MaintenanceError::Source)?;

    let superworkspace = optional_superworkspace(&source_context.root)?;

    let mut current_workspaces = 0;

    let mut current_projects = 0;

    let mut incompatible_workspaces = Vec::new();

    let mut superworkspace_root = None;

    let mut jetbrains_present = false;

    let mut ide_status = None;

    if let Some(superworkspace) = superworkspace {
        superworkspace_root = Some(superworkspace.root.clone());

        current_projects = superworkspace.projects.len();

        for repository in &superworkspace.repositories {
            match &repository.kind {
                SuperworkspaceRepositoryKind::ContainerRepo => {}

                SuperworkspaceRepositoryKind::VaporWorkspace => {
                    current_workspaces += 1;
                }

                SuperworkspaceRepositoryKind::IncompatibleVaporWorkspace { message } => {
                    incompatible_workspaces.push(MaintenanceIssue {
                        name: repository.name.clone(),
                        message: message.clone(),
                    });
                }
            }
        }

        jetbrains_present = superworkspace.root.join(".idea").is_dir();

        if jetbrains_present && toolchain.is_installed() && !superworkspace.projects.is_empty() {
            ide_status =
                Some(inspect_ide(&superworkspace, &toolchain).map_err(MaintenanceError::Ide)?);
        }
    }

    Ok(MaintenanceStatus {
        installation_root: installation.root,
        installation_source: installation.root_source,

        toolchain_version: toolchain.pin.version.clone(),
        toolchain_installed: toolchain.is_installed(),

        source_root: source_context.root,
        source_context_source: source_context.source,

        superworkspace_root,
        current_workspaces,
        current_projects,

        incompatible_workspaces,

        jetbrains_present,
        ide_status,
    })
}

/// Repair safe Vapor-managed state in the currently resolvable environment.
///
/// Today this includes:
///
/// - installing the pinned managed Rust toolchain if it is missing;
/// - reconciling an already-existing JetBrains/RustRover integration.
///
/// It deliberately does not modify authored source merely because diagnosis
/// found legacy or incompatible repositories.
pub fn repair_managed_state() -> Result<MaintenanceRepairReport, MaintenanceError> {
    let toolchain = ManagedToolchain::discover().map_err(MaintenanceError::Toolchain)?;

    let mut toolchain_installed = false;

    if !toolchain.is_installed() {
        toolchain.install().map_err(MaintenanceError::Toolchain)?;

        toolchain_installed = true;
    }

    let installation = VaporInstallation::discover().map_err(MaintenanceError::Installation)?;

    let source_context =
        resolve_source_context(&installation, None).map_err(MaintenanceError::Source)?;

    let development_changes = reconcile_existing_development_environment(&source_context.root)?;

    let status = diagnose_managed_state()?;

    Ok(MaintenanceRepairReport {
        toolchain_installed,
        development_changes,
        status,
    })
}

/// Proactively reconcile development-environment state that already exists.
///
/// This is intentionally non-invasive:
///
/// - no JetBrains project -> no JetBrains project is created;
/// - no recognized Superworkspace -> nothing happens;
/// - missing toolchain -> broad `repair` owns installation of it;
/// - existing JetBrains project -> Vapor reconciles its modeled Cargo projects
///   and managed Rust configuration.
///
/// This operation is suitable for ordinary successful workflows such as
/// `source open` and ecosystem deployment.
pub fn reconcile_existing_development_environment(
    source_root: &Path,
) -> Result<Vec<PathBuf>, MaintenanceError> {
    let Some(superworkspace) = optional_superworkspace(source_root)? else {
        return Ok(Vec::new());
    };

    if !superworkspace.root.join(".idea").is_dir() {
        return Ok(Vec::new());
    }

    if superworkspace.projects.is_empty() {
        return Ok(Vec::new());
    }

    let toolchain = ManagedToolchain::discover().map_err(MaintenanceError::Toolchain)?;

    if !toolchain.is_installed() {
        return Ok(Vec::new());
    }

    let report = repair_ide(&superworkspace, &toolchain).map_err(MaintenanceError::Ide)?;

    Ok(report.changed)
}

fn optional_superworkspace(start: &Path) -> Result<Option<VaporSuperworkspace>, MaintenanceError> {
    match VaporSuperworkspace::discover_from(start) {
        Ok(superworkspace) => Ok(Some(superworkspace)),

        Err(SuperworkspaceError::NotFound { .. }) => Ok(None),

        Err(error) => Err(MaintenanceError::Superworkspace(error)),
    }
}

#[derive(Debug)]
pub enum MaintenanceError {
    Installation(InstallationError),

    Toolchain(ToolchainError),

    Source(SourceError),

    Superworkspace(SuperworkspaceError),

    Ide(IdeError),
}

impl fmt::Display for MaintenanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installation(error) => error.fmt(formatter),

            Self::Toolchain(error) => error.fmt(formatter),

            Self::Source(error) => error.fmt(formatter),

            Self::Superworkspace(error) => error.fmt(formatter),

            Self::Ide(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for MaintenanceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Installation(error) => Some(error),

            Self::Toolchain(error) => Some(error),

            Self::Source(error) => Some(error),

            Self::Superworkspace(error) => Some(error),

            Self::Ide(error) => Some(error),
        }
    }
}
