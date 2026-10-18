//! Local filesystem, installation, and Workshop providers for Vapor Client.
//!
//! This crate owns the host-facing content operations shared by Studio and
//! command-line frontends. Frontends provide presentation and invocation; this
//! crate owns the common provider implementation.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod cross_toolchain;
mod source;

/// Content discovery, packaging, installation, and Workshop operations.
pub mod content;
/// Installation and source workspace discovery.
pub mod discovery;
/// App distribution staging and SteamPipe publishing.
pub mod distribution;
/// Role-specific Vapor manifest parsing.
pub mod manifest;
/// SteamCMD-backed Steam operations.
pub mod steam;
/// Source workspace structure and policy.
pub mod workspace;
