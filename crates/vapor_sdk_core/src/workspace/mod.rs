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
    /// Update workspace dependencies in Cargo.lock using Vapor's local registry cache.
    ResolveLockfile,
    /// Run `cargo check --locked` through the Vapor-managed Cargo binary.
    Check,
    /// Run `cargo fmt` through the Vapor-managed Cargo binary.
    Fmt,
    /// Run `cargo build --workspace --locked` through the Vapor-managed Cargo binary.
    Build,
    /// Run `cargo build --workspace --release --locked` through the Vapor-managed Cargo binary.
    BuildRelease,
    /// Run `cargo test --workspace --locked` through the Vapor-managed Cargo binary.
    Test,
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

/// Update the current workspace lockfile from Vapor-managed Cargo's local index/cache.
pub fn workspace_resolve_lockfile(
    globals: &GlobalOptions,
) -> Result<WorkspaceCargoReport, WorkspaceCommandError> {
    cargo::VaporCargo::new(globals)?.run(&["update", "--workspace", "--offline"])
}

/// Run `cargo check --workspace --locked` through Vapor-owned Rustup/Cargo state.
pub fn workspace_check(
    globals: &GlobalOptions,
) -> Result<WorkspaceCargoReport, WorkspaceCommandError> {
    cargo::VaporCargo::new(globals)?.run(&["check", "--workspace", "--locked"])
}

/// Run `cargo fmt` through Vapor-owned Rustup/Cargo state.
pub fn workspace_fmt(
    globals: &GlobalOptions,
) -> Result<WorkspaceCargoReport, WorkspaceCommandError> {
    cargo::VaporCargo::new(globals)?.run(&["fmt"])
}

/// Run `cargo build --workspace --locked` through Vapor-owned Rustup/Cargo state.
pub fn workspace_build(
    globals: &GlobalOptions,
) -> Result<WorkspaceCargoReport, WorkspaceCommandError> {
    cargo::VaporCargo::new(globals)?.run(&["build", "--workspace", "--locked"])
}

/// Run `cargo build --workspace --release --locked` through Vapor-owned Rustup/Cargo state.
pub fn workspace_build_release(
    globals: &GlobalOptions,
) -> Result<WorkspaceCargoReport, WorkspaceCommandError> {
    cargo::VaporCargo::new(globals)?.run(&["build", "--workspace", "--release", "--locked"])
}

/// Run `cargo test --workspace --locked` through Vapor-owned Rustup/Cargo state.
pub fn workspace_test(
    globals: &GlobalOptions,
) -> Result<WorkspaceCargoReport, WorkspaceCommandError> {
    cargo::VaporCargo::new(globals)?.run(&["test", "--workspace", "--locked"])
}

/// Build and promote the unified `vapor` terminal facade into `$VAPOR_HOME/bin`.
pub fn workspace_deploy(
    globals: &GlobalOptions,
) -> Result<WorkspaceDeployReport, WorkspaceCommandError> {
    deploy::workspace_deploy(globals)
}
