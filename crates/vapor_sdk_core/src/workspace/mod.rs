//! SDK workspace workflows backed by the Vapor-managed Cargo binary.

mod cargo;
mod deploy;
mod error;
mod identity;
mod manage;
mod report;

use crate::GlobalOptions;
pub use error::WorkspaceCommandError;
pub use report::{
    WorkspaceCargoReport, WorkspaceDeployReport, WorkspaceStatusReport, WorkspaceSyncReport,
};

/// Commands that operate on the current authoring workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceCommand {
    /// Inspect the current Vapor workspace identity and managed structure.
    Status,
    /// Create or update SDK-managed workspace structure.
    Sync,
    /// Run `cargo check` through the Vapor-managed Cargo binary.
    Check,
    /// Run `cargo fmt` through the Vapor-managed Cargo binary.
    Fmt,
    /// Run `cargo build --workspace` through the Vapor-managed Cargo binary.
    Build,
    /// Build and promote the first-party SDK CLI into the executable-root `bin` directory.
    Deploy,
}

/// Inspect the current Vapor workspace identity and managed structure.
pub fn workspace_status(
    globals: &GlobalOptions,
) -> Result<WorkspaceStatusReport, WorkspaceCommandError> {
    manage::workspace_status(globals)
}

/// Create or update SDK-managed workspace structure.
pub fn workspace_sync(
    globals: &GlobalOptions,
) -> Result<WorkspaceSyncReport, WorkspaceCommandError> {
    manage::workspace_sync(globals)
}

/// Run `cargo check --workspace` through Vapor-owned Rustup/Cargo state.
pub fn workspace_check(
    globals: &GlobalOptions,
) -> Result<WorkspaceCargoReport, WorkspaceCommandError> {
    cargo::VaporCargo::new(globals)?.run(&["check", "--workspace"])
}

/// Run `cargo fmt` through Vapor-owned Rustup/Cargo state.
pub fn workspace_fmt(
    globals: &GlobalOptions,
) -> Result<WorkspaceCargoReport, WorkspaceCommandError> {
    cargo::VaporCargo::new(globals)?.run(&["fmt"])
}

/// Run `cargo build --workspace` through Vapor-owned Rustup/Cargo state.
pub fn workspace_build(
    globals: &GlobalOptions,
) -> Result<WorkspaceCargoReport, WorkspaceCommandError> {
    cargo::VaporCargo::new(globals)?.run(&["build", "--workspace"])
}

/// Build and promote the unified `vapor` terminal facade into `$VAPOR_HOME/bin`.
pub fn workspace_deploy(
    globals: &GlobalOptions,
) -> Result<WorkspaceDeployReport, WorkspaceCommandError> {
    deploy::workspace_deploy(globals)
}
