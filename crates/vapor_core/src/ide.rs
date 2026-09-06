//! Project-local IDE integration for a Vapor Superworkspace.
//!
//! Vapor writes only the JetBrains project files it owns beneath the
//! Superworkspace's `.idea` directory. Global IDE configuration remains
//! untouched.

use crate::{ManagedToolchain, VaporSuperworkspace};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const IDEA_DIR: &str = ".idea";
const CARGO_PROJECTS_FILE: &str = "cargoProjects.xml";
const RUST_SETTINGS_FILE: &str = "rust.xml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdeFileState {
    Missing,
    Outdated,
    Current,
}

impl fmt::Display for IdeFileState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Missing => "missing",
            Self::Outdated => "outdated",
            Self::Current => "current",
        })
    }
}

#[derive(Debug, Clone)]
pub struct IdeFileStatus {
    pub path: PathBuf,
    pub state: IdeFileState,
}

#[derive(Debug, Clone)]
pub struct IdeStatus {
    pub project_root: PathBuf,
    pub idea_root: PathBuf,
    pub toolchain_home: PathBuf,
    pub stdlib_source: Option<PathBuf>,
    pub cargo_projects: Vec<PathBuf>,
    pub files: Vec<IdeFileStatus>,
}

impl IdeStatus {
    pub fn is_current(&self) -> bool {
        self.files
            .iter()
            .all(|file| file.state == IdeFileState::Current)
    }
}

#[derive(Debug, Clone)]
pub struct IdeRepairReport {
    pub written: Vec<PathBuf>,
    pub status: IdeStatus,
}

struct IdePlan {
    project_root: PathBuf,
    idea_root: PathBuf,
    toolchain_home: PathBuf,
    stdlib_source: Option<PathBuf>,
    cargo_projects: Vec<PathBuf>,
    files: Vec<IdeFile>,
}

struct IdeFile {
    path: PathBuf,
    contents: String,
}

pub fn inspect_ide(
    superworkspace: &VaporSuperworkspace,
    toolchain: &ManagedToolchain,
) -> Result<IdeStatus, IdeError> {
    build_plan(superworkspace, toolchain)?.status()
}

pub fn repair_ide(
    superworkspace: &VaporSuperworkspace,
    toolchain: &ManagedToolchain,
) -> Result<IdeRepairReport, IdeError> {
    let plan = build_plan(superworkspace, toolchain)?;

    fs::create_dir_all(&plan.idea_root).map_err(|source| IdeError::Io {
        path: plan.idea_root.clone(),
        source,
    })?;

    let mut written = Vec::new();

    for file in &plan.files {
        if file_state(file)? == IdeFileState::Current {
            continue;
        }

        fs::write(&file.path, &file.contents).map_err(|source| IdeError::Io {
            path: file.path.clone(),
            source,
        })?;

        written.push(file.path.clone());
    }

    Ok(IdeRepairReport {
        written,
        status: plan.status()?,
    })
}

fn build_plan(
    superworkspace: &VaporSuperworkspace,
    toolchain: &ManagedToolchain,
) -> Result<IdePlan, IdeError> {
    if !toolchain.is_installed() {
        return Err(IdeError::ToolchainMissing {
            rustc: toolchain.rustc_path.clone(),
        });
    }

    let toolchain_home = toolchain
        .rustc_path
        .parent()
        .ok_or_else(|| IdeError::InvalidToolchain {
            path: toolchain.rustc_path.clone(),
        })?
        .to_path_buf();

    let stdlib_source = toolchain_home
        .parent()
        .map(|root| root.join("lib/rustlib/src/rust/library"))
        .filter(|path| path.is_dir());

    let mut cargo_projects = superworkspace
        .projects
        .iter()
        .map(|project| project.project.cargo_manifest_path.clone())
        .collect::<Vec<_>>();

    cargo_projects.sort();
    cargo_projects.dedup();

    if cargo_projects.is_empty() {
        return Err(IdeError::NoCargoProjects {
            superworkspace: superworkspace.root.clone(),
        });
    }

    let idea_root = superworkspace.root.join(IDEA_DIR);

    let cargo_projects_file = IdeFile {
        path: idea_root.join(CARGO_PROJECTS_FILE),
        contents: cargo_projects_xml(&superworkspace.root, &cargo_projects)?,
    };

    let rust_settings_file = IdeFile {
        path: idea_root.join(RUST_SETTINGS_FILE),
        contents: rust_xml(&toolchain_home, stdlib_source.as_deref()),
    };

    Ok(IdePlan {
        project_root: superworkspace.root.clone(),
        idea_root,
        toolchain_home,
        stdlib_source,
        cargo_projects,
        files: vec![cargo_projects_file, rust_settings_file],
    })
}

impl IdePlan {
    fn status(&self) -> Result<IdeStatus, IdeError> {
        let files = self
            .files
            .iter()
            .map(|file| {
                Ok(IdeFileStatus {
                    path: file.path.clone(),
                    state: file_state(file)?,
                })
            })
            .collect::<Result<Vec<_>, IdeError>>()?;

        Ok(IdeStatus {
            project_root: self.project_root.clone(),
            idea_root: self.idea_root.clone(),
            toolchain_home: self.toolchain_home.clone(),
            stdlib_source: self.stdlib_source.clone(),
            cargo_projects: self.cargo_projects.clone(),
            files,
        })
    }
}

fn cargo_projects_xml(project_root: &Path, cargo_projects: &[PathBuf]) -> Result<String, IdeError> {
    let mut entries = String::new();

    for manifest in cargo_projects {
        let reference = project_reference(project_root, manifest)?;

        entries.push_str("    <cargoProject FILE=\"");

        entries.push_str(&xml_escape(&reference));

        entries.push_str("\" />\n");
    }

    Ok(format!(
        "<!-- Generated by Vapor. -->\n\
             <project version=\"4\">\n\
             <component name=\"CargoProjects\">\n\
             {entries}\
             </component>\n\
             </project>\n"
    ))
}

fn rust_xml(toolchain_home: &Path, stdlib_source: Option<&Path>) -> String {
    let mut source = String::from(
        "<!-- Generated by Vapor. -->\n\
             <project version=\"4\">\n\
             <component name=\"RsProjectSettings\">\n",
    );

    source.push_str(&format!(
        "    <option name=\"toolchainHomeDirectory\" value=\"{}\" />\n",
        xml_escape(&toolchain_home.to_string_lossy(),),
    ));

    if let Some(stdlib_source) = stdlib_source {
        source.push_str(&format!(
            "    <option name=\"explicitPathToStdlib\" value=\"{}\" />\n",
            xml_escape(&stdlib_source.to_string_lossy(),),
        ));
    }

    source.push_str(
        "  </component>\n\
         </project>\n",
    );

    source
}

fn project_reference(project_root: &Path, path: &Path) -> Result<String, IdeError> {
    let relative =
        path.strip_prefix(project_root)
            .map_err(|_| IdeError::ProjectOutsideSuperworkspace {
                project: path.to_path_buf(),
                superworkspace: project_root.to_path_buf(),
            })?;

    if relative.as_os_str().is_empty() {
        return Ok("$PROJECT_DIR$".to_owned());
    }

    Ok(format!(
        "$PROJECT_DIR$/{}",
        relative.to_string_lossy().replace('\\', "/"),
    ))
}

fn file_state(file: &IdeFile) -> Result<IdeFileState, IdeError> {
    if !file.path.is_file() {
        return Ok(IdeFileState::Missing);
    }

    let current = fs::read_to_string(&file.path).map_err(|source| IdeError::Io {
        path: file.path.clone(),
        source,
    })?;

    if current == file.contents {
        Ok(IdeFileState::Current)
    } else {
        Ok(IdeFileState::Outdated)
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[derive(Debug)]
pub enum IdeError {
    ToolchainMissing {
        rustc: PathBuf,
    },

    InvalidToolchain {
        path: PathBuf,
    },

    NoCargoProjects {
        superworkspace: PathBuf,
    },

    ProjectOutsideSuperworkspace {
        project: PathBuf,
        superworkspace: PathBuf,
    },

    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for IdeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ToolchainMissing { rustc } => {
                write!(
                    formatter,
                    "cannot configure the IDE because the Vapor-managed Rust toolchain is missing; expected Rustc at `{}`",
                    rustc.display()
                )
            }

            Self::InvalidToolchain { path } => {
                write!(
                    formatter,
                    "cannot determine the Vapor-managed Rust toolchain root from `{}`",
                    path.display()
                )
            }

            Self::NoCargoProjects { superworkspace } => {
                write!(
                    formatter,
                    "Vapor Superworkspace `{}` contains no current Vapor Projects",
                    superworkspace.display()
                )
            }

            Self::ProjectOutsideSuperworkspace {
                project,
                superworkspace,
            } => {
                write!(
                    formatter,
                    "Vapor Project `{}` is outside Superworkspace `{}`",
                    project.display(),
                    superworkspace.display()
                )
            }

            Self::Io { path, source } => {
                write!(
                    formatter,
                    "failed to access IDE configuration `{}`: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for IdeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
