//! Vapor installation teardown and explicit purge operations.
//!
//! Ordinary uninstall removes machine integration only. Vapor-owned OS user
//! data and authored source survive unless explicitly purged.

use crate::installation::{InstallationError, VAPOR_HOME_ENV, VaporInstallation};
use crate::source::{SourceError, source_state};
use crate::superworkspace::{SuperworkspaceError, VaporSuperworkspace};
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const LEGACY_APP_ROOT_ENV: &str = "LOO_CAST_APP_ROOT";
const PROFILE_BLOCK_START: &str = "# >>> Vapor managed PATH >>>";
const PROFILE_BLOCK_END: &str = "# <<< Vapor managed PATH <<<";

#[derive(Debug, Clone, Copy, Default)]
pub struct UninstallOptions {
    pub purge_app_external: bool,
    pub purge_superworkspace: bool,
}

#[derive(Debug, Clone)]
pub struct UninstallReport {
    pub installation_root: PathBuf,
    pub user_data_root: PathBuf,
    pub superworkspace_root: Option<PathBuf>,
    pub integration: Vec<String>,
    pub app_external_purged: bool,
    pub superworkspace_purged: bool,
}

pub fn uninstall_installation(
    options: UninstallOptions,
) -> Result<UninstallReport, UninstallError> {
    let installation = VaporInstallation::discover().map_err(UninstallError::Installation)?;
    let user_data_root = installation.user_data_root();

    let superworkspace_root = if options.purge_superworkspace {
        Some(active_superworkspace(&installation)?)
    } else {
        None
    };

    if !options.purge_app_external && installation.root.join("state").is_dir() {
        installation
            .ensure_state_root()
            .map_err(UninstallError::Installation)?;
    }

    let integration = remove_machine_integration()?;

    let mut app_external_purged = false;
    let mut superworkspace_purged = false;

    if let Some(superworkspace) = &superworkspace_root {
        superworkspace_purged = remove_owned_tree(
            superworkspace,
            &[installation.root.as_path(), user_data_root.as_path()],
        )?;
    }

    if options.purge_app_external {
        app_external_purged =
            remove_owned_tree(&user_data_root, &[installation.root.as_path()])?;

        let legacy_state = installation.root.join("state");
        if legacy_state.is_dir() {
            fs::remove_dir_all(&legacy_state).map_err(|source| UninstallError::Io {
                path: legacy_state,
                source,
            })?;
            app_external_purged = true;
        }
    }

    Ok(UninstallReport {
        installation_root: installation.root,
        user_data_root,
        superworkspace_root,
        integration,
        app_external_purged,
        superworkspace_purged,
    })
}

fn active_superworkspace(installation: &VaporInstallation) -> Result<PathBuf, UninstallError> {
    let state = source_state(installation).map_err(UninstallError::Source)?;
    let active = state.active.ok_or(UninstallError::NoActiveSuperworkspace)?;
    let superworkspace =
        VaporSuperworkspace::discover_from(&active).map_err(UninstallError::Superworkspace)?;

    Ok(superworkspace.root)
}

fn remove_machine_integration() -> Result<Vec<String>, UninstallError> {
    let mut removed = Vec::new();

    if let Some(anchor) = legacy_anchor_file() {
        if anchor.is_file() {
            fs::remove_file(&anchor).map_err(|source| UninstallError::Io {
                path: anchor.clone(),
                source,
            })?;
            removed.push(format!(
                "removed legacy app-root anchor {}",
                anchor.display()
            ));

            if let Some(parent) = anchor.parent() {
                let _ = fs::remove_dir(parent);
            }
        }
    }

    if cfg!(windows) {
        for variable in [VAPOR_HOME_ENV, LEGACY_APP_ROOT_ENV] {
            if remove_windows_user_environment(variable)? {
                removed.push(format!("removed user environment variable {variable}"));
            }
        }
    } else {
        for profile in shell_profiles() {
            if remove_managed_profile_block(&profile)? {
                removed.push(format!(
                    "removed managed Vapor PATH block from {}",
                    profile.display()
                ));
            }
        }
    }

    Ok(removed)
}

fn legacy_anchor_file() -> Option<PathBuf> {
    if cfg!(windows) {
        env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("USERPROFILE")
                    .map(|home| PathBuf::from(home).join("AppData/Roaming"))
            })
            .map(|root| root.join("loo_cast/app_root.path"))
    } else {
        env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|root| root.join("loo_cast/app_root.path"))
    }
}

fn shell_profiles() -> Vec<PathBuf> {
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };

    [".profile", ".bashrc", ".zshrc"]
        .into_iter()
        .map(|name| home.join(name))
        .collect()
}

fn remove_managed_profile_block(path: &Path) -> Result<bool, UninstallError> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(UninstallError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let mut changed = false;
    let mut inside = false;
    let mut output = String::new();

    for line in source.lines() {
        if line == PROFILE_BLOCK_START {
            changed = true;
            inside = true;
            continue;
        }

        if inside {
            if line == PROFILE_BLOCK_END {
                inside = false;
            }
            continue;
        }

        output.push_str(line);
        output.push('\n');
    }

    if inside {
        return Err(UninstallError::MalformedProfileBlock {
            path: path.to_path_buf(),
        });
    }

    if changed {
        fs::write(path, output).map_err(|source| UninstallError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }

    Ok(changed)
}

fn remove_windows_user_environment(variable: &str) -> Result<bool, UninstallError> {
    let key = r"HKCU\Environment";
    let query = Command::new("reg")
        .args(["query", key, "/V", variable])
        .output()
        .map_err(|source| UninstallError::CommandStart {
            command: format!("reg query {key} /V {variable}"),
            source,
        })?;

    if !query.status.success() {
        return Ok(false);
    }

    let delete = Command::new("reg")
        .args(["delete", key, "/V", variable, "/F"])
        .output()
        .map_err(|source| UninstallError::CommandStart {
            command: format!("reg delete {key} /V {variable} /F"),
            source,
        })?;

    if !delete.status.success() {
        return Err(UninstallError::CommandFailed {
            command: format!("reg delete {key} /V {variable} /F"),
            status: delete.status.to_string(),
        });
    }

    Ok(true)
}

fn remove_owned_tree(path: &Path, protected: &[&Path]) -> Result<bool, UninstallError> {
    if !path.exists() {
        return Ok(false);
    }

    let path = fs::canonicalize(path).map_err(|source| UninstallError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    if path.parent().is_none() || home_dir().is_some_and(|home| same_path(&path, &home)) {
        return Err(UninstallError::UnsafePurgeRoot { path });
    }

    for protected in protected {
        let protected = fs::canonicalize(protected).unwrap_or_else(|_| (*protected).to_path_buf());
        if path.starts_with(&protected) || protected.starts_with(&path) {
            return Err(UninstallError::UnsafePurgeRoot { path });
        }
    }

    fs::remove_dir_all(&path).map_err(|source| UninstallError::Io {
        path: path.clone(),
        source,
    })?;

    Ok(true)
}

fn same_path(left: &Path, right: &Path) -> bool {
    fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf())
        == fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf())
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[derive(Debug)]
pub enum UninstallError {
    Installation(InstallationError),
    Source(SourceError),
    Superworkspace(SuperworkspaceError),
    NoActiveSuperworkspace,
    UnsafePurgeRoot { path: PathBuf },
    MalformedProfileBlock { path: PathBuf },
    CommandStart { command: String, source: io::Error },
    CommandFailed { command: String, status: String },
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for UninstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installation(error) => error.fmt(formatter),
            Self::Source(error) => error.fmt(formatter),
            Self::Superworkspace(error) => error.fmt(formatter),
            Self::NoActiveSuperworkspace => formatter.write_str(
                "--purge-superworkspace requires a remembered active Vapor source/Superworkspace",
            ),
            Self::UnsafePurgeRoot { path } => write!(
                formatter,
                "refusing unsafe purge root `{}`",
                path.display()
            ),
            Self::MalformedProfileBlock { path } => write!(
                formatter,
                "managed Vapor PATH block in `{}` has no closing marker; refusing to rewrite it",
                path.display()
            ),
            Self::CommandStart { command, source } => {
                write!(formatter, "failed to start `{command}`: {source}")
            }
            Self::CommandFailed { command, status } => {
                write!(formatter, "`{command}` failed with {status}")
            }
            Self::Io { path, source } => {
                write!(formatter, "failed to modify `{}`: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for UninstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Installation(error) => Some(error),
            Self::Source(error) => Some(error),
            Self::Superworkspace(error) => Some(error),
            Self::CommandStart { source, .. } | Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
