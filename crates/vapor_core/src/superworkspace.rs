//! Vapor Superworkspace discovery.
//!
//! Development storage follows the modeled hierarchy:
//!
//! Vapor Superworkspace
//! ├── Container Repo
//! │   └── Source Repo / Vapor Workspace
//! │       └── Vapor Project
//! └── Source Repo / Vapor Workspace
//!     └── Vapor Project
//!
//! A Superworkspace is a local checkout container and is not itself a Git
//! repository or source-bearing Vapor identity.
//!
//! Container Repos are traversed only through the Source Repo paths explicitly
//! declared by their `.gitmodules`. Vapor does not recursively scan arbitrary
//! filesystem descendants looking for repositories.

use crate::{VaporProject, VaporWorkspace, WORKSPACE_MANIFEST_FILE_NAME};
use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const GITMODULES_FILE: &str = ".gitmodules";

#[derive(Debug, Clone)]
pub struct VaporSuperworkspace {
    pub root: PathBuf,
    pub repositories: Vec<SuperworkspaceRepository>,
    pub projects: Vec<SuperworkspaceProject>,
}

#[derive(Debug, Clone)]
pub struct SuperworkspaceRepository {
    /// Path-like display name relative to the Superworkspace.
    ///
    /// Examples:
    ///
    /// - `Vapor-Root`
    /// - `Vapor-Root/Vapor`
    /// - `Vapor-Root/Vapor-Examples`
    pub name: String,

    pub root: PathBuf,

    pub kind: SuperworkspaceRepositoryKind,
}

#[derive(Debug, Clone)]
pub enum SuperworkspaceRepositoryKind {
    ContainerRepo,

    VaporWorkspace,

    IncompatibleVaporWorkspace { message: String },
}

impl fmt::Display for SuperworkspaceRepositoryKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ContainerRepo => "Container Repo",

            Self::VaporWorkspace => "Vapor Workspace",

            Self::IncompatibleVaporWorkspace { .. } => "incompatible Vapor Workspace",
        })
    }
}

#[derive(Debug, Clone)]
pub struct SuperworkspaceProject {
    pub repository: String,
    pub project: VaporProject,
}

impl VaporSuperworkspace {
    /// Discover the nearest Vapor Superworkspace containing `start`.
    ///
    /// A candidate:
    ///
    /// - is not itself a Container Repo;
    /// - is not itself a Vapor Workspace;
    /// - directly contains at least one Container Repo or Vapor Workspace.
    ///
    /// Container Repo members are subsequently discovered through their
    /// declared `.gitmodules` paths.
    pub fn discover_from(start: &Path) -> Result<Self, SuperworkspaceError> {
        let start = fs::canonicalize(start).map_err(|source| SuperworkspaceError::Io {
            path: start.to_path_buf(),
            source,
        })?;

        let start_directory = if start.is_dir() {
            start.clone()
        } else {
            start
                .parent()
                .ok_or_else(|| SuperworkspaceError::InvalidStart {
                    path: start.clone(),
                })?
                .to_path_buf()
        };

        for candidate in start_directory.ancestors() {
            if is_container_repo(candidate) || is_vapor_workspace(candidate) {
                continue;
            }

            if let Some(superworkspace) = Self::load_candidate(candidate)? {
                return Ok(superworkspace);
            }
        }

        Err(SuperworkspaceError::NotFound {
            start: start_directory,
        })
    }

    fn load_candidate(root: &Path) -> Result<Option<Self>, SuperworkspaceError> {
        let entries = fs::read_dir(root).map_err(|source| SuperworkspaceError::Io {
            path: root.to_path_buf(),
            source,
        })?;

        let mut repositories = Vec::new();

        let mut projects = Vec::new();

        let mut seen_roots = BTreeSet::new();

        for entry in entries {
            let entry = entry.map_err(|source| SuperworkspaceError::Io {
                path: root.to_path_buf(),
                source,
            })?;

            let repository_root = entry.path();

            if !repository_root.is_dir() {
                continue;
            }

            let name = relative_display_name(root, &repository_root);

            if is_container_repo(&repository_root) {
                register_container_repo(
                    root,
                    &repository_root,
                    name,
                    &mut repositories,
                    &mut projects,
                    &mut seen_roots,
                )?;

                continue;
            }

            if is_vapor_workspace(&repository_root) {
                register_workspace(
                    root,
                    &repository_root,
                    name,
                    &mut repositories,
                    &mut projects,
                    &mut seen_roots,
                )?;
            }
        }

        if repositories.is_empty() {
            return Ok(None);
        }

        repositories.sort_by(|left, right| left.name.cmp(&right.name));

        projects.sort_by(|left, right| {
            left.repository
                .cmp(&right.repository)
                .then_with(|| left.project.name.cmp(&right.project.name))
        });

        Ok(Some(Self {
            root: root.to_path_buf(),
            repositories,
            projects,
        }))
    }
}

fn register_container_repo(
    superworkspace_root: &Path,
    container_root: &Path,
    name: String,
    repositories: &mut Vec<SuperworkspaceRepository>,
    projects: &mut Vec<SuperworkspaceProject>,
    seen_roots: &mut BTreeSet<PathBuf>,
) -> Result<(), SuperworkspaceError> {
    let canonical = canonical_or_original(container_root);

    if !seen_roots.insert(canonical) {
        return Ok(());
    }

    repositories.push(SuperworkspaceRepository {
        name,
        root: container_root.to_path_buf(),
        kind: SuperworkspaceRepositoryKind::ContainerRepo,
    });

    let submodules = container_submodule_paths(container_root)?;

    for relative_path in submodules {
        if relative_path.is_absolute() {
            return Err(SuperworkspaceError::InvalidContainerRepo {
                container: container_root.to_path_buf(),
                message: format!("submodule path `{}` is absolute", relative_path.display()),
            });
        }

        let workspace_root = container_root.join(&relative_path);

        // A declared submodule may not currently be initialized/checked out.
        // That is valid local development storage state; it simply contributes
        // no local Workspace or Project yet.
        if !workspace_root.is_dir() {
            continue;
        }

        if is_container_repo(&workspace_root) {
            return Err(SuperworkspaceError::NestedContainerRepo {
                container: container_root.to_path_buf(),
                nested: workspace_root,
            });
        }

        if !is_vapor_workspace(&workspace_root) {
            continue;
        }

        let name = relative_display_name(superworkspace_root, &workspace_root);

        register_workspace(
            superworkspace_root,
            &workspace_root,
            name,
            repositories,
            projects,
            seen_roots,
        )?;
    }

    Ok(())
}

fn register_workspace(
    _superworkspace_root: &Path,
    workspace_root: &Path,
    name: String,
    repositories: &mut Vec<SuperworkspaceRepository>,
    projects: &mut Vec<SuperworkspaceProject>,
    seen_roots: &mut BTreeSet<PathBuf>,
) -> Result<(), SuperworkspaceError> {
    let canonical = canonical_or_original(workspace_root);

    if !seen_roots.insert(canonical) {
        return Ok(());
    }

    match VaporWorkspace::load(workspace_root) {
        Ok(workspace) => {
            for project in workspace.projects {
                projects.push(SuperworkspaceProject {
                    repository: name.clone(),
                    project,
                });
            }

            repositories.push(SuperworkspaceRepository {
                name,
                root: workspace_root.to_path_buf(),
                kind: SuperworkspaceRepositoryKind::VaporWorkspace,
            });
        }

        Err(error) => {
            repositories.push(SuperworkspaceRepository {
                name,
                root: workspace_root.to_path_buf(),
                kind: SuperworkspaceRepositoryKind::IncompatibleVaporWorkspace {
                    message: error.to_string(),
                },
            });
        }
    }

    Ok(())
}

/// Read Source Repo paths from a Container Repo's `.gitmodules`.
///
/// `.gitmodules` is Git config syntax rather than TOML. For the single piece
/// of information Vapor needs here, parsing `path = ...` assignments is both
/// sufficient and avoids spawning Git merely to understand already-authored
/// Container Repo topology.
fn container_submodule_paths(container_root: &Path) -> Result<Vec<PathBuf>, SuperworkspaceError> {
    let path = container_root.join(GITMODULES_FILE);

    let source = fs::read_to_string(&path).map_err(|source| SuperworkspaceError::Io {
        path: path.clone(),
        source,
    })?;

    let mut paths = Vec::new();

    for line in source.lines() {
        let line = line.trim();

        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with(';')
            || line.starts_with('[')
        {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        if key.trim() != "path" {
            continue;
        }

        let value = value.trim();

        if value.is_empty() {
            return Err(SuperworkspaceError::InvalidContainerRepo {
                container: container_root.to_path_buf(),
                message: "empty submodule path in .gitmodules".to_owned(),
            });
        }

        paths.push(PathBuf::from(value));
    }

    paths.sort();
    paths.dedup();

    Ok(paths)
}

fn relative_display_name(superworkspace_root: &Path, repository_root: &Path) -> String {
    repository_root
        .strip_prefix(superworkspace_root)
        .unwrap_or(repository_root)
        .to_string_lossy()
        .replace('\\', "/")
}

fn canonical_or_original(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn is_container_repo(root: &Path) -> bool {
    root.join(GITMODULES_FILE).is_file()
}

fn is_vapor_workspace(root: &Path) -> bool {
    root.join(WORKSPACE_MANIFEST_FILE_NAME).is_file()
}

#[derive(Debug)]
pub enum SuperworkspaceError {
    InvalidStart { path: PathBuf },

    NotFound { start: PathBuf },

    InvalidContainerRepo { container: PathBuf, message: String },

    NestedContainerRepo { container: PathBuf, nested: PathBuf },

    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for SuperworkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStart { path } => {
                write!(
                    formatter,
                    "cannot discover a Vapor Superworkspace from `{}`",
                    path.display()
                )
            }

            Self::NotFound { start } => {
                write!(
                    formatter,
                    "could not find a Vapor Superworkspace containing `{}`",
                    start.display()
                )
            }

            Self::InvalidContainerRepo { container, message } => {
                write!(
                    formatter,
                    "invalid Vapor Container Repo `{}`: {message}",
                    container.display()
                )
            }

            Self::NestedContainerRepo { container, nested } => {
                write!(
                    formatter,
                    "Vapor Container Repo `{}` contains nested Container Repo `{}`; \
                     Container Repos may not be submodules of other Container Repos",
                    container.display(),
                    nested.display()
                )
            }

            Self::Io { path, source } => {
                write!(
                    formatter,
                    "failed to inspect Vapor development storage `{}`: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for SuperworkspaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),

            _ => None,
        }
    }
}
