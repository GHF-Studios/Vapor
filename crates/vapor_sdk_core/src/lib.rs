//! Reusable SDK command vocabulary and command specifications.
//!
//! This crate defines the typed request model and implements the first concrete
//! SDK-owned workflows: portable toolchain management, workspace Cargo wrappers,
//! SDK promotion, and root SteamPipe packaging/publishing.

#![forbid(unsafe_code)]

pub mod commands;
pub mod content;
pub mod environment;
pub mod options;
pub mod repair;
pub mod root;
pub mod spec;
pub mod steam;
pub mod template;
pub mod toolchain;
pub mod workspace;

pub use commands::{
    ContentReadCommand, LeafCommand, PackCommand, PackCompositionCommand, PackagepackCommand,
    SdkCommand, SourceAuthoringCommand,
};
pub use content::{ContentSource, ContentType, allowed_pack_children};
pub use environment::{EnvironmentCommand, EnvironmentReport, environment_status};
pub use options::GlobalOptions;
pub use repair::{RepairCommand, RepairTarget};
pub use spec::{CommandSpec, StateSurface, describe_command};
pub use template::TemplateCommand;
pub use toolchain::{
    CARGO_HOME_DIR, CanonicalToolchain, OUTPUT_DIR, RUSTUP_BIN_DIR, RUSTUP_DIR, RUSTUP_HOME_DIR,
    RustupSource, TOOLCHAIN_BOOTSTRAP_DIR, ToolchainCommand, ToolchainComponent,
    ToolchainInstallError, ToolchainInstallPlan, ToolchainInstallReport, ToolchainInstallState,
    ToolchainPlanError, ToolchainStatus, ToolchainStatusError, VAPOR_HOME_ENV, VaporHomeSource,
    toolchain_install, toolchain_install_plan, toolchain_status,
};
pub use vapor_core::ChildContentRef;
pub use workspace::{
    WorkspaceCargoReport, WorkspaceCommand, WorkspaceCommandError, WorkspaceDeployReport,
    WorkspaceStatusReport, WorkspaceSyncReport, workspace_build, workspace_check, workspace_deploy,
    workspace_fmt, workspace_status, workspace_sync, workspace_test,
};
