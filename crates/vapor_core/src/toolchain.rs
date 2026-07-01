//! Canonical Vapor Rust/Cargo toolchain model.
//!
//! This module describes the human-selected toolchain line and the ecosystem
//! defaults Vapor infers from it. Download URLs, archive hashes, exact rustc/cargo
//! commit metadata, and install receipts belong in generated lock/state artifacts.

use crate::ToolchainIntent;

/// Host triples Vapor intends to bootstrap first.
pub const SUPPORTED_HOST_TRIPLES: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
];

/// Target standard libraries Vapor expects the toolchain to carry first.
pub const SUPPORTED_TARGET_TRIPLES: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
];

/// Required toolchain components for the first Vapor-managed Rust install.
pub const REQUIRED_TOOLCHAIN_COMPONENTS: &[ToolchainComponent] = &[
    ToolchainComponent::Rustc,
    ToolchainComponent::Cargo,
    ToolchainComponent::RustStd,
    ToolchainComponent::Rustfmt,
    ToolchainComponent::Clippy,
    ToolchainComponent::RustSrc,
];

/// Rust distribution component names Vapor treats as required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolchainComponent {
    Rustc,
    Cargo,
    RustStd,
    Rustfmt,
    Clippy,
    RustSrc,
}

impl ToolchainComponent {
    /// Stable Rust distribution component spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rustc => "rustc",
            Self::Cargo => "cargo",
            Self::RustStd => "rust-std",
            Self::Rustfmt => "rustfmt",
            Self::Clippy => "clippy",
            Self::RustSrc => "rust-src",
        }
    }
}

/// Canonical ecosystem toolchain inferred from the root `Vapor.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalToolchain {
    pub channel: String,
    pub date: String,
}

impl CanonicalToolchain {
    /// Build the canonical toolchain model from human-authored manifest intent.
    pub fn from_intent(intent: ToolchainIntent) -> Self {
        Self { channel: intent.channel, date: intent.date }
    }

    /// Stable toolchain identifier used for install paths and lock metadata.
    pub fn identifier(&self) -> String {
        format!("{}-{}", self.channel, self.date)
    }

    /// Whether this host is currently part of the Vapor bootstrap surface.
    pub fn supports_host(&self, host_triple: &str) -> bool {
        let _ = self;
        SUPPORTED_HOST_TRIPLES.contains(&host_triple)
    }

    /// Host triples this toolchain line is expected to support.
    pub fn supported_host_triples(&self) -> &'static [&'static str] {
        let _ = self;
        SUPPORTED_HOST_TRIPLES
    }

    /// Target standard libraries this toolchain line is expected to install.
    pub fn supported_target_triples(&self) -> &'static [&'static str] {
        let _ = self;
        SUPPORTED_TARGET_TRIPLES
    }

    /// Components required before Vapor considers the toolchain usable.
    pub fn required_components(&self) -> &'static [ToolchainComponent] {
        let _ = self;
        REQUIRED_TOOLCHAIN_COMPONENTS
    }
}

/// Compile-time host triple for the currently running SDK binary.
pub fn current_host_triple() -> &'static str {
    CURRENT_HOST_TRIPLE
}

#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
const CURRENT_HOST_TRIPLE: &str = "x86_64-unknown-linux-gnu";

#[cfg(all(target_arch = "x86_64", target_os = "windows", target_env = "msvc"))]
const CURRENT_HOST_TRIPLE: &str = "x86_64-pc-windows-msvc";

#[cfg(not(any(
    all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"),
    all(target_arch = "x86_64", target_os = "windows", target_env = "msvc")
)))]
const CURRENT_HOST_TRIPLE: &str = "unknown";
