//! SDK-managed Rustup/Cargo command intent and local status reporting.

mod install;
mod plan;

use std::env;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use vapor_core::{HostTarget, ManagedToolchain};

pub use install::{ToolchainInstallError, ToolchainInstallReport, toolchain_install};
pub use plan::{ToolchainInstallPlan, ToolchainPlanError, toolchain_install_plan};

/// Environment variable that overrides where Vapor stores executable-local state.
pub const VAPOR_HOME_ENV: &str = "VAPOR_HOME";

/// Directory under `VAPOR_HOME` that contains the Rustup binary Vapor prefers.
pub const RUSTUP_DIR: &str = "rustup";

/// Directory under `VAPOR_HOME/rustup` that contains the Rustup executable.
pub const RUSTUP_BIN_DIR: &str = "bin";

/// Directory under `VAPOR_HOME` used as Rustup's managed home.
pub const RUSTUP_HOME_DIR: &str = "rustup-home";

/// Directory under `VAPOR_HOME` used as Cargo's managed home.
pub const CARGO_HOME_DIR: &str = "cargo-home";

/// Directory under `VAPOR_HOME` reserved for Rustup acquisition/bootstrap state.
pub const TOOLCHAIN_BOOTSTRAP_DIR: &str = "toolchain-bootstrap";

/// Directory under `VAPOR_HOME` for SDK-managed build outputs.
pub const OUTPUT_DIR: &str = "output";

/// Toolchain commands for the pinned SDK-managed Rust/Cargo toolchain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolchainCommand {
    /// Inspect pinned toolchain state.
    Status,
    /// Install the pinned toolchain chosen by the project owner.
    Install,
    /// Repair a damaged or incomplete pinned toolchain installation.
    Repair,
}

/// Current local state for the canonical Vapor Rust/Cargo toolchain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainStatus {
    pub toolchain: CanonicalToolchain,
    pub host_triple: &'static str,
    pub host_supported: bool,
    pub vapor_home_source: VaporHomeSource,
    pub vapor_home: PathBuf,
    /// App-local Rustup executable path Vapor prefers over PATH lookup.
    pub local_rustup_path: PathBuf,
    /// Rustup executable Vapor will invoke, if one is available.
    pub rustup_path: Option<PathBuf>,
    pub rustup_source: RustupSource,
    /// Rustup home scoped to this Vapor install.
    pub rustup_home: PathBuf,
    /// Cargo home scoped to this Vapor install.
    pub cargo_home: PathBuf,
    /// Expected Rustup-managed toolchain root for the canonical pin and host.
    pub toolchain_root: PathBuf,
    /// Reserved state for acquiring/updating the app-local Rustup binary.
    pub bootstrap_root: PathBuf,
    /// Stable output root for future build/package promotion.
    pub output_root: PathBuf,
    pub cargo_path: PathBuf,
    pub rustc_path: PathBuf,
    pub install_state: ToolchainInstallState,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalToolchain {
    pub channel: String,
    pub version: Option<String>,
    pub date: String,
}

impl CanonicalToolchain {
    pub fn identifier(&self) -> &str {
        self.version.as_deref().unwrap_or(&self.channel)
    }
    pub fn required_components(&self) -> &'static [ToolchainComponent] {
        &[
            ToolchainComponent::Rustc,
            ToolchainComponent::Cargo,
            ToolchainComponent::RustStd,
            ToolchainComponent::Rustfmt,
            ToolchainComponent::Clippy,
            ToolchainComponent::RustSrc,
        ]
    }
    pub fn supported_host_triples(&self) -> &'static [&'static str] {
        &["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"]
    }
    pub fn supported_target_triples(&self) -> &'static [&'static str] {
        self.supported_host_triples()
    }
    pub fn supports_host(&self, host: &str) -> bool {
        self.supported_host_triples().contains(&host)
    }
}

impl ToolchainStatus {
    /// Supported host triples inferred from the canonical toolchain model.
    pub fn supported_host_triples(&self) -> &'static [&'static str] {
        self.toolchain.supported_host_triples()
    }

    /// Target triples inferred from the canonical toolchain model.
    pub fn supported_target_triples(&self) -> &'static [&'static str] {
        self.toolchain.supported_target_triples()
    }
}

/// Where `VAPOR_HOME` came from for this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaporHomeSource {
    Environment,
    ExecutableRoot,
}

impl VaporHomeSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Environment => VAPOR_HOME_ENV,
            Self::ExecutableRoot => "executable root",
        }
    }
}

/// Where the Rustup executable came from for this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustupSource {
    VaporLocal,
    PathLookup,
    Unavailable,
}

impl RustupSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VaporLocal => "vapor-local",
            Self::PathLookup => "path",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Coarse local install state before real archive verification exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolchainInstallState {
    MissingRustup,
    Missing,
    PresentUnverified,
    Broken { reason: String },
}

impl ToolchainInstallState {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::MissingRustup => "missing_rustup",
            Self::Missing => "missing",
            Self::PresentUnverified => "present_unverified",
            Self::Broken { .. } => "broken",
        }
    }
}

/// Inspect the canonical Vapor toolchain without mutating local state.
pub fn toolchain_status() -> Result<ToolchainStatus, ToolchainStatusError> {
    let managed = ManagedToolchain::discover()
        .map_err(|error| ToolchainStatusError::Core(error.to_string()))?;
    let host =
        HostTarget::current().map_err(|error| ToolchainStatusError::Core(error.to_string()))?;
    let host_triple = host.triple();
    let toolchain = CanonicalToolchain {
        channel: managed.pin.channel.clone(),
        version: Some(managed.pin.version.clone()),
        date: managed.pin.date.clone(),
    };
    let vapor_home = managed.vapor_home.clone();
    let vapor_home_source = if env::var_os(VAPOR_HOME_ENV).is_some() {
        VaporHomeSource::Environment
    } else {
        VaporHomeSource::ExecutableRoot
    };
    let local_rustup_path = vapor_home
        .join(RUSTUP_DIR)
        .join(RUSTUP_BIN_DIR)
        .join(executable_name("rustup"));
    let (rustup_path, rustup_source) = resolve_rustup(&local_rustup_path);
    let rustup_home = managed.rustup_home.clone();
    let cargo_home = managed.cargo_home.clone();
    let toolchain_root = managed
        .rustc_path
        .parent()
        .and_then(Path::parent)
        .unwrap_or(&managed.rustup_home)
        .to_path_buf();
    let bootstrap_root = managed.user_data_root.join(TOOLCHAIN_BOOTSTRAP_DIR);
    let output_root = managed.user_data_root.join(OUTPUT_DIR);
    let cargo_path = managed.cargo_path.clone();
    let rustc_path = managed.rustc_path.clone();
    let install_state =
        inspect_install_state(&rustup_path, &toolchain_root, &cargo_path, &rustc_path);

    Ok(ToolchainStatus {
        host_supported: toolchain.supports_host(host_triple),
        toolchain,
        host_triple,
        vapor_home_source,
        vapor_home,
        local_rustup_path,
        rustup_path,
        rustup_source,
        rustup_home,
        cargo_home,
        toolchain_root,
        bootstrap_root,
        output_root,
        cargo_path,
        rustc_path,
        install_state,
    })
}

fn executable_name(stem: &str) -> String {
    format!("{stem}{}", env::consts::EXE_SUFFIX)
}

fn resolve_rustup(local_rustup_path: &Path) -> (Option<PathBuf>, RustupSource) {
    if local_rustup_path.is_file() {
        return (
            Some(local_rustup_path.to_path_buf()),
            RustupSource::VaporLocal,
        );
    }

    let Some(path) = env::var_os("PATH") else {
        return (None, RustupSource::Unavailable);
    };

    let rustup_name = executable_name("rustup");
    for root in env::split_paths(&path) {
        let candidate = root.join(&rustup_name);
        if candidate.is_file() {
            return (Some(candidate), RustupSource::PathLookup);
        }
    }

    (None, RustupSource::Unavailable)
}

fn inspect_install_state(
    rustup_path: &Option<PathBuf>,
    toolchain_root: &Path,
    cargo_path: &Path,
    rustc_path: &Path,
) -> ToolchainInstallState {
    if rustup_path.is_none() {
        return ToolchainInstallState::MissingRustup;
    }

    if !toolchain_root.exists() {
        return ToolchainInstallState::Missing;
    }

    let mut missing = Vec::new();
    if !cargo_path.exists() {
        missing.push("cargo");
    }
    if !rustc_path.exists() {
        missing.push("rustc");
    }

    if missing.is_empty() {
        ToolchainInstallState::PresentUnverified
    } else {
        ToolchainInstallState::Broken {
            reason: format!("missing {}", missing.join(" and ")),
        }
    }
}

/// Error returned while inspecting local Vapor toolchain state.
#[derive(Debug)]
pub enum ToolchainStatusError {
    Core(String),
}

impl fmt::Display for ToolchainStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(error) => write!(formatter, "{error}"),
        }
    }
}

impl Error for ToolchainStatusError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(_) => None,
        }
    }
}
