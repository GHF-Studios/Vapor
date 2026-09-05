//! Vapor-managed Rust toolchain.
//!
//! The current bootstrap discovers the canonical toolchain from the enclosing
//! Vapor workspace. Installation and execution use Vapor-owned Rustup/Cargo
//! state rather than the user's ambient Rust toolchain.

use serde::Deserialize;
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

pub const WORKSPACE_MANIFEST_FILE_NAME: &str = "Workspace.vapor.toml";

pub const VAPOR_HOME_ENV: &str = "VAPOR_HOME";

const RUSTUP_HOME_DIR: &str = "rustup-home";
const CARGO_HOME_DIR: &str = "cargo-home";
const LOCAL_RUSTUP_DIR: &str = "rustup/bin";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ToolchainPin {
    pub channel: String,
    pub version: String,
    pub date: String,
}

impl ToolchainPin {
    pub fn identifier(&self) -> &str {
        &self.version
    }
}

#[derive(Debug, Clone)]
pub struct ManagedToolchain {
    pub workspace_root: PathBuf,
    pub vapor_home: PathBuf,
    pub rustup_home: PathBuf,
    pub cargo_home: PathBuf,
    pub pin: ToolchainPin,
    pub cargo_path: PathBuf,
    pub rustc_path: PathBuf,
    pub rust_analyzer_path: PathBuf,
}

impl ManagedToolchain {
    /// Discover the canonical Vapor workspace and its pinned Rust toolchain.
    ///
    /// This is intentionally a bootstrap rule. A shipped Vapor installation
    /// will eventually obtain this state from its installation profile rather
    /// than from the current working directory.
    pub fn discover() -> Result<Self, ToolchainError> {
        let start = env::current_dir().map_err(ToolchainError::CurrentDirectory)?;

        let workspace_root =
            find_workspace_root(&start).ok_or_else(|| ToolchainError::WorkspaceNotFound {
                start: start.clone(),
            })?;

        let manifest_path = workspace_root.join(WORKSPACE_MANIFEST_FILE_NAME);

        let source = fs::read_to_string(&manifest_path).map_err(|source| ToolchainError::Io {
            path: manifest_path.clone(),
            source,
        })?;

        let manifest: WorkspaceManifest =
            toml::from_str(&source).map_err(|error| ToolchainError::Manifest {
                path: manifest_path,
                message: error.to_string(),
            })?;

        let vapor_home = env::var_os(VAPOR_HOME_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace_root.join(".vapor"));

        let rustup_home = vapor_home.join(RUSTUP_HOME_DIR);
        let cargo_home = vapor_home.join(CARGO_HOME_DIR);

        let host = current_host_triple()?;

        let toolchain_root = rustup_home
            .join("toolchains")
            .join(format!("{}-{host}", manifest.toolchain.identifier()));

        let bin = toolchain_root.join("bin");

        Ok(Self {
            workspace_root,
            vapor_home,
            rustup_home,
            cargo_home,
            pin: manifest.toolchain,
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
    /// Rustup itself may currently come from PATH. The resulting Rust
    /// installation does not.
    pub fn install(&self) -> Result<(), ToolchainError> {
        let rustup = self
            .find_rustup()
            .ok_or_else(|| ToolchainError::RustupUnavailable {
                expected: self.local_rustup_path(),
            })?;

        fs::create_dir_all(&self.rustup_home).map_err(|source| ToolchainError::Io {
            path: self.rustup_home.clone(),
            source,
        })?;

        fs::create_dir_all(&self.cargo_home).map_err(|source| ToolchainError::Io {
            path: self.cargo_home.clone(),
            source,
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

        Ok(())
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

        if local.is_file() {
            return Some(local);
        }

        let path = env::var_os("PATH")?;

        let executable = executable_name("rustup");

        env::split_paths(&path)
            .map(|root| root.join(&executable))
            .find(|candidate| candidate.is_file())
    }
}

#[derive(Deserialize)]
struct WorkspaceManifest {
    toolchain: ToolchainPin,
}

fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|root| root.join(WORKSPACE_MANIFEST_FILE_NAME).is_file())
        .map(Path::to_path_buf)
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
    CurrentDirectory(io::Error),

    WorkspaceNotFound { start: PathBuf },

    Manifest { path: PathBuf, message: String },

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
            Self::CurrentDirectory(error) => {
                write!(formatter, "failed to determine current directory: {error}")
            }

            Self::WorkspaceNotFound { start } => {
                write!(
                    formatter,
                    "could not find `{WORKSPACE_MANIFEST_FILE_NAME}` from `{}`",
                    start.display()
                )
            }

            Self::Manifest { path, message } => {
                write!(
                    formatter,
                    "invalid Vapor workspace manifest `{}`: {message}",
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
                    "Rustup is unavailable; Vapor looked for `{}` and on PATH",
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
                    "Vapor-managed Rust {version} is not installed; expected Cargo at `{}`; run `vapor toolchain install`",
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
            Self::CurrentDirectory(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::JoinPaths(error) => Some(error),
            _ => None,
        }
    }
}
