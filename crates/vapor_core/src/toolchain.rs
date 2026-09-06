//! Vapor-managed Rust toolchain.
//!
//! Installation and execution use Vapor-owned Rustup/Cargo state rather than
//! the user's ambient Rust toolchain.
//!
//! The toolchain belongs operationally to a Vapor Installation.
//!
//! A deployed/installed Vapor carries its toolchain pin in installation-owned
//! metadata so the installed command can recover its exact managed toolchain
//! without depending on authored source, a remembered source selection, or the
//! process working directory.
//!
//! During source bootstrap, before installation metadata exists, the enclosing
//! Vapor Workspace remains the source of the pin.

use crate::{
    InstallationRootSource, ToolchainPin, VaporInstallation, VaporWorkspace, WorkspaceError,
};
use serde::{Deserialize, Serialize};
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

const RUSTUP_HOME_DIR: &str = "rustup-home";
const CARGO_HOME_DIR: &str = "cargo-home";
const LOCAL_RUSTUP_DIR: &str = "rustup/bin";

const INSTALLATION_METADATA_DIR: &str = "metadata";
const TOOLCHAIN_METADATA_FILE: &str = "toolchain.toml";
const TOOLCHAIN_METADATA_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstallationToolchainMetadata {
    schema: u32,
    channel: String,
    version: String,
    date: String,
}

#[derive(Debug, Clone)]
pub struct ManagedToolchain {
    /// Source Workspace associated with this toolchain construction.
    ///
    /// For source-driven development this is the actual Vapor Workspace.
    ///
    /// When an installed Vapor reconstructs its toolchain solely from packaged
    /// Installation metadata, no source Workspace is required; the Installation
    /// root is used as the neutral operational anchor for this legacy field.
    pub workspace_root: PathBuf,

    pub vapor_home: PathBuf,
    pub installation_source: InstallationRootSource,

    pub rustup_home: PathBuf,
    pub cargo_home: PathBuf,

    pub pin: ToolchainPin,

    pub cargo_path: PathBuf,
    pub rustc_path: PathBuf,
    pub rust_analyzer_path: PathBuf,
}

impl ManagedToolchain {
    /// Discover the managed toolchain for the current Vapor environment.
    ///
    /// Resolution:
    ///
    /// 1. Discover the active Vapor Installation.
    /// 2. If that Installation carries packaged toolchain metadata, use it.
    /// 3. Otherwise fall back to the enclosing authored Vapor Workspace.
    ///
    /// Step 3 exists for source/bootstrap execution only. A properly deployed
    /// Vapor Installation should be independently self-describing.
    pub fn discover() -> Result<Self, ToolchainError> {
        let installation = VaporInstallation::discover().map_err(ToolchainError::Installation)?;

        if let Some(pin) = read_installation_toolchain_metadata(&installation)? {
            return Self::for_installation(&installation, installation.root.clone(), pin);
        }

        let workspace = VaporWorkspace::discover().map_err(ToolchainError::Workspace)?;

        Self::for_workspace(&workspace)
    }

    /// Construct the managed toolchain belonging to a known authored Workspace.
    ///
    /// The Workspace supplies the toolchain pin while the active Installation
    /// supplies the actual Rustup/Cargo storage boundary.
    pub fn for_workspace(workspace: &VaporWorkspace) -> Result<Self, ToolchainError> {
        let installation = VaporInstallation::for_workspace(workspace);

        Self::for_installation(
            &installation,
            workspace.root.clone(),
            workspace.manifest.toolchain.clone(),
        )
    }

    pub fn for_installation(
        installation: &VaporInstallation,
        workspace_root: PathBuf,
        pin: ToolchainPin,
    ) -> Result<Self, ToolchainError> {
        let vapor_home = installation.root.clone();

        let rustup_home = vapor_home.join(RUSTUP_HOME_DIR);

        let cargo_home = vapor_home.join(CARGO_HOME_DIR);

        let host = current_host_triple()?;

        let toolchain_root = rustup_home
            .join("toolchains")
            .join(format!("{}-{host}", pin.identifier()));

        let bin = toolchain_root.join("bin");

        Ok(Self {
            workspace_root,
            vapor_home,
            installation_source: installation.root_source,
            rustup_home,
            cargo_home,
            pin,
            cargo_path: bin.join(executable_name("cargo")),
            rustc_path: bin.join(executable_name("rustc")),
            rust_analyzer_path: bin.join(executable_name("rust-analyzer")),
        })
    }

    pub fn is_installed(&self) -> bool {
        self.cargo_path.is_file() && self.rustc_path.is_file()
    }

    /// Install or repair the pinned toolchain using Rustup as bootstrap.
    ///
    /// Vapor only accepts a Rustup executable that demonstrably honors the
    /// Installation's managed `RUSTUP_HOME`.
    ///
    /// Successful installation also refreshes the Installation's packaged
    /// toolchain metadata so installed Vapor can subsequently recover this pin
    /// without source discovery.
    pub fn install(&self) -> Result<(), ToolchainError> {
        fs::create_dir_all(&self.rustup_home).map_err(|source| ToolchainError::Io {
            path: self.rustup_home.clone(),
            source,
        })?;

        fs::create_dir_all(&self.cargo_home).map_err(|source| ToolchainError::Io {
            path: self.cargo_home.clone(),
            source,
        })?;

        let rustup = self
            .find_rustup()
            .ok_or_else(|| ToolchainError::RustupUnavailable {
                expected: self.local_rustup_path(),
            })?;

        let status = Command::new(&rustup)
            .args([
                "toolchain",
                "install",
                self.pin.identifier(),
                "--profile",
                "minimal",
                "--no-self-update",
                "--component",
                "rustfmt",
                "--component",
                "clippy",
                "--component",
                "rust-src",
                "--component",
                "rust-analyzer",
            ])
            .env("RUSTUP_HOME", &self.rustup_home)
            .env("CARGO_HOME", &self.cargo_home)
            .status()
            .map_err(|source| ToolchainError::Io {
                path: rustup.clone(),
                source,
            })?;

        if !status.success() {
            return Err(ToolchainError::RustupFailed {
                path: rustup,
                status,
            });
        }

        if !self.is_installed() {
            return Err(ToolchainError::ToolchainIncomplete {
                cargo: self.cargo_path.clone(),
                rustc: self.rustc_path.clone(),
            });
        }

        self.persist_installation_metadata()?;

        Ok(())
    }

    /// Persist the exact managed toolchain pin as Installation metadata.
    ///
    /// This file belongs to the replaceable installation payload, not mutable
    /// local state. Steam/root deployment should package this metadata alongside
    /// the Vapor binaries.
    pub fn persist_installation_metadata(&self) -> Result<PathBuf, ToolchainError> {
        let metadata_root = self.vapor_home.join(INSTALLATION_METADATA_DIR);

        fs::create_dir_all(&metadata_root).map_err(|source| ToolchainError::Io {
            path: metadata_root.clone(),
            source,
        })?;

        let path = metadata_root.join(TOOLCHAIN_METADATA_FILE);

        let metadata = InstallationToolchainMetadata {
            schema: TOOLCHAIN_METADATA_SCHEMA,
            channel: self.pin.channel.clone(),
            version: self.pin.version.clone(),
            date: self.pin.date.clone(),
        };

        let source = toml::to_string_pretty(&metadata).map_err(|error| {
            ToolchainError::InstallationMetadata {
                path: path.clone(),
                message: error.to_string(),
            }
        })?;

        fs::write(&path, source).map_err(|source| ToolchainError::Io {
            path: path.clone(),
            source,
        })?;

        Ok(path)
    }

    /// Construct Cargo with the Vapor-managed toolchain environment.
    pub fn cargo_command(&self) -> Result<Command, ToolchainError> {
        if !self.is_installed() {
            return Err(ToolchainError::ToolchainNotInstalled {
                version: self.pin.version.clone(),
                expected: self.cargo_path.clone(),
            });
        }

        let bin = self
            .cargo_path
            .parent()
            .ok_or_else(|| ToolchainError::BinaryHasNoParent {
                path: self.cargo_path.clone(),
            })?;

        let mut paths = vec![bin.to_path_buf()];

        if let Some(existing) = env::var_os("PATH") {
            paths.extend(env::split_paths(&existing));
        }

        let path = env::join_paths(paths).map_err(ToolchainError::JoinPaths)?;

        let mut command = Command::new(&self.cargo_path);

        command
            .env("CARGO_HOME", &self.cargo_home)
            .env("RUSTUP_HOME", &self.rustup_home)
            .env("RUSTUP_TOOLCHAIN", self.pin.identifier())
            .env("RUSTC", &self.rustc_path)
            .env("PATH", path)
            .env_remove("RUSTC_WRAPPER")
            .env_remove("RUSTC_WORKSPACE_WRAPPER");

        Ok(command)
    }

    fn local_rustup_path(&self) -> PathBuf {
        self.vapor_home
            .join(LOCAL_RUSTUP_DIR)
            .join(executable_name("rustup"))
    }

    fn find_rustup(&self) -> Option<PathBuf> {
        let local = self.local_rustup_path();

        let path_candidates = env::var_os("PATH")
            .into_iter()
            .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
            .map(|root| root.join(executable_name("rustup")));

        std::iter::once(local)
            .chain(path_candidates)
            .filter(|candidate| candidate.is_file())
            .find(|candidate| self.rustup_honors_managed_home(candidate))
    }

    fn rustup_honors_managed_home(&self, rustup: &Path) -> bool {
        let Ok(output) = Command::new(rustup)
            .args(["show", "home"])
            .env("RUSTUP_HOME", &self.rustup_home)
            .env("CARGO_HOME", &self.cargo_home)
            .output()
        else {
            return false;
        };

        if !output.status.success() {
            return false;
        }

        let reported_home = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());

        reported_home == self.rustup_home
    }
}

fn read_installation_toolchain_metadata(
    installation: &VaporInstallation,
) -> Result<Option<ToolchainPin>, ToolchainError> {
    let path = installation
        .root
        .join(INSTALLATION_METADATA_DIR)
        .join(TOOLCHAIN_METADATA_FILE);

    if !path.is_file() {
        return Ok(None);
    }

    let source = fs::read_to_string(&path).map_err(|source| ToolchainError::Io {
        path: path.clone(),
        source,
    })?;

    let metadata: InstallationToolchainMetadata =
        toml::from_str(&source).map_err(|error| ToolchainError::InstallationMetadata {
            path: path.clone(),
            message: error.to_string(),
        })?;

    if metadata.schema != TOOLCHAIN_METADATA_SCHEMA {
        return Err(ToolchainError::InstallationMetadata {
            path,
            message: format!(
                "unsupported toolchain metadata schema {}; this Vapor supports {}",
                metadata.schema, TOOLCHAIN_METADATA_SCHEMA,
            ),
        });
    }

    Ok(Some(ToolchainPin {
        channel: metadata.channel,
        version: metadata.version,
        date: metadata.date,
    }))
}

fn executable_name(stem: &str) -> String {
    format!("{stem}{}", env::consts::EXE_SUFFIX)
}

fn current_host_triple() -> Result<&'static str, ToolchainError> {
    if cfg!(all(
        target_arch = "x86_64",
        target_os = "linux",
        target_env = "gnu"
    )) {
        Ok("x86_64-unknown-linux-gnu")
    } else if cfg!(all(
        target_arch = "x86_64",
        target_os = "windows",
        target_env = "msvc"
    )) {
        Ok("x86_64-pc-windows-msvc")
    } else {
        Err(ToolchainError::UnsupportedHost)
    }
}

#[derive(Debug)]
pub enum ToolchainError {
    Installation(crate::InstallationError),

    Workspace(WorkspaceError),

    InstallationMetadata { path: PathBuf, message: String },

    Io { path: PathBuf, source: io::Error },

    UnsupportedHost,

    RustupUnavailable { expected: PathBuf },

    RustupFailed { path: PathBuf, status: ExitStatus },

    ToolchainNotInstalled { version: String, expected: PathBuf },

    ToolchainIncomplete { cargo: PathBuf, rustc: PathBuf },

    BinaryHasNoParent { path: PathBuf },

    JoinPaths(env::JoinPathsError),
}

impl fmt::Display for ToolchainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installation(error) => error.fmt(formatter),

            Self::Workspace(error) => error.fmt(formatter),

            Self::InstallationMetadata { path, message } => {
                write!(
                    formatter,
                    "invalid Vapor Installation toolchain metadata `{}`: {message}",
                    path.display()
                )
            }

            Self::Io { path, source } => {
                write!(formatter, "failed to access `{}`: {source}", path.display())
            }

            Self::UnsupportedHost => {
                write!(
                    formatter,
                    "this host is not yet supported by the Vapor-managed toolchain"
                )
            }

            Self::RustupUnavailable { expected } => {
                write!(
                    formatter,
                    "no usable Rustup installation was found; Vapor looked for `{}` \
                     and on PATH, but requires Rustup to honor its managed RUSTUP_HOME",
                    expected.display()
                )
            }

            Self::RustupFailed { path, status } => {
                write!(
                    formatter,
                    "Rustup `{}` failed with {status}",
                    path.display()
                )
            }

            Self::ToolchainNotInstalled { version, expected } => {
                write!(
                    formatter,
                    "Vapor-managed Rust {version} is not installed; expected Cargo \
                     at `{}`; use the Vapor Installer to establish Content Developer capability",
                    expected.display()
                )
            }

            Self::ToolchainIncomplete { cargo, rustc } => {
                write!(
                    formatter,
                    "toolchain installation completed but Cargo/Rustc are missing (`{}`, `{}`)",
                    cargo.display(),
                    rustc.display()
                )
            }

            Self::BinaryHasNoParent { path } => {
                write!(
                    formatter,
                    "toolchain binary `{}` has no parent directory",
                    path.display()
                )
            }

            Self::JoinPaths(error) => {
                write!(
                    formatter,
                    "failed to construct Vapor toolchain PATH: {error}"
                )
            }
        }
    }
}

impl std::error::Error for ToolchainError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Installation(error) => Some(error),

            Self::Workspace(error) => Some(error),

            Self::Io { source, .. } => Some(source),

            Self::JoinPaths(error) => Some(error),

            _ => None,
        }
    }
}
