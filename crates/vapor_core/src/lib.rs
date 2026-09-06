//! Core semantic model and orchestration primitives for Vapor.

#![forbid(unsafe_code)]

pub mod cargo;
pub mod cargo_reconciliation;
pub mod cli;
pub mod content;
pub mod development;
pub mod identity;
pub mod installation;
pub mod local;
pub mod manifest;
pub mod resolution;
pub mod role;
pub mod toolchain;
pub mod workspace;

pub use cargo::{
    CargoInspectionError, CargoPackageInspection, CargoRealization, CargoRealizationError,
    CargoTargetInspection, build_cargo_realization, generate_local_cargo_realization,
    inspect_local_cargo_package, run_cargo_realization,
};

pub use cargo_reconciliation::{
    CargoDependencyReconciliation, CargoDependencyState, CargoReconciliationError,
    CargoRepairReport, LibraryCargoReconciliation, repair_local_library_cargo_dependencies,
    verify_local_library_cargo_dependencies,
};

pub use cli::{CliSurface, run_cli};

pub use content::{ContentKind, DependencySpec};

pub use development::{
    DevelopmentError, DevelopmentOperation, development_target_dir, run_workspace_operation,
};

pub use identity::{ContentVersionId, ParseVaporIdError, VaporId};

pub use installation::{
    InstallationError, InstallationRootSource, VAPOR_HOME_ENV, VaporInstallation,
};

pub use local::{
    CONTENT_MANIFEST_FILE_NAME, LocalCatalog, LocalContent, LocalDiscoveryError,
    discover_local_content,
};

pub use manifest::{ContentHeader, ContentManifest, ManifestError, parse_content_manifest};

pub use resolution::{
    ResolutionError, ResolvedComposition, ResolvedContentGraph, ResolvedContentNode,
    resolve_local_content, resolve_local_content_kind, resolve_local_pack,
    resolve_local_packagepack, validate_resolved_content_graph,
};

pub use role::{
    ParseVaporRoleError, RoleError, RoleStatus, RoleTransitionReport, VaporRole, demote_role,
    git_available, installed_role, promote_role, role_status,
};

pub use toolchain::{ManagedToolchain, ToolchainError};

pub use workspace::{
    ToolchainPin, VaporProject, VaporWorkspace, WORKSPACE_MANIFEST_FILE_NAME, WorkspaceError,
    WorkspaceHeader, WorkspaceManifest, WorkspaceProjectSpec,
};
