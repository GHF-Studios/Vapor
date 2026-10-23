//! Local filesystem, installation, and Workshop providers for Vapor Client.
//!
//! This crate owns the host-facing content operations shared by Studio and
//! command-line frontends. Frontends provide presentation and invocation; this
//! crate owns the common provider implementation.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

/// App-local cross-compilation tool inspection and Cargo environment setup.
pub mod cross_toolchain;
/// Commands using the Rust toolchain bundled with the selected Vapor app.
pub mod managed_cargo;
mod process_output;
mod cargo_output;
mod source;
mod target;
pub mod workspace_build;

/// Installation of source-owned app-root assets.
pub mod app_root;
/// Content discovery, packaging, installation, and Workshop operations.
pub mod content;
/// Installation and source workspace discovery.
pub mod discovery;
/// App distribution staging and SteamPipe publishing.
pub mod distribution;
/// Publishing the curated product documentation site.
pub mod docs_publish;
/// Building and installing Rustdoc.
pub mod documentation;
/// Role-specific Vapor manifest parsing.
pub mod manifest;
/// Managed Rust validation, release builds, and app-root promotion.
pub mod release;
/// SteamCMD-backed Steam operations.
pub mod steam;
/// Source workspace structure and policy.
pub mod workspace;
/// Canonical runtime target identity and validation.
pub use target::{host_runtime_target, validate_runtime_target};
