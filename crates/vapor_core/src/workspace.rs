//! Vapor Workspace discovery and semantic model.
//!
//! A Vapor Workspace is a source-bearing Git repository containing one or more
//! Vapor Projects. `Workspace.vapor.toml` is its local recognition point.

use semver::Version;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const WORKSPACE_MANIFEST_FILE_NAME: &str = "Workspace.vapor.toml";

const WORKSPACE_SCHEMA_VERSION: u32 = 1;
const CARGO_MANIFEST_FILE_NAME: &str = "Cargo.toml";

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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceHeader {
    pub name: String,
    pub organization: String,
    pub version: Version,
    pub repository: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceProjectSpec {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceManifest {
    pub schema: u32,
    pub workspace: WorkspaceHeader,
    pub toolchain: ToolchainPin,

    #[serde(default, rename = "project")]
    pub projects: Vec<WorkspaceProjectSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaporProject {
    pub name: String,
    pub root: PathBuf,
    pub cargo_manifest_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct VaporWorkspace {
    pub root: PathBuf,
    pub manifest: WorkspaceManifest,
    pub projects: Vec<VaporProject>,
}

impl VaporWorkspace {
    pub fn discover() -> Result<Self, WorkspaceError> {
        let start = env::current_dir().map_err(WorkspaceError::CurrentDirectory)?;

        Self::discover_from(&start)
    }

    pub fn discover_from(start: &Path) -> Result<Self, WorkspaceError> {
        let root = find_workspace_root(start).ok_or_else(|| WorkspaceError::WorkspaceNotFound {
            start: start.to_path_buf(),
        })?;

        Self::load(&root)
    }

    pub fn load(root: &Path) -> Result<Self, WorkspaceError> {
        let manifest_path = root.join(WORKSPACE_MANIFEST_FILE_NAME);

        let source = fs::read_to_string(&manifest_path).map_err(|source| WorkspaceError::Io {
            path: manifest_path.clone(),
            source,
        })?;

        let manifest: WorkspaceManifest =
            toml::from_str(&source).map_err(|error| WorkspaceError::Manifest {
                path: manifest_path,
                message: error.to_string(),
            })?;

        if manifest.schema != WORKSPACE_SCHEMA_VERSION {
            return Err(WorkspaceError::UnsupportedSchema {
                found: manifest.schema,
                supported: WORKSPACE_SCHEMA_VERSION,
            });
        }

        if manifest.projects.is_empty() {
            return Err(WorkspaceError::NoProjects);
        }

        let root = fs::canonicalize(root).map_err(|source| WorkspaceError::Io {
            path: root.to_path_buf(),
            source,
        })?;

        let mut names = BTreeSet::new();
        let mut projects = Vec::with_capacity(manifest.projects.len());

        for project in &manifest.projects {
            if !valid_project_name(&project.name) {
                return Err(WorkspaceError::InvalidProjectName {
                    name: project.name.clone(),
                });
            }

            if !names.insert(project.name.clone()) {
                return Err(WorkspaceError::DuplicateProjectName {
                    name: project.name.clone(),
                });
            }

            if project.path.is_absolute() {
                return Err(WorkspaceError::AbsoluteProjectPath {
                    name: project.name.clone(),
                    path: project.path.clone(),
                });
            }

            let unresolved_root = root.join(&project.path);

            let project_root =
                fs::canonicalize(&unresolved_root).map_err(|source| WorkspaceError::Io {
                    path: unresolved_root,
                    source,
                })?;

            if !project_root.starts_with(&root) {
                return Err(WorkspaceError::ProjectOutsideWorkspace {
                    name: project.name.clone(),
                    path: project_root,
                });
            }

            let cargo_manifest_path = project_root.join(CARGO_MANIFEST_FILE_NAME);

            if !cargo_manifest_path.is_file() {
                return Err(WorkspaceError::CargoManifestMissing {
                    name: project.name.clone(),
                    path: cargo_manifest_path,
                });
            }

            projects.push(VaporProject {
                name: project.name.clone(),
                root: project_root,
                cargo_manifest_path,
            });
        }

        Ok(Self {
            root,
            manifest,
            projects,
        })
    }

    pub fn project(&self, name: &str) -> Option<&VaporProject> {
        self.projects.iter().find(|project| project.name == name)
    }
}

fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|root| root.join(WORKSPACE_MANIFEST_FILE_NAME).is_file())
        .map(Path::to_path_buf)
}

fn valid_project_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

#[derive(Debug)]
pub enum WorkspaceError {
    CurrentDirectory(io::Error),

    WorkspaceNotFound { start: PathBuf },

    Io { path: PathBuf, source: io::Error },

    Manifest { path: PathBuf, message: String },

    UnsupportedSchema { found: u32, supported: u32 },

    NoProjects,

    InvalidProjectName { name: String },

    DuplicateProjectName { name: String },

    AbsoluteProjectPath { name: String, path: PathBuf },

    ProjectOutsideWorkspace { name: String, path: PathBuf },

    CargoManifestMissing { name: String, path: PathBuf },
}

impl fmt::Display for WorkspaceError {
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

            Self::Io { path, source } => {
                write!(formatter, "failed to access `{}`: {source}", path.display())
            }

            Self::Manifest { path, message } => {
                write!(
                    formatter,
                    "invalid Vapor Workspace manifest `{}`: {message}",
                    path.display()
                )
            }

            Self::UnsupportedSchema { found, supported } => {
                write!(
                    formatter,
                    "unsupported Vapor Workspace schema {found}; this Vapor supports schema {supported}"
                )
            }

            Self::NoProjects => {
                write!(formatter, "Vapor Workspace declares no Vapor Projects")
            }

            Self::InvalidProjectName { name } => {
                write!(
                    formatter,
                    "invalid Vapor Project name `{name}`; use ASCII letters, digits, `.`, `-`, or `_`"
                )
            }

            Self::DuplicateProjectName { name } => {
                write!(
                    formatter,
                    "Vapor Workspace declares Vapor Project `{name}` more than once"
                )
            }

            Self::AbsoluteProjectPath { name, path } => {
                write!(
                    formatter,
                    "Vapor Project `{name}` uses absolute path `{}`; project paths must be relative to the Workspace",
                    path.display()
                )
            }

            Self::ProjectOutsideWorkspace { name, path } => {
                write!(
                    formatter,
                    "Vapor Project `{name}` resolves outside the Workspace at `{}`",
                    path.display()
                )
            }

            Self::CargoManifestMissing { name, path } => {
                write!(
                    formatter,
                    "Vapor Project `{name}` has no Cargo manifest at `{}`",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for WorkspaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CurrentDirectory(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
