//! Vapor installation identity and root discovery.
//!
//! A Vapor Installation is the local executable/tooling/state boundary.
//!
//! In a shipped Steam installation, Vapor binaries discover the Steam App
//! Instance by walking upward from their own executable location.
//!
//! During the rewrite bootstrap, binaries built from source fall back to the
//! enclosing Vapor Workspace's `.vapor` directory.

use crate::{VaporWorkspace, WorkspaceError};
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const VAPOR_HOME_ENV: &str = "VAPOR_HOME";

const STATE_DIR: &str = "state";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallationRootSource {
    Environment,
    Executable,
    WorkspaceBootstrap,
}

impl fmt::Display for InstallationRootSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Environment => VAPOR_HOME_ENV,
            Self::Executable => "executable root",
            Self::WorkspaceBootstrap => "Workspace bootstrap",
        })
    }
}

#[derive(Debug, Clone)]
pub struct VaporInstallation {
    pub root: PathBuf,
    pub root_source: InstallationRootSource,
}

impl VaporInstallation {
    /// Discover the active Vapor Installation.
    ///
    /// Resolution order:
    ///
    /// 1. Explicit `VAPOR_HOME`.
    /// 2. A normal installed executable beneath `bin/`.
    /// 3. Bootstrap fallback to `<Vapor Workspace>/.vapor`.
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

        let workspace = VaporWorkspace::discover().map_err(InstallationError::Workspace)?;

        Ok(Self::for_workspace(&workspace))
    }

    /// Resolve the Installation associated with a known source Workspace.
    ///
    /// An explicit or executable-relative installation wins. Otherwise the
    /// rewrite bootstrap uses `<workspace>/.vapor`.
    pub fn for_workspace(workspace: &VaporWorkspace) -> Self {
        if let Some(root) = explicit_vapor_home() {
            return Self {
                root,
                root_source: InstallationRootSource::Environment,
            };
        }

        if let Ok(executable) = env::current_exe()
            && let Some(root) = installation_root_from_executable(&executable)
        {
            return Self {
                root,
                root_source: InstallationRootSource::Executable,
            };
        }

        Self {
            root: workspace.root.join(".vapor"),
            root_source: InstallationRootSource::WorkspaceBootstrap,
        }
    }

    pub fn state_root(&self) -> PathBuf {
        self.root.join(STATE_DIR)
    }

    pub fn ensure_state_root(&self) -> Result<PathBuf, InstallationError> {
        let state_root = self.state_root();

        fs::create_dir_all(&state_root).map_err(|source| InstallationError::Io {
            path: state_root.clone(),
            source,
        })?;

        Ok(state_root)
    }
}

fn explicit_vapor_home() -> Option<PathBuf> {
    env::var_os(VAPOR_HOME_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
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

    Workspace(WorkspaceError),

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

            Self::Workspace(error) => error.fmt(formatter),

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
            Self::Workspace(error) => Some(error),
            Self::Io { source, .. } => Some(source),
        }
    }
}
