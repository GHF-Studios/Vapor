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
    InstallationRootSource, OperationOutputStream, ToolchainPin,
    VaporInstallation, VaporWorkspace, WorkspaceError,
};
use serde::{Deserialize, Serialize};
use std::env;
use std::ffi::OsStr;
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
pub const WINDOWS_GNU_TARGET: &str = "x86_64-pc-windows-gnullvm";

/// Rust toolchain identity and roots applied to a Vapor-managed child process.
///
/// Discovery and cache ownership stay with the caller. This type keeps the
/// executable-selection environment consistent across Cargo operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustToolchainCommandEnvironment {
    vapor_home: PathBuf,
    cargo_home: PathBuf,
    rustup_home: PathBuf,
    toolchain: std::ffi::OsString,
    rustc: PathBuf,
    rustdoc: PathBuf,
}

impl RustToolchainCommandEnvironment {
    pub fn new(
        vapor_home: impl Into<PathBuf>,
        cargo_home: impl Into<PathBuf>,
        rustup_home: impl Into<PathBuf>,
        toolchain: impl Into<std::ffi::OsString>,
        rustc: impl Into<PathBuf>,
        rustdoc: impl Into<PathBuf>,
    ) -> Self {
        Self {
            vapor_home: vapor_home.into(),
            cargo_home: cargo_home.into(),
            rustup_home: rustup_home.into(),
            toolchain: toolchain.into(),
            rustc: rustc.into(),
            rustdoc: rustdoc.into(),
        }
    }

    /// Apply Vapor's chosen Rust tools and remove ambient compiler wrappers.
    pub fn apply_to(&self, command: &mut Command) {
        command
            .env(crate::VAPOR_HOME_ENV, &self.vapor_home)
            .env("CARGO_HOME", &self.cargo_home)
            .env("RUSTUP_HOME", &self.rustup_home)
            .env("RUSTUP_TOOLCHAIN", &self.toolchain)
            .env("RUSTC", &self.rustc)
            .env("RUSTDOC", &self.rustdoc)
            .env_remove("RUSTC_WRAPPER")
            .env_remove("RUSTC_WORKSPACE_WRAPPER");
    }
}

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
    pub user_data_root: PathBuf,
    pub installation_source: InstallationRootSource,

    pub rustup_home: PathBuf,
    pub cargo_home: PathBuf,

    pub pin: ToolchainPin,

    pub cargo_path: PathBuf,
    pub rustc_path: PathBuf,
    pub rust_analyzer_path: PathBuf,
}

impl ManagedToolchain {
    /// Read the toolchain pin owned by an installed Vapor App Instance.
    ///
    /// This is shared by operations that execute installation-bundled tools
    /// directly and therefore must honor the same metadata as the managed
    /// developer toolchain without inheriting its separate user-data roots.
    pub fn installation_pin(
        installation: &VaporInstallation,
    ) -> Result<Option<ToolchainPin>, ToolchainError> {
        read_installation_toolchain_metadata(installation)
    }

    /// Discover the managed toolchain for the current source or installed context.
    ///
    /// The active Installation always owns physical Rust/Cargo state.
    ///
    /// When packaged Installation metadata is available, it supplies the pin.
    /// During source development, an authored Vapor Workspace may still supply
    /// the pin, but never the Installation root.
    pub fn discover() -> Result<Self, ToolchainError> {
        let installation = VaporInstallation::discover().map_err(ToolchainError::Installation)?;

        if let Some(pin) = read_installation_toolchain_metadata(&installation)? {
            return Self::for_installation(&installation, installation.root.clone(), pin);
        }

        // This is the source-oriented entry point. Keep workspace discovery
        // here, where the caller's source context is intentional, rather than
        // allowing installation-scoped operations to inherit ambient CWD.
        let workspace = VaporWorkspace::discover().map_err(ToolchainError::Workspace)?;
        Self::for_installation(
            &installation,
            workspace.root.clone(),
            workspace.manifest.toolchain.clone(),
        )
    }

    /// Discover the toolchain using a caller-selected installation root.
    pub fn discover_for_installation(
        installation: &VaporInstallation,
    ) -> Result<Self, ToolchainError> {
        let metadata_path = installation
            .root
            .join(INSTALLATION_METADATA_DIR)
            .join(TOOLCHAIN_METADATA_FILE);
        let pin = read_installation_toolchain_metadata(installation)?.ok_or(
            ToolchainError::MissingInstallationMetadata {
                path: metadata_path,
            },
        )?;
        Self::for_installation(installation, installation.root.clone(), pin)
    }

    /// Construct the managed toolchain for a known authored Workspace.
    ///
    /// The Workspace supplies authored toolchain intent. The active Vapor
    /// Installation supplies all physical managed-tool storage.
    pub fn for_workspace(workspace: &VaporWorkspace) -> Result<Self, ToolchainError> {
        let installation = VaporInstallation::discover().map_err(ToolchainError::Installation)?;

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
        let user_data_root = installation.user_data_root();

        let rustup_home = user_data_root.join(RUSTUP_HOME_DIR);

        let cargo_home = user_data_root.join(CARGO_HOME_DIR);

        let host = current_host_triple()?;

        let toolchain_root = rustup_home
            .join("toolchains")
            .join(format!("{}-{host}", pin.identifier()));

        let bin = toolchain_root.join("bin");

        Ok(Self {
            workspace_root,
            vapor_home,
            user_data_root,
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

    /// Check whether Rustup has installed the Windows GNU ABI target into this
    /// installation's pinned toolchain.
    pub fn target_is_installed(&self, target: &str) -> bool {
        let Some(toolchain_root) = self.rustc_path.parent().and_then(Path::parent) else {
            return false;
        };
        toolchain_root
            .join("lib/rustlib")
            .join(target)
            .join("lib")
            .is_dir()
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
        self.install_with_output(|_, _, _| {})
    }

    /// Install or repair the pinned toolchain while forwarding Rustup output.
    ///
    /// Output is line-bounded by the shared managed-process runner and tagged
    /// with the executable name before it reaches the operation stream.
    pub fn install_with_output(
        &self,
        mut report_output: impl FnMut(OperationOutputStream, &str, &str),
    ) -> Result<(), ToolchainError> {
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

        let mut install = crate::managed_process::background_command(&rustup);
        install
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
            .env("CARGO_HOME", &self.cargo_home);
        let status = crate::managed_process::run(&mut install, |stream, text| {
            report_output(stream, "rustup toolchain install", text)
        })
        .map_err(|message| ToolchainError::Process {
            path: rustup.clone(),
            message,
        })?;

        if !status.success() {
            return Err(ToolchainError::RustupFailed {
                path: rustup,
                status,
            });
        }

        let mut target = crate::managed_process::background_command(&rustup);
        target
            .args([
                "target",
                "add",
                WINDOWS_GNU_TARGET,
                "--toolchain",
                self.pin.identifier(),
            ])
            .env("RUSTUP_HOME", &self.rustup_home)
            .env("CARGO_HOME", &self.cargo_home);
        let status = crate::managed_process::run(&mut target, |stream, text| {
            report_output(stream, "rustup target add", text)
        })
        .map_err(|message| ToolchainError::Process {
            path: rustup.clone(),
            message,
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

        if !self.target_is_installed(WINDOWS_GNU_TARGET) {
            return Err(ToolchainError::TargetIncomplete {
                target: WINDOWS_GNU_TARGET,
                toolchain: self.pin.version.clone(),
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

    /// Construct any child process with the Vapor-managed Rust toolchain environment.
    ///
    /// This is used when an operation is authored by another subsystem (for example,
    /// a Platform Server repository script) but every Cargo/Rust process spawned by
    /// that operation must still use Vapor's managed toolchain.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Result<Command, ToolchainError> {
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

        let mut command = crate::managed_process::background_command(program);
        RustToolchainCommandEnvironment::new(
            &self.vapor_home,
            &self.cargo_home,
            &self.rustup_home,
            self.pin.identifier(),
            &self.rustc_path,
            self.rustc_path.with_file_name(executable_name("rustdoc")),
        )
        .apply_to(&mut command);
        command.env("PATH", path);

        Ok(command)
    }

    /// Construct Cargo with the Vapor-managed toolchain environment.
    pub fn cargo_command(&self) -> Result<Command, ToolchainError> {
        self.command(&self.cargo_path)
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
        let Ok(output) = crate::managed_process::background_command(rustup)
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
    crate::HostTarget::current()
        .map_err(|_| ToolchainError::UnsupportedHost)?
        .rustup_host_triple()
        .ok_or(ToolchainError::UnsupportedHost)
}

#[derive(Debug)]
pub enum ToolchainError {
    Installation(crate::InstallationError),

    Workspace(WorkspaceError),

    InstallationMetadata {
        path: PathBuf,
        message: String,
    },

    MissingInstallationMetadata {
        path: PathBuf,
    },

    Io {
        path: PathBuf,
        source: io::Error,
    },

    UnsupportedHost,

    RustupUnavailable {
        expected: PathBuf,
    },

    RustupFailed {
        path: PathBuf,
        status: ExitStatus,
    },

    Process {
        path: PathBuf,
        message: String,
    },

    ToolchainNotInstalled {
        version: String,
        expected: PathBuf,
    },

    ToolchainIncomplete {
        cargo: PathBuf,
        rustc: PathBuf,
    },

    TargetIncomplete {
        target: &'static str,
        toolchain: String,
    },

    BinaryHasNoParent {
        path: PathBuf,
    },

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

            Self::MissingInstallationMetadata { path } => write!(
                formatter,
                "this Vapor installation is missing its packaged Rust toolchain pin (`{}`); verify or reinstall the Vapor app files",
                path.display()
            ),

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

            Self::Process { path, message } => {
                write!(formatter, "managed process `{}` failed: {message}", path.display())
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

            Self::TargetIncomplete { target, toolchain } => {
                write!(
                    formatter,
                    "Rust {toolchain} was installed but target {target} is missing"
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

#[cfg(test)]
mod tests {
    use super::{ManagedToolchain, RustToolchainCommandEnvironment, ToolchainError};
    use crate::VaporInstallation;
    use std::{
        collections::BTreeMap,
        ffi::{OsStr, OsString},
        fs,
        process::Command,
    };

    #[test]
    fn rust_toolchain_environment_sets_pinned_paths_and_removes_wrappers() {
        let environment = RustToolchainCommandEnvironment::new(
            "/vapor/app",
            "/vapor/user/cargo-home",
            "/vapor/user/rustup-home",
            "1.97.0-x86_64-unknown-linux-gnu",
            "/vapor/user/rustup-home/toolchains/1.97.0/bin/rustc",
            "/vapor/user/rustup-home/toolchains/1.97.0/bin/rustdoc",
        );
        let mut command = Command::new("cargo");
        command
            .env("RUSTC_WRAPPER", "/ambient/wrapper")
            .env("RUSTC_WORKSPACE_WRAPPER", "/ambient/workspace-wrapper");

        environment.apply_to(&mut command);

        let variables = command
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(OsStr::to_owned)))
            .collect::<BTreeMap<OsString, Option<OsString>>>();
        assert_eq!(
            variables.get(OsStr::new("VAPOR_HOME")),
            Some(&Some("/vapor/app".into()))
        );
        assert_eq!(
            variables.get(OsStr::new("CARGO_HOME")),
            Some(&Some("/vapor/user/cargo-home".into()))
        );
        assert_eq!(
            variables.get(OsStr::new("RUSTUP_HOME")),
            Some(&Some("/vapor/user/rustup-home".into()))
        );
        assert_eq!(
            variables.get(OsStr::new("RUSTUP_TOOLCHAIN")),
            Some(&Some("1.97.0-x86_64-unknown-linux-gnu".into()))
        );
        assert_eq!(
            variables.get(OsStr::new("RUSTC")),
            Some(&Some(
                "/vapor/user/rustup-home/toolchains/1.97.0/bin/rustc".into()
            ))
        );
        assert_eq!(
            variables.get(OsStr::new("RUSTDOC")),
            Some(&Some(
                "/vapor/user/rustup-home/toolchains/1.97.0/bin/rustdoc".into()
            ))
        );
        assert_eq!(variables.get(OsStr::new("RUSTC_WRAPPER")), Some(&None));
        assert_eq!(
            variables.get(OsStr::new("RUSTC_WORKSPACE_WRAPPER")),
            Some(&None)
        );
    }

    #[test]
    fn selected_installation_supplies_the_toolchain_root() {
        let root = std::env::temp_dir().join(format!(
            "vapor-explicit-installation-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let metadata = root.join("metadata/toolchain.toml");
        fs::create_dir_all(metadata.parent().expect("metadata parent"))
            .expect("create installation metadata directory");
        fs::write(
            &metadata,
            "schema = 1\nchannel = \"stable\"\nversion = \"1.97.0\"\ndate = \"2026-01-01\"\n",
        )
        .expect("write installation toolchain metadata");
        fs::write(root.join("App.vapor.toml"), "[root]\n").expect("write app manifest");

        let installation = VaporInstallation::from_explicit_root(root.clone())
            .expect("construct explicit installation context");
        let toolchain = ManagedToolchain::discover_for_installation(&installation)
            .expect("discover toolchain for selected installation");

        assert_eq!(toolchain.vapor_home, root);
        assert_eq!(
            toolchain.installation_source,
            crate::InstallationRootSource::Explicit
        );

        fs::remove_dir_all(&installation.root).expect("remove test installation");
    }

    #[test]
    fn installed_toolchain_discovery_does_not_borrow_the_process_workspace() {
        let root = std::env::temp_dir().join(format!(
            "vapor-installation-without-pin-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&root).expect("create app root");
        fs::write(root.join("App.vapor.toml"), "[root]\n").expect("write app manifest");

        let installation = VaporInstallation::from_explicit_root(root.clone())
            .expect("construct explicit installation context");
        let result = ManagedToolchain::discover_for_installation(&installation);

        assert!(matches!(
            result,
            Err(ToolchainError::MissingInstallationMetadata { .. })
        ));

        fs::remove_dir_all(&installation.root).expect("remove test installation");
    }

    fn unique_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after UNIX epoch")
            .as_nanos()
    }
}
