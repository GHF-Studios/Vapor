//! App-local registration and resolution of external Vapor source roots.
//!
//! Authored source deliberately lives outside the Vapor Installation.
//! The Installation remembers known source roots and one active source context.
//!
//! One-shot CLI context resolution follows:
//!
//! 1. explicit command source/root;
//! 2. the most-specific known source containing the process working directory;
//! 3. the remembered active source.
//!
//! The raw process working directory is never promoted into Vapor source
//! identity merely because a command happened to be launched there.
//!
//! A future stateful Vapor Shell may insert its session cursor between explicit
//! command context and ambient process working-directory refinement.

use crate::{InstallationError, VaporInstallation};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const SOURCE_STATE_FILE_NAME: &str = "sources.toml";

#[derive(Debug, Clone)]
pub struct SourceState {
    pub active: Option<PathBuf>,
    pub known: Vec<PathBuf>,
    pub state_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceContextSource {
    Explicit,
    WorkingDirectory,
    Active,
}

impl fmt::Display for SourceContextSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Explicit => "explicit command context",
            Self::WorkingDirectory => "working directory",
            Self::Active => "remembered active source",
        })
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedSourceContext {
    pub root: PathBuf,
    pub source: SourceContextSource,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedSourceState {
    active: Option<PathBuf>,

    #[serde(default)]
    known: Vec<PathBuf>,
}

/// Inspect source registration belonging to one Vapor Installation.
pub fn source_state(installation: &VaporInstallation) -> Result<SourceState, SourceError> {
    let state_path = source_state_path(installation);
    let persisted = load(&state_path)?;

    Ok(SourceState {
        active: persisted.active,
        known: persisted.known,
        state_path,
    })
}

/// Return the currently selected external source root.
pub fn active_source(installation: &VaporInstallation) -> Result<Option<PathBuf>, SourceError> {
    Ok(source_state(installation)?.active)
}

/// Resolve the effective source root for one one-shot Vapor operation.
pub fn resolve_source_context(
    installation: &VaporInstallation,
    explicit: Option<PathBuf>,
) -> Result<ResolvedSourceContext, SourceError> {
    if let Some(root) = explicit {
        return Ok(ResolvedSourceContext {
            root,
            source: SourceContextSource::Explicit,
        });
    }

    let working_directory = std::env::current_dir().map_err(SourceError::CurrentDirectory)?;

    let state = source_state(installation)?;

    select_source_context(&state, &working_directory).ok_or_else(|| SourceError::NoContext {
        working_directory,
        state_path: state.state_path,
    })
}

/// Validate, remember, and select one external source root.
pub fn open_source(
    installation: &VaporInstallation,
    source: &Path,
) -> Result<SourceState, SourceError> {
    let source = fs::canonicalize(source).map_err(|source_error| SourceError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })?;

    if !source.is_dir() {
        return Err(SourceError::NotDirectory { path: source });
    }

    let installation_root =
        fs::canonicalize(&installation.root).unwrap_or_else(|_| installation.root.clone());

    if roots_overlap(&source, &installation_root) {
        return Err(SourceError::OverlapsInstallation {
            source,
            installation: installation_root,
        });
    }

    let state_path = source_state_path(installation);
    let mut persisted = load(&state_path)?;

    if !persisted.known.contains(&source) {
        persisted.known.push(source.clone());
        persisted.known.sort();
    }

    persisted.active = Some(source);

    save(installation, &persisted)?;

    source_state(installation)
}

fn select_source_context(
    state: &SourceState,
    working_directory: &Path,
) -> Option<ResolvedSourceContext> {
    if let Some(root) = state
        .known
        .iter()
        .filter(|root| working_directory.starts_with(root))
        .max_by_key(|root| root.components().count())
    {
        return Some(ResolvedSourceContext {
            root: root.clone(),
            source: SourceContextSource::WorkingDirectory,
        });
    }

    state.active.as_ref().map(|root| ResolvedSourceContext {
        root: root.clone(),
        source: SourceContextSource::Active,
    })
}

fn roots_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn source_state_path(installation: &VaporInstallation) -> PathBuf {
    installation.state_root().join(SOURCE_STATE_FILE_NAME)
}

fn load(path: &Path) -> Result<PersistedSourceState, SourceError> {
    if !path.is_file() {
        return Ok(PersistedSourceState::default());
    }

    let source = fs::read_to_string(path).map_err(|source| SourceError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    toml::from_str(&source).map_err(|error| SourceError::InvalidState {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

fn save(installation: &VaporInstallation, state: &PersistedSourceState) -> Result<(), SourceError> {
    let state_root = installation
        .ensure_state_root()
        .map_err(SourceError::Installation)?;

    let path = state_root.join(SOURCE_STATE_FILE_NAME);

    let source = toml::to_string_pretty(state).map_err(|error| SourceError::Encode {
        message: error.to_string(),
    })?;

    fs::write(&path, source).map_err(|source| SourceError::Io { path, source })
}

#[derive(Debug)]
pub enum SourceError {
    Installation(InstallationError),

    CurrentDirectory(io::Error),

    NoContext {
        working_directory: PathBuf,
        state_path: PathBuf,
    },

    NotDirectory {
        path: PathBuf,
    },

    OverlapsInstallation {
        source: PathBuf,
        installation: PathBuf,
    },

    InvalidState {
        path: PathBuf,
        message: String,
    },

    Encode {
        message: String,
    },

    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installation(error) => error.fmt(formatter),

            Self::CurrentDirectory(error) => {
                write!(
                    formatter,
                    "failed to determine current source context: {error}"
                )
            }

            Self::NoContext {
                working_directory,
                state_path,
            } => {
                write!(
                    formatter,
                    "no Vapor source context is available; `{}` is not inside \
                     a known source and no active source is recorded in `{}`",
                    working_directory.display(),
                    state_path.display(),
                )
            }

            Self::NotDirectory { path } => {
                write!(
                    formatter,
                    "Vapor source root is not a directory: `{}`",
                    path.display()
                )
            }

            Self::OverlapsInstallation {
                source,
                installation,
            } => {
                write!(
                    formatter,
                    "Vapor authored source and the Vapor Installation must be \
                     disjoint; source `{}` overlaps installation `{}`",
                    source.display(),
                    installation.display()
                )
            }

            Self::InvalidState { path, message } => {
                write!(
                    formatter,
                    "invalid Vapor source state `{}`: {message}",
                    path.display()
                )
            }

            Self::Encode { message } => {
                write!(formatter, "failed to encode Vapor source state: {message}")
            }

            Self::Io { path, source } => {
                write!(
                    formatter,
                    "failed to access Vapor source state `{}`: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for SourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Installation(error) => Some(error),
            Self::CurrentDirectory(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(active: Option<&str>, known: &[&str]) -> SourceState {
        SourceState {
            active: active.map(PathBuf::from),
            known: known.iter().map(PathBuf::from).collect(),
            state_path: PathBuf::from("state"),
        }
    }

    #[test]
    fn working_directory_chooses_most_specific_known_source() {
        let state = state(Some("elsewhere"), &["root", "root/nested"]);

        let context = select_source_context(&state, Path::new("root/nested/project")).unwrap();

        assert_eq!(context.source, SourceContextSource::WorkingDirectory);

        assert_eq!(context.root, PathBuf::from("root/nested"));
    }

    #[test]
    fn active_source_is_used_outside_known_sources() {
        let state = state(Some("root"), &["root"]);

        let context = select_source_context(&state, Path::new("unrelated")).unwrap();

        assert_eq!(context.source, SourceContextSource::Active);

        assert_eq!(context.root, PathBuf::from("root"));
    }

    #[test]
    fn unrelated_working_directory_is_not_a_source_context() {
        let state = state(None, &[]);

        assert!(select_source_context(&state, Path::new("somewhere"),).is_none());
    }
}
