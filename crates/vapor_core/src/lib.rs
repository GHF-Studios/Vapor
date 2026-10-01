//! Core semantic model and orchestration primitives for Vapor.

#![forbid(unsafe_code)]

pub mod cargo;
pub mod cargo_reconciliation;
pub mod cli;
pub mod content;
pub mod development;
pub mod developer_environment;
pub mod ecosystem;
pub mod host;
pub mod ide;
pub mod identity;
pub mod install;
pub mod installation;
pub mod local;
pub mod maintenance;
pub mod managed_tool;
pub mod manifest;
pub mod registry;
pub mod resolution;
pub mod run;
pub mod profiling;
pub mod role;
pub mod source;
pub mod steam;
pub mod superworkspace;
pub mod toolchain;
pub mod tracy;
pub mod uninstall;
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
    BuiltBinary, DeployedBinary, DevelopmentError, DevelopmentOperation, EcosystemBuildReport,
    EcosystemDeploymentReport, LocalDeploymentAuthority,
    build_workspace_deployment_authority, build_workspace_deployment_inputs, deploy_workspace,
    deploy_workspace_from_authority, development_target_dir,
    local_deployment_authority_active, run_workspace_deployment_authority,
    run_workspace_operation,
};

pub use developer_environment::{
    DeveloperEnvironment, DeveloperEnvironmentError, DeveloperEnvironmentRepair,
    DeveloperEnvironmentStatus,
};

pub use ecosystem::{
    AcquiredRepository, EcosystemAcquisitionReport, EcosystemBootstrap, EcosystemError,
    acquire_ecosystem, acquire_ecosystem_repositories, install_ecosystem_bootstrap,
};

pub use ide::{
    IdeError, IdeFileState, IdeFileStatus, IdeRepairReport, IdeStatus, inspect_ide, repair_ide,
};

pub use identity::{ContentVersionId, ParseVaporIdError, VaporId};

pub use host::{
    DisplayServer, HostAbi, HostArchitecture, HostCapability, HostEnvironment,
    HostEnvironmentError, HostOperatingSystem, HostTarget,
};

pub use install::{InstallError, InstallReport, install_installation};

pub use installation::{
    InstallationError, InstallationRootSource, VAPOR_HOME_ENV, VaporInstallation,
};

pub use local::{
    CONTENT_MANIFEST_FILE_NAME, LocalCatalog, LocalContent, LocalDiscoveryError,
    discover_local_content,
};

pub use maintenance::{
    MaintenanceError, MaintenanceIssue, MaintenanceRepairReport, MaintenanceStatus,
    diagnose_managed_state, reconcile_existing_development_environment, repair_managed_state,
};

pub use managed_tool::{
    ManagedToolError, ManagedToolRequest, ReleaseAssetStatus,
    download_verified_github_release_asset, find_executable, find_executable_on_path,
    github_release_client,
};

pub use manifest::{ContentHeader, ContentManifest, ManifestError, parse_content_manifest};

pub use registry::{RegisteredEcosystem, RegisteredRepository, RegistryClient, RegistryError};

pub use resolution::{
    ResolutionError, ResolvedComposition, ResolvedContentGraph, ResolvedContentNode,
    resolve_local_content, resolve_local_content_kind, resolve_local_pack,
    resolve_local_packagepack, validate_resolved_content_graph,
};

pub use run::{
    ActiveDevelopmentSession, DevelopmentRunError, active_development_session,
    development_sessions, run_workspace_configuration,
    run_workspace_configuration_with_profile_hint,
};

pub use profiling::{
    ProfileError, ProfileReport, ProfileReportOptions, ProfileSort, ProfileTraceRecord,
    ProfileZoneSummary, analyze_profile_trace, import_profile_trace,
    list_profile_traces, profile_library_path, resolve_profile_trace,
};

pub use role::{
    ParseVaporRoleError, RoleError, RoleStatus, RoleTransitionReport, VaporRole, demote_role,
    git_available, installed_role, promote_role, role_status,
};

pub use source::{
    ResolvedSourceContext, SourceContextSource, SourceError, SourceState, active_source,
    forget_source, open_source, resolve_source_context, source_state,
};

pub use steam::{
    EcosystemDistributionManifest, SteamDeploymentError, SteamDeploymentOptions,
    SteamDeploymentReport, SteamDepotStage, deploy_ecosystem_to_steam,
};

pub use superworkspace::{
    SUPERWORKSPACE_MANIFEST_FILE_NAME, SuperworkspaceError, SuperworkspaceProject,
    SuperworkspaceRepository, SuperworkspaceRepositoryKind, VaporSuperworkspace,
};

pub use toolchain::{ManagedToolchain, ToolchainError};

pub use tracy::{
    ManagedTracyRepair, ManagedTracyToolset, TRACY_PROFILER_VERSION, TracyError,
};

pub use uninstall::{UninstallError, UninstallOptions, UninstallReport, uninstall_installation};

pub use workspace::{
    RunConfiguration, RunConfigurationError, RunConfigurationSpec, ToolchainPin, VaporProject,
    VaporWorkspace, WORKSPACE_MANIFEST_FILE_NAME, WorkspaceError, WorkspaceHeader,
    WorkspaceManifest, WorkspaceProjectSpec,
};
