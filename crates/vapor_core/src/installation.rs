//! Vapor's independently installed per-user home.
//!
//! Steam product roots are separate local products and are passed to Vapor as
//! explicit context through `VAPOR_PRODUCT_ROOT`. They never identify or own
//! the Vapor installation.

use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const VAPOR_HOME_ENV: &str = "VAPOR_HOME";
pub const VAPOR_PRODUCT_ROOT_ENV: &str = "VAPOR_PRODUCT_ROOT";
pub const VAPOR_HOME_MANIFEST_FILE_NAME: &str = "Vapor-Home.toml";

const STATE_DIR: &str = "data/state";
const HOME_MARKER: &str = "schema = 1\n\n[vapor]\nid = \"ghf-studios/vapor\"\n";
const BUNDLE_MARKER: &str = "Vapor-Standalone.toml";
const BUNDLE_DIRS: &[&str] = &["bin", "docs", "examples", "metadata", "resources"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallationRootSource {
    Explicit,
    Environment,
    Executable,
}

impl fmt::Display for InstallationRootSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Explicit => "explicit Vapor home",
            Self::Environment => VAPOR_HOME_ENV,
            Self::Executable => "executable under Vapor home",
        })
    }
}

#[derive(Debug, Clone)]
pub struct VaporInstallation {
    pub root: PathBuf,
    pub root_source: InstallationRootSource,
}

impl VaporInstallation {
    /// Resolve a caller-selected Vapor home. The home must carry Vapor's own
    /// marker; an `App.vapor.toml` Steam product manifest is never sufficient.
    pub fn from_explicit_root(root: PathBuf) -> Result<Self, InstallationError> {
        let canonical_root = fs::canonicalize(&root).map_err(|source| InstallationError::Io {
            path: root.clone(),
            source,
        })?;
        validate_home_marker(&canonical_root)?;
        Ok(Self {
            root: canonical_root,
            root_source: InstallationRootSource::Explicit,
        })
    }

    /// Resolve the default home without creating it. The installer owns
    /// creation and marker installation.
    pub fn default_home_root() -> Result<PathBuf, InstallationError> {
        platform_home_root().ok_or(InstallationError::HomeUnavailable)
    }

    /// Create the canonical home marker and Vapor-owned directory skeleton.
    /// This is used by the standalone installer after it has selected its
    /// destination; ordinary discovery remains read-only.
    pub fn initialize_home(root: &Path) -> Result<Self, InstallationError> {
        fs::create_dir_all(root).map_err(|source| InstallationError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let canonical_root = fs::canonicalize(root).map_err(|source| InstallationError::Io {
            path: root.to_path_buf(),
            source,
        })?;

        if canonical_root.join("App.vapor.toml").is_file()
            && !canonical_root.join(VAPOR_HOME_MANIFEST_FILE_NAME).is_file()
        {
            return Err(InstallationError::InvalidExplicitRoot {
                root: canonical_root,
                reason: "a Steam product root cannot be initialized as Vapor home",
            });
        }

        let marker = canonical_root.join(VAPOR_HOME_MANIFEST_FILE_NAME);
        if !marker.exists() {
            fs::write(&marker, HOME_MARKER).map_err(|source| InstallationError::Io {
                path: marker.clone(),
                source,
            })?;
        }
        validate_home_marker(&canonical_root)?;

        for directory in [
            "bin",
            "config",
            "content",
            "data",
            "docs",
            "examples",
            "metadata",
            "output",
            "resources",
            "sources",
            "cache/build",
            "tools/cargo/bin",
            "tools/cargo/home",
            "tools/rustup/bin",
            "tools/rustup/home",
        ] {
            let path = canonical_root.join(directory);
            fs::create_dir_all(&path).map_err(|source| InstallationError::Io {
                path,
                source,
            })?;
        }

        Ok(Self {
            root: canonical_root,
            root_source: InstallationRootSource::Explicit,
        })
    }

    /// Install the current platform's standalone distribution payload into a
    /// separate Vapor home. The bundle marker is required, and only the
    /// runtime-owned release directories are copied; user data, source
    /// checkouts, caches, and tools are created or managed by their own flows.
    pub fn install_from_bundle(
        bundle_root: &Path,
        home_root: &Path,
    ) -> Result<Self, InstallationError> {
        let bundle_root = fs::canonicalize(bundle_root).map_err(|source| InstallationError::Io {
            path: bundle_root.to_path_buf(),
            source,
        })?;
        if !bundle_root.is_dir() {
            return Err(InstallationError::InvalidBundle {
                root: bundle_root,
                reason: "bundle root is not a directory",
            });
        }
        validate_bundle(&bundle_root)?;

        let home_exists = home_root.exists();
        if !home_exists {
            fs::create_dir_all(home_root).map_err(|source| InstallationError::Io {
                path: home_root.to_path_buf(),
                source,
            })?;
        }
        let canonical_home = fs::canonicalize(home_root).map_err(|source| InstallationError::Io {
            path: home_root.to_path_buf(),
            source,
        })?;

        if canonical_home.join("App.vapor.toml").is_file()
            && !canonical_home
                .join(VAPOR_HOME_MANIFEST_FILE_NAME)
                .is_file()
        {
            return Err(InstallationError::InvalidExplicitRoot {
                root: canonical_home,
                reason: "a Steam product root cannot be initialized as Vapor home",
            });
        }

        if canonical_home == bundle_root {
            return Self::initialize_home(&canonical_home);
        }
        if canonical_home.starts_with(&bundle_root) || bundle_root.starts_with(&canonical_home) {
            return Err(InstallationError::InvalidBundle {
                root: bundle_root,
                reason: "bundle and destination Vapor home must be separate directories",
            });
        }

        for name in BUNDLE_DIRS {
            let source = bundle_root.join(name);
            let target = canonical_home.join(name);
            if !source.is_dir() {
                return Err(InstallationError::InvalidBundle {
                    root: bundle_root,
                    reason: "a required release directory is missing",
                });
            }
            replace_owned_tree(&source, &target)?;
        }

        Self::initialize_home(&canonical_home)
    }

    /// Discover the active standalone Vapor home from inherited context or the
    /// location of the running Vapor binary. There is no Steam-app fallback.
    pub fn discover() -> Result<Self, InstallationError> {
        if let Some(root) = explicit_vapor_home() {
            let mut installation = Self::from_explicit_root(root)?;
            installation.root_source = InstallationRootSource::Environment;
            return Ok(installation);
        }

        let executable = env::current_exe().map_err(InstallationError::CurrentExecutable)?;
        let root = home_root_from_executable(&executable)
            .ok_or_else(|| InstallationError::NotFound {
                executable: executable.clone(),
            })?;
        let mut installation = Self::from_explicit_root(root)?;
        installation.root_source = InstallationRootSource::Executable;
        Ok(installation)
    }

    /// Vapor owns its complete mutable state tree inside this independent home.
    pub fn user_data_root(&self) -> PathBuf {
        self.root.clone()
    }

    /// Canonical persisted state path. No Steam-root or hidden `.vapor`
    /// fallback is consulted.
    pub fn state_root(&self) -> PathBuf {
        self.root.join(STATE_DIR)
    }

    /// Ensure the canonical persisted state directory exists.
    pub fn ensure_state_root(&self) -> Result<PathBuf, InstallationError> {
        let state_root = self.state_root();
        fs::create_dir_all(&state_root).map_err(|source| InstallationError::Io {
            path: state_root.clone(),
            source,
        })?;
        Ok(state_root)
    }
}

fn validate_home_marker(root: &Path) -> Result<(), InstallationError> {
    let marker = root.join(VAPOR_HOME_MANIFEST_FILE_NAME);
    let source = fs::read_to_string(&marker).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            InstallationError::InvalidExplicitRoot {
                root: root.to_path_buf(),
                reason: "Vapor-Home.toml is missing",
            }
        } else {
            InstallationError::Io {
                path: marker.clone(),
                source: error,
            }
        }
    })?;
    let manifest: toml::Value = toml::from_str(&source).map_err(|_| {
        InstallationError::InvalidBundle {
            root: root.to_path_buf(),
            reason: "Vapor-Standalone.toml is not valid TOML",
        }
    })?;
    let manifest_identity = manifest
        .get("vapor")
        .and_then(|vapor| vapor.get("id"))
        .and_then(toml::Value::as_str);
    if manifest.get("schema").and_then(toml::Value::as_integer) != Some(1)
        || manifest_identity != Some("ghf-studios/vapor")
    {
        return Err(InstallationError::InvalidExplicitRoot {
            root: root.to_path_buf(),
            reason: "Vapor-Home.toml does not identify the supported Vapor home",
        });
    }
    Ok(())
}

fn validate_bundle(root: &Path) -> Result<(), InstallationError> {
    let marker = root.join(BUNDLE_MARKER);
    let source = fs::read_to_string(&marker).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            InstallationError::InvalidBundle {
                root: root.to_path_buf(),
                reason: "Vapor-Standalone.toml is missing",
            }
        } else {
            InstallationError::Io {
                path: marker.clone(),
                source: error,
            }
        }
    })?;
    let manifest: toml::Value = toml::from_str(&source).map_err(|_| {
        InstallationError::InvalidBundle {
            root: root.to_path_buf(),
            reason: "Vapor-Standalone.toml is not valid TOML",
        }
    })?;
    let manifest_identity = manifest
        .get("vapor")
        .and_then(|vapor| vapor.get("id"))
        .and_then(toml::Value::as_str);
    if manifest.get("schema").and_then(toml::Value::as_integer) != Some(1)
        || manifest_identity != Some("ghf-studios/vapor")
    {
        return Err(InstallationError::InvalidBundle {
            root: root.to_path_buf(),
            reason: "Vapor-Standalone.toml does not identify the supported release",
        });
    }
    for directory in BUNDLE_DIRS {
        if !root.join(directory).is_dir() {
            return Err(InstallationError::InvalidBundle {
                root: root.to_path_buf(),
                reason: "a required release directory is missing",
            });
        }
    }
    let host_target = crate::HostTarget::current()
        .map_err(|_| InstallationError::InvalidBundle {
            root: root.to_path_buf(),
            reason: "this platform is not a supported Vapor release target",
        })?
        .triple();
    let payload_target = manifest
        .get("payload")
        .and_then(|payload| payload.get("target"))
        .and_then(toml::Value::as_str);
    let declared_targets = manifest
        .get("payload")
        .and_then(|payload| payload.get("targets"))
        .and_then(toml::Value::as_array);
    if payload_target != Some(host_target)
        || !declared_targets.is_some_and(|targets| {
            targets
                .iter()
                .any(|target| target.as_str() == Some(host_target))
        })
    {
        return Err(InstallationError::InvalidBundle {
            root: root.to_path_buf(),
            reason: "the standalone bundle targets a different ABI",
        });
    }
    for executable in ["vapor", "vapor-client-service", "vapor-installer-gui"] {
        let path = root
            .join("bin")
            .join(host_target)
            .join(format!("{executable}{}", env::consts::EXE_SUFFIX));
        if !path.is_file() {
            return Err(InstallationError::InvalidBundle {
                root: root.to_path_buf(),
                reason: "the current platform runtime payload is incomplete",
            });
        }
    }
    Ok(())
}

fn replace_owned_tree(source: &Path, target: &Path) -> Result<(), InstallationError> {
    let parent = target.parent().ok_or_else(|| InstallationError::InvalidBundle {
        root: source.to_path_buf(),
        reason: "install target has no parent directory",
    })?;
    fs::create_dir_all(parent).map_err(|source_error| InstallationError::Io {
        path: parent.to_path_buf(),
        source: source_error,
    })?;
    let name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("payload");
    let staging = parent.join(format!(".{name}.installing-{}", std::process::id()));
    let previous = parent.join(format!(".{name}.previous-{}", std::process::id()));
    for temporary in [&staging, &previous] {
        if temporary.exists() {
            fs::remove_dir_all(temporary).map_err(|source_error| InstallationError::Io {
                path: temporary.to_path_buf(),
                source: source_error,
            })?;
        }
    }
    copy_regular_tree(source, &staging)?;
    let had_previous = target.exists();
    if had_previous {
        fs::rename(target, &previous).map_err(|source_error| InstallationError::Io {
            path: target.to_path_buf(),
            source: source_error,
        })?;
    }
    if let Err(source_error) = fs::rename(&staging, target) {
        if had_previous {
            let _ = fs::rename(&previous, target);
        }
        let _ = fs::remove_dir_all(&staging);
        return Err(InstallationError::Io {
            path: target.to_path_buf(),
            source: source_error,
        });
    }
    if had_previous {
        fs::remove_dir_all(&previous).map_err(|source_error| InstallationError::Io {
            path: previous,
            source: source_error,
        })?;
    }
    Ok(())
}

fn copy_regular_tree(source: &Path, target: &Path) -> Result<(), InstallationError> {
    let metadata = fs::symlink_metadata(source).map_err(|source_error| InstallationError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(InstallationError::InvalidBundle {
            root: source.to_path_buf(),
            reason: "release payload contains a symbolic link",
        });
    }
    if metadata.is_dir() {
        fs::create_dir_all(target).map_err(|source| InstallationError::Io {
            path: target.to_path_buf(),
            source,
        })?;
        for entry in fs::read_dir(source).map_err(|source_error| InstallationError::Io {
            path: source.to_path_buf(),
            source: source_error,
        })? {
            let entry = entry.map_err(|source_error| InstallationError::Io {
                path: source.to_path_buf(),
                source: source_error,
            })?;
            copy_regular_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if metadata.is_file() {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| InstallationError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::copy(source, target).map_err(|source_error| InstallationError::Io {
            path: source.to_path_buf(),
            source: source_error,
        })?;
    }
    Ok(())
}

fn explicit_vapor_home() -> Option<PathBuf> {
    env::var_os(VAPOR_HOME_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn platform_home_root() -> Option<PathBuf> {
    #[cfg(windows)]
    let home = env::var_os("USERPROFILE").or_else(|| env::var_os("HOME"));
    #[cfg(not(windows))]
    let home = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE"));

    home.map(PathBuf::from).map(|home| home.join("Vapor"))
}

fn home_root_from_executable(executable: &Path) -> Option<PathBuf> {
    let executable = fs::canonicalize(executable).ok()?;
    let directory = executable.parent()?;
    if directory.file_name().is_some_and(|name| name == "bin") {
        directory.parent().map(Path::to_path_buf)
    } else if directory
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "bin")
    {
        directory.parent()?.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

#[derive(Debug)]
pub enum InstallationError {
    CurrentExecutable(io::Error),
    NotFound { executable: PathBuf },
    HomeUnavailable,
    InvalidExplicitRoot { root: PathBuf, reason: &'static str },
    InvalidBundle { root: PathBuf, reason: &'static str },
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for InstallationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentExecutable(error) => write!(
                formatter,
                "failed to determine the running Vapor executable: {error}"
            ),
            Self::NotFound { executable } => write!(
                formatter,
                "no standalone Vapor home is active for `{}`; install Vapor or provide {VAPOR_HOME_ENV}",
                executable.display()
            ),
            Self::HomeUnavailable => {
                formatter.write_str("cannot determine the current user's home directory")
            }
            Self::InvalidExplicitRoot { root, reason } => write!(
                formatter,
                "invalid Vapor home `{}`: {reason}",
                root.display()
            ),
            Self::InvalidBundle { root, reason } => write!(
                formatter,
                "invalid standalone Vapor bundle `{}`: {reason}",
                root.display()
            ),
            Self::Io { path, source } => {
                write!(formatter, "failed to access `{}`: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for InstallationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CurrentExecutable(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::NotFound { .. }
            | Self::HomeUnavailable
            | Self::InvalidExplicitRoot { .. }
            | Self::InvalidBundle { .. } => None,
        }
    }
}
