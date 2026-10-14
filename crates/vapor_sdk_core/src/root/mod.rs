//! Root Vapor package and SteamPipe publishing workflows.
//!
//! Root publishing is intentionally about the Steam-distributed Vapor app root,
//! not about arbitrary authored content. The installed Rust toolchain is not
//! packaged here; Vapor should wrap/install Rust tooling deliberately instead
//! of shipping the active toolchain tree as app content.

pub mod error;
pub mod package;
pub mod publish;
pub mod types;
