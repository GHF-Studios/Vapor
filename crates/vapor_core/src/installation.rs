//! Vapor installation identity and root discovery.
//!
//! A Vapor Installation is the replaceable App Instance boundary. Mutable
//! persistent Vapor state belongs to OS user data instead of the Steam depot.
//!
//! A normal installed Vapor discovers its App Instance by walking upward from
//! its own executable location.
//!
//! Vapor processes launched indirectly from authored source inherit
//! `VAPOR_HOME` from the installed Vapor process that spawned them.
//!
//! Authored Workspaces are never Vapor Installations.

use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const VAPOR_HOME_ENV: &str = "VAPOR_HOME";

const STATE_DIR: &str = "state";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallationRootSource {
    Explicit,
    Environment,
    Executable,
}

impl fmt::Display for InstallationRootSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Explicit => "explicit app root",
            Self::Environment => VAPOR_HOME_ENV,
            Self::Executable => "executable root",
        })
    }
}

#[derive(Debug, Clone)]
pub struct VaporInstallation {
    pub root: PathBuf,
    pub root_source: InstallationRootSource,
}

impl VaporInstallation {
    /// Construct an installation context from a caller-selected app root.
    ///
    /// Use this when an operation receives an explicit installation target;
    /// it keeps every downstream path calculation tied to that target instead
    /// of rediscovering a potentially different root from process state.
    pub fn from_explicit_root(root: PathBuf) -> Result<Self, InstallationError> {
        let canonical_root = fs::canonicalize(&root).map_err(|source| InstallationError::Io {
            path: root.clone(),
            source,
        })?;
        if canonical_root.join("Vapor-Client.vapor.toml").is_file() {
            return Err(InstallationError::InvalidExplicitRoot {
                root: canonical_root,
                reason: "this is an app source root, not an installed app root",
            });
        }
        let manifest = canonical_root.join("App.vapor.toml");
        if !manifest.is_file() {
            return Err(InstallationError::InvalidExplicitRoot {
                root: canonical_root,
                reason: "App.vapor.toml is missing",
            });
        }
        let source = fs::read_to_string(&manifest).map_err(|source| InstallationError::Io {
            path: manifest.clone(),
            source,
        })?;
        if !source.lines().any(|line| line.trim() == "[root]") {
            return Err(InstallationError::InvalidExplicitRoot {
                root: canonical_root,
                reason: "App.vapor.toml does not declare [root]",
            });
        }
        Ok(Self {
            root: canonical_root,
            root_source: InstallationRootSource::Explicit,
        })
    }

    /// Discover the active Vapor Installation.
    ///
    /// Resolution order:
    ///
    /// 1. Explicit/inherited `VAPOR_HOME`.
    /// 2. An installed Vapor executable beneath the App Instance's `bin/`.
    ///
    /// There is intentionally no authored-Workspace fallback. A Vapor source
    /// checkout is not an Installation.
    pub fn discover() -> Result<Self, InstallationError> {
        if let Some(root) = explicit_vapor_home() {
            return Ok(Self {
                root,
                root_source: InstallationRootSource::Environment,
            });
        }

        let executable = env::current_exe().map_err(InstallationError::CurrentExecutable)?;

        if let Some(root) = installation_root_from_executable(&executable) {
            return Ok(Self {
                root,
                root_source: InstallationRootSource::Executable,
            });
        }

        Err(InstallationError::NotFound { executable })
    }

    /// OS-owned mutable Vapor state, intentionally outside the Steam App Instance.
    pub fn user_data_root(&self) -> PathBuf {
        platform_user_data_root().unwrap_or_else(|| env::temp_dir().join("vapor-user-data"))
    }

    /// Resolve persisted state while remaining compatible with the pre-userdata
    /// layout until migration has actually occurred.
    pub fn state_root(&self) -> PathBuf {
        let external = self.external_state_root();
        let legacy = self.legacy_state_root();

        if external.exists() || !legacy.exists() {
            external
        } else {
            legacy
        }
    }

    /// Ensure state is external to the Steam App Instance and migrate the
    /// previous `<installation>/state` tree before any new writes occur.
    pub fn ensure_state_root(&self) -> Result<PathBuf, InstallationError> {
        let state_root = self.external_state_root();
        let legacy = self.legacy_state_root();

        if !state_root.exists() && legacy.is_dir() {
            let parent = state_root.parent().ok_or_else(|| InstallationError::Io {
                path: state_root.clone(),
                source: io::Error::new(io::ErrorKind::InvalidInput, "state root has no parent"),
            })?;
            fs::create_dir_all(parent).map_err(|source| InstallationError::Io {
                path: parent.to_path_buf(),
                source,
            })?;

            let temporary = parent.join(format!(".state-migrate-{}", std::process::id()));
            if temporary.exists() {
                fs::remove_dir_all(&temporary).map_err(|source| InstallationError::Io {
                    path: temporary.clone(),
                    source,
                })?;
            }

            copy_missing_tree(&legacy, &temporary)?;
            fs::rename(&temporary, &state_root).map_err(|source| InstallationError::Io {
                path: state_root.clone(),
                source,
            })?;
            fs::remove_dir_all(&legacy).map_err(|source| InstallationError::Io {
                path: legacy,
                source,
            })?;

            return Ok(state_root);
        }

        fs::create_dir_all(&state_root).map_err(|source| InstallationError::Io {
            path: state_root.clone(),
            source,
        })?;

        // If an external state tree already exists from a newer build, merge
        // only legacy files that do not have an external successor.
        if legacy.is_dir() {
            copy_missing_tree(&legacy, &state_root)?;
            fs::remove_dir_all(&legacy).map_err(|source| InstallationError::Io {
                path: legacy,
                source,
            })?;
        }

        Ok(state_root)
    }

    fn external_state_root(&self) -> PathBuf {
        self.user_data_root().join(STATE_DIR)
    }

    fn legacy_state_root(&self) -> PathBuf {
        self.root.join(STATE_DIR)
    }
}

fn explicit_vapor_home() -> Option<PathBuf> {
    env::var_os(VAPOR_HOME_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn platform_user_data_root() -> Option<PathBuf> {
    match env::consts::OS {
        "windows" => env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("AppData/Local"))
            })
            .map(|root| root.join("Vapor")),

        "macos" => env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Library/Application Support/Vapor")),

        _ => env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
            .map(|root| root.join("vapor")),
    }
}

fn copy_missing_tree(source: &Path, destination: &Path) -> Result<(), InstallationError> {
    fs::create_dir_all(destination).map_err(|source_error| InstallationError::Io {
        path: destination.to_path_buf(),
        source: source_error,
    })?;

    for entry in fs::read_dir(source).map_err(|source_error| InstallationError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })? {
        let entry = entry.map_err(|source_error| InstallationError::Io {
            path: source.to_path_buf(),
            source: source_error,
        })?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|source_error| InstallationError::Io {
                path: source_path.clone(),
                source: source_error,
            })?;

        if file_type.is_dir() {
            copy_missing_tree(&source_path, &destination_path)?;
        } else if file_type.is_file() && !destination_path.exists() {
            fs::copy(&source_path, &destination_path).map_err(|source_error| {
                InstallationError::Io {
                    path: destination_path.clone(),
                    source: source_error,
                }
            })?;
        }
    }

    Ok(())
}

fn installation_root_from_executable(executable: &Path) -> Option<PathBuf> {
    let executable = fs::canonicalize(executable).ok()?;

    let directory = executable.parent()?;

    // <installation>/bin/vapor
    if directory.file_name().is_some_and(|name| name == "bin") {
        return directory.parent().map(Path::to_path_buf);
    }

    // <installation>/bin/<target>/vapor
    if directory
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "bin")
    {
        return directory
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
    }

    None
}

#[derive(Debug)]
pub enum InstallationError {
    CurrentExecutable(io::Error),

    NotFound { executable: PathBuf },

    InvalidExplicitRoot { root: PathBuf, reason: &'static str },

    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for InstallationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentExecutable(error) => {
                write!(
                    formatter,
                    "failed to determine current Vapor executable: {error}"
                )
            }

            Self::NotFound { executable } => {
                write!(
                    formatter,
                    "no active Vapor Installation could be resolved for `{}`; \
                     run through an installed Vapor App Instance or provide \
                     inherited {VAPOR_HOME_ENV}",
                    executable.display(),
                )
            }

            Self::InvalidExplicitRoot { root, reason } => write!(
                formatter,
                "invalid explicit Vapor app root `{}`: {reason}",
                root.display(),
            ),

            Self::Io { path, source } => {
                write!(
                    formatter,
                    "failed to access Vapor installation state `{}`: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for InstallationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CurrentExecutable(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::NotFound { .. } => None,
            Self::InvalidExplicitRoot { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InstallationRootSource, VaporInstallation};

    #[test]
    fn explicit_installation_context_keeps_the_selected_root() {
        let root = std::env::temp_dir().join(format!(
            "vapor-explicit-root-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after UNIX epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create explicit app root");
        std::fs::write(root.join("App.vapor.toml"), "[root]\n").expect("write app manifest");

        let installation = VaporInstallation::from_explicit_root(root.clone())
            .expect("construct explicit installation context");

        assert_eq!(
            installation.root,
            std::fs::canonicalize(&root).expect("canonicalize root")
        );
        assert_eq!(installation.root_source, InstallationRootSource::Explicit);
        std::fs::remove_dir_all(root).expect("remove test app root");
    }

    #[test]
    fn explicit_installation_context_rejects_unmarked_directories() {
        let root = std::env::temp_dir().join(format!(
            "vapor-unmarked-root-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after UNIX epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create unmarked directory");

        let error = VaporInstallation::from_explicit_root(root.clone())
            .expect_err("unmarked directory must not become an installation");

        assert!(matches!(
            error,
            super::InstallationError::InvalidExplicitRoot { .. }
        ));
        std::fs::remove_dir_all(root).expect("remove test directory");
    }
}
