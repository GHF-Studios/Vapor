//! Project-local JetBrains/RustRover integration for a Vapor Superworkspace.
//!
//! Vapor reconciles the pieces of JetBrains project state that correspond to
//! Vapor's modeled development environment:
//!
//! - attached Cargo projects;
//! - the Vapor-managed Rust toolchain;
//! - the Vapor-managed Rust standard-library source.
//!
//! JetBrains stores Cargo-project attachment and Rust project settings in the
//! project-local `.idea/workspace.xml`. Vapor therefore reconciles those
//! components in-place while preserving unrelated IDE state.
//!
//! Older Vapor IDE implementations generated standalone `cargoProjects.xml`,
//! `rust.xml`, and `.idea/vapor-toolchain`. Those are obsolete and are removed
//! after the real workspace state has been reconciled.

use crate::{ManagedToolchain, VaporSuperworkspace};
use std::fmt;
use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

const IDEA_DIR: &str = ".idea";
const WORKSPACE_FILE: &str = "workspace.xml";

const CARGO_COMPONENT: &str = "CargoProjects";
const RUST_COMPONENT: &str = "RustProjectSettings";
const LEGACY_RUST_COMPONENT: &str = "RsProjectSettings";

const LEGACY_CARGO_PROJECTS_FILE: &str = "cargoProjects.xml";
const LEGACY_RUST_SETTINGS_FILE: &str = "rust.xml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdeFileState {
    Missing,
    Outdated,
    Current,
    Obsolete,
}

impl fmt::Display for IdeFileState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Missing => "missing",
            Self::Outdated => "outdated",
            Self::Current => "current",
            Self::Obsolete => "obsolete",
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
    pub changed: Vec<PathBuf>,
    pub status: IdeStatus,
}

struct IdePlan {
    project_root: PathBuf,
    idea_root: PathBuf,
    workspace_path: PathBuf,
    toolchain_home: PathBuf,
    stdlib_source: Option<PathBuf>,
    cargo_projects: Vec<PathBuf>,
    cargo_references: Vec<String>,
    legacy_paths: Vec<PathBuf>,
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

    let current = read_workspace(&plan.workspace_path)?;

    let desired = reconcile_workspace(
        current.as_deref(),
        &plan.workspace_path,
        &plan.cargo_references,
        &plan.toolchain_home,
        plan.stdlib_source.as_deref(),
    )?;

    let mut changed = Vec::new();

    if current.as_deref() != Some(desired.as_str()) {
        fs::write(&plan.workspace_path, desired).map_err(|source| IdeError::Io {
            path: plan.workspace_path.clone(),
            source,
        })?;

        changed.push(plan.workspace_path.clone());
    }

    for path in &plan.legacy_paths {
        if !path.exists() {
            continue;
        }

        if path.is_dir() {
            fs::remove_dir_all(path).map_err(|source| IdeError::Io {
                path: path.clone(),
                source,
            })?;
        } else {
            fs::remove_file(path).map_err(|source| IdeError::Io {
                path: path.clone(),
                source,
            })?;
        }

        changed.push(path.clone());
    }

    Ok(IdeRepairReport {
        changed,
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

    let toolchain_root = toolchain_home
        .parent()
        .ok_or_else(|| IdeError::InvalidToolchain {
            path: toolchain.rustc_path.clone(),
        })?;

    // RustRover's explicit stdlib setting points at the rust-src checkout
    // root, not specifically at its `library` child.
    let stdlib_source = toolchain_root
        .join("lib/rustlib/src/rust")
        .is_dir()
        .then(|| toolchain_root.join("lib/rustlib/src/rust"));

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

    let cargo_references = cargo_projects
        .iter()
        .map(|manifest| project_reference(&superworkspace.root, manifest))
        .collect::<Result<Vec<_>, IdeError>>()?;

    let idea_root = superworkspace.root.join(IDEA_DIR);

    let legacy_paths = vec![
        idea_root.join(LEGACY_CARGO_PROJECTS_FILE),
        idea_root.join(LEGACY_RUST_SETTINGS_FILE),
    ];

    Ok(IdePlan {
        project_root: superworkspace.root.clone(),
        workspace_path: idea_root.join(WORKSPACE_FILE),
        idea_root,
        toolchain_home,
        stdlib_source,
        cargo_projects,
        cargo_references,
        legacy_paths,
    })
}

impl IdePlan {
    fn status(&self) -> Result<IdeStatus, IdeError> {
        let current = read_workspace(&self.workspace_path)?;

        let workspace_state = match current {
            None => IdeFileState::Missing,

            Some(current) => {
                let desired = reconcile_workspace(
                    Some(&current),
                    &self.workspace_path,
                    &self.cargo_references,
                    &self.toolchain_home,
                    self.stdlib_source.as_deref(),
                )?;

                if current == desired {
                    IdeFileState::Current
                } else {
                    IdeFileState::Outdated
                }
            }
        };

        let mut files = vec![IdeFileStatus {
            path: self.workspace_path.clone(),
            state: workspace_state,
        }];

        for path in &self.legacy_paths {
            if path.exists() {
                files.push(IdeFileStatus {
                    path: path.clone(),
                    state: IdeFileState::Obsolete,
                });
            }
        }

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

fn read_workspace(path: &Path) -> Result<Option<String>, IdeError> {
    match fs::read_to_string(path) {
        Ok(source) => Ok(Some(source)),

        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),

        Err(source) => Err(IdeError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn reconcile_workspace(
    current: Option<&str>,
    workspace_path: &Path,
    cargo_references: &[String],
    toolchain_home: &Path,
    stdlib_source: Option<&Path>,
) -> Result<String, IdeError> {
    let mut workspace = current
        .unwrap_or(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <project version=\"4\">\n\
             </project>\n",
        )
        .to_owned();

    if !workspace.contains("<project") || !workspace.contains("</project>") {
        return Err(IdeError::MalformedWorkspace {
            path: workspace_path.to_path_buf(),
            message: "missing JetBrains <project> root element".to_owned(),
        });
    }

    let cargo_component = reconcile_cargo_component(&workspace, cargo_references, workspace_path)?;

    workspace = upsert_component(
        &workspace,
        CARGO_COMPONENT,
        &cargo_component,
        workspace_path,
    )?;

    let rust_component =
        reconcile_rust_component(&workspace, toolchain_home, stdlib_source, workspace_path)?;

    workspace = upsert_component(&workspace, RUST_COMPONENT, &rust_component, workspace_path)?;

    workspace = remove_component(&workspace, LEGACY_RUST_COMPONENT, workspace_path)?;

    Ok(workspace)
}

fn reconcile_cargo_component(
    workspace: &str,
    desired: &[String],
    workspace_path: &Path,
) -> Result<String, IdeError> {
    let existing_component =
        find_component_range(workspace, CARGO_COMPONENT)?.map(|range| &workspace[range]);

    let mut existing_projects = Vec::<(String, String)>::new();

    if let Some(component) = existing_component {
        for range in cargo_project_ranges(component)? {
            let block = &component[range.clone()];

            let opening_end = block
                .find('>')
                .ok_or_else(|| IdeError::MalformedWorkspace {
                    path: workspace_path.to_path_buf(),
                    message: "malformed <cargoProject> element".to_owned(),
                })?;

            let opening = &block[..=opening_end];

            if let Some(file) = attribute_value(opening, "FILE") {
                existing_projects.push((xml_unescape(file), block.to_owned()));
            }
        }
    }

    let mut component = String::from("<component name=\"CargoProjects\">\n");

    for reference in desired {
        if let Some((_, existing)) = existing_projects.iter().find(|(file, _)| file == reference) {
            component.push_str("    ");
            component.push_str(existing.trim());
            component.push('\n');
        } else {
            component.push_str(&format!(
                "    <cargoProject FILE=\"{}\" />\n",
                xml_escape(reference),
            ));
        }
    }

    component.push_str("  </component>");

    Ok(component)
}

fn reconcile_rust_component(
    workspace: &str,
    toolchain_home: &Path,
    stdlib_source: Option<&Path>,
    workspace_path: &Path,
) -> Result<String, IdeError> {
    let existing = find_component_range(workspace, RUST_COMPONENT)?
        .map(|range| workspace[range].to_owned())
        .or_else(|| {
            find_component_range(workspace, LEGACY_RUST_COMPONENT)
                .ok()
                .flatten()
                .map(|range| workspace[range].to_owned())
        });

    let mut component = existing.unwrap_or_else(|| {
        String::from(
            "<component name=\"RustProjectSettings\">\n\
                 </component>",
        )
    });

    component = rename_component(&component, RUST_COMPONENT, workspace_path)?;

    let managed_environment = managed_rust_environment_from_toolchain_home(toolchain_home);

    let preserved_environment_entries = if managed_environment.is_some() {
        preserved_rust_environment_entries(&component, workspace_path)?
    } else {
        Vec::new()
    };

    component = remove_option(&component, "toolchainHomeDirectory", workspace_path)?;

    component = remove_option(&component, "explicitPathToStdlib", workspace_path)?;

    // RustRover persists project-wide Rust/Cargo environment variables under
    // RustProjectSettings -> envs -> map.
    //
    // Once Vapor can identify its managed Installation environment, Vapor owns
    // the RUSTUP_HOME and CARGO_HOME entries while preserving unrelated
    // developer-defined environment variables.
    if managed_environment.is_some() {
        component = remove_option(&component, "envs", workspace_path)?;
    }

    let opening_end = component
        .find('>')
        .ok_or_else(|| IdeError::MalformedWorkspace {
            path: workspace_path.to_path_buf(),
            message: "malformed RustProjectSettings component".to_owned(),
        })?
        + 1;

    let mut settings = String::new();

    if let Some((rustup_home, cargo_home)) = managed_environment {
        settings.push_str(
            "\n    <option name=\"envs\">\n\
             \x20     <map>\n",
        );

        settings.push_str(&format!(
            "        <entry key=\"CARGO_HOME\" value=\"{}\" />\n",
            xml_escape(&cargo_home.to_string_lossy(),),
        ));

        settings.push_str(&format!(
            "        <entry key=\"RUSTUP_HOME\" value=\"{}\" />\n",
            xml_escape(&rustup_home.to_string_lossy(),),
        ));

        for entry in preserved_environment_entries {
            settings.push_str("        ");

            settings.push_str(entry.trim());

            settings.push('\n');
        }

        settings.push_str(
            "      </map>\n\
             \x20   </option>",
        );
    }

    settings.push_str(&format!(
        "\n    <option name=\"toolchainHomeDirectory\" value=\"{}\" />",
        xml_escape(&toolchain_home.to_string_lossy(),),
    ));

    if let Some(stdlib_source) = stdlib_source {
        settings.push_str(&format!(
            "\n    <option name=\"explicitPathToStdlib\" value=\"{}\" />",
            xml_escape(&stdlib_source.to_string_lossy(),),
        ));
    }

    component.insert_str(opening_end, &settings);

    Ok(component)
}

/// Recover the complete Vapor-managed Rust environment from the toolchain path.
///
/// Current managed topology:
///
/// <installation>
/// ├── cargo-home/
/// └── rustup-home/
///     └── toolchains/
///         └── <toolchain>/
///             └── bin/
///
/// This deliberately keys off the named `rustup-home` boundary rather than a
/// fixed number of parent traversals.
fn managed_rust_environment_from_toolchain_home(
    toolchain_home: &Path,
) -> Option<(PathBuf, PathBuf)> {
    let rustup_home = toolchain_home
        .ancestors()
        .find(|ancestor| {
            ancestor
                .file_name()
                .is_some_and(|name| name == "rustup-home")
        })?
        .to_path_buf();

    let installation_root = rustup_home.parent()?;

    let cargo_home = installation_root.join("cargo-home");

    Some((rustup_home, cargo_home))
}

/// Preserve developer-defined RustRover environment variables while allowing
/// Vapor to authoritatively replace its own managed RUSTUP_HOME/CARGO_HOME.
///
/// This also cleans up malformed historical state such as:
///
/// <entry
///     key="RUSTUP_HOME"
///     value="/rustup-home CARGO_HOME=/cargo-home"
/// />
///
/// because the complete managed RUSTUP_HOME entry is discarded and rebuilt.
fn preserved_rust_environment_entries(
    component: &str,
    workspace_path: &Path,
) -> Result<Vec<String>, IdeError> {
    let Some(option_range) = find_option_range(component, "envs", workspace_path)? else {
        return Ok(Vec::new());
    };

    let option = &component[option_range];

    let mut entries = Vec::new();

    let mut search_from = 0;

    while let Some(relative) = option[search_from..].find("<entry") {
        let start = search_from + relative;

        let Some(tag_end_relative) = option[start..].find('>') else {
            return Err(IdeError::MalformedWorkspace {
                path: workspace_path.to_path_buf(),
                message: "unterminated RustProjectSettings environment entry".to_owned(),
            });
        };

        let tag_end = start + tag_end_relative + 1;

        let end = if option[start..tag_end].trim_end().ends_with("/>") {
            tag_end
        } else {
            let Some(close_relative) = option[tag_end..].find("</entry>") else {
                return Err(IdeError::MalformedWorkspace {
                    path: workspace_path.to_path_buf(),
                    message: "unterminated RustProjectSettings environment entry".to_owned(),
                });
            };

            tag_end + close_relative + "</entry>".len()
        };

        let block = &option[start..end];

        let opening = &option[start..tag_end];

        let key = attribute_value(opening, "key").map(xml_unescape);

        let vapor_managed = matches!(key.as_deref(), Some("RUSTUP_HOME" | "CARGO_HOME"));

        if !vapor_managed {
            entries.push(block.to_owned());
        }

        search_from = end;
    }

    Ok(entries)
}

fn upsert_component(
    workspace: &str,
    name: &str,
    component: &str,
    workspace_path: &Path,
) -> Result<String, IdeError> {
    if let Some(range) = find_component_range(workspace, name)? {
        let mut updated = workspace.to_owned();
        updated.replace_range(range, component);
        return Ok(updated);
    }

    let project_end =
        workspace
            .rfind("</project>")
            .ok_or_else(|| IdeError::MalformedWorkspace {
                path: workspace_path.to_path_buf(),
                message: "missing closing </project> element".to_owned(),
            })?;

    let mut updated = workspace.to_owned();

    let insertion = if updated[..project_end].ends_with('\n') {
        format!("  {component}\n")
    } else {
        format!("\n  {component}\n")
    };

    updated.insert_str(project_end, &insertion);

    Ok(updated)
}

fn remove_component(
    workspace: &str,
    name: &str,
    _workspace_path: &Path,
) -> Result<String, IdeError> {
    let mut updated = workspace.to_owned();

    while let Some(range) = find_component_range(&updated, name)? {
        updated.replace_range(range, "");
    }

    Ok(updated)
}

fn find_component_range(source: &str, name: &str) -> Result<Option<Range<usize>>, IdeError> {
    let needle = format!("name=\"{name}\"");
    let mut search_from = 0;

    while let Some(relative) = source[search_from..].find(&needle) {
        let attribute_start = search_from + relative;

        let Some(component_start) = source[..attribute_start].rfind("<component") else {
            search_from = attribute_start + needle.len();
            continue;
        };

        let Some(tag_end_relative) = source[component_start..].find('>') else {
            return Err(IdeError::MalformedWorkspace {
                path: PathBuf::from(WORKSPACE_FILE),
                message: format!("unterminated `{name}` component opening tag"),
            });
        };

        let tag_end = component_start + tag_end_relative + 1;

        if attribute_start >= tag_end {
            search_from = attribute_start + needle.len();
            continue;
        }

        if source[component_start..tag_end].trim_end().ends_with("/>") {
            return Ok(Some(component_start..tag_end));
        }

        let Some(close_relative) = source[tag_end..].find("</component>") else {
            return Err(IdeError::MalformedWorkspace {
                path: PathBuf::from(WORKSPACE_FILE),
                message: format!("unterminated `{name}` component"),
            });
        };

        let component_end = tag_end + close_relative + "</component>".len();

        return Ok(Some(component_start..component_end));
    }

    Ok(None)
}

fn cargo_project_ranges(component: &str) -> Result<Vec<Range<usize>>, IdeError> {
    let mut ranges = Vec::new();
    let mut search_from = 0;

    while let Some(relative) = component[search_from..].find("<cargoProject") {
        let start = search_from + relative;

        let Some(tag_end_relative) = component[start..].find('>') else {
            return Err(IdeError::MalformedWorkspace {
                path: PathBuf::from(WORKSPACE_FILE),
                message: "unterminated <cargoProject> opening tag".to_owned(),
            });
        };

        let tag_end = start + tag_end_relative + 1;

        let end = if component[start..tag_end].trim_end().ends_with("/>") {
            tag_end
        } else {
            let Some(close_relative) = component[tag_end..].find("</cargoProject>") else {
                return Err(IdeError::MalformedWorkspace {
                    path: PathBuf::from(WORKSPACE_FILE),
                    message: "unterminated <cargoProject> element".to_owned(),
                });
            };

            tag_end + close_relative + "</cargoProject>".len()
        };

        ranges.push(start..end);
        search_from = end;
    }

    Ok(ranges)
}

fn remove_option(component: &str, name: &str, workspace_path: &Path) -> Result<String, IdeError> {
    let mut updated = component.to_owned();

    while let Some(range) = find_option_range(&updated, name, workspace_path)? {
        updated.replace_range(range, "");
    }

    Ok(updated)
}

fn find_option_range(
    source: &str,
    name: &str,
    workspace_path: &Path,
) -> Result<Option<Range<usize>>, IdeError> {
    let needle = format!("name=\"{name}\"");

    let mut search_from = 0;

    while let Some(relative) = source[search_from..].find(&needle) {
        let attribute_start = search_from + relative;

        let Some(option_start) = source[..attribute_start].rfind("<option") else {
            search_from = attribute_start + needle.len();

            continue;
        };

        let Some(tag_end_relative) = source[option_start..].find('>') else {
            return Err(IdeError::MalformedWorkspace {
                path: workspace_path.to_path_buf(),
                message: format!("unterminated `{name}` option"),
            });
        };

        let tag_end = option_start + tag_end_relative + 1;

        if attribute_start >= tag_end {
            search_from = attribute_start + needle.len();

            continue;
        }

        let element_range = if source[option_start..tag_end].trim_end().ends_with("/>") {
            option_start..tag_end
        } else {
            let Some(close_relative) = source[tag_end..].find("</option>") else {
                return Err(IdeError::MalformedWorkspace {
                    path: workspace_path.to_path_buf(),
                    message: format!("unterminated `{name}` option"),
                });
            };

            option_start..tag_end + close_relative + "</option>".len()
        };

        return Ok(Some(expand_element_range_to_line(source, element_range)));
    }

    Ok(None)
}

fn expand_element_range_to_line(source: &str, range: Range<usize>) -> Range<usize> {
    let line_start = source[..range.start]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);

    let before_element = &source[line_start..range.start];

    let starts_own_line = before_element.chars().all(char::is_whitespace);

    if !starts_own_line {
        return range;
    }

    let after_element = &source[range.end..];

    let trailing_whitespace = after_element
        .chars()
        .take_while(|character| *character == ' ' || *character == '\t' || *character == '\r')
        .map(char::len_utf8)
        .sum::<usize>();

    let after_whitespace = range.end + trailing_whitespace;

    if source.as_bytes().get(after_whitespace) == Some(&b'\n') {
        line_start..after_whitespace + 1
    } else {
        range
    }
}

fn rename_component(
    component: &str,
    name: &str,
    workspace_path: &Path,
) -> Result<String, IdeError> {
    let opening_end = component
        .find('>')
        .ok_or_else(|| IdeError::MalformedWorkspace {
            path: workspace_path.to_path_buf(),
            message: "malformed JetBrains component".to_owned(),
        })?;

    let opening = &component[..opening_end];

    let name_start = opening
        .find("name=\"")
        .ok_or_else(|| IdeError::MalformedWorkspace {
            path: workspace_path.to_path_buf(),
            message: "JetBrains component has no name attribute".to_owned(),
        })?
        + "name=\"".len();

    let name_end = name_start
        + opening[name_start..]
            .find('"')
            .ok_or_else(|| IdeError::MalformedWorkspace {
                path: workspace_path.to_path_buf(),
                message: "malformed JetBrains component name".to_owned(),
            })?;

    let mut updated = component.to_owned();
    updated.replace_range(name_start..name_end, name);

    Ok(updated)
}

fn attribute_value<'a>(opening_tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");

    let start = opening_tag.find(&needle)? + needle.len();

    let end = start + opening_tag[start..].find('"')?;

    Some(&opening_tag[start..end])
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

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
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

    MalformedWorkspace {
        path: PathBuf,
        message: String,
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
                    "cannot configure the IDE because the Vapor-managed Rust \
                     toolchain is missing; expected Rustc at `{}`",
                    rustc.display()
                )
            }

            Self::InvalidToolchain { path } => {
                write!(
                    formatter,
                    "cannot determine the Vapor-managed Rust toolchain root \
                     from `{}`",
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

            Self::MalformedWorkspace { path, message } => {
                write!(
                    formatter,
                    "invalid JetBrains workspace `{}`: {message}",
                    path.display()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconciles_real_workspace_state_without_destroying_other_components() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<project version="4">
  <component name="CargoProjects">
    <cargoProject FILE="$PROJECT_DIR$/sources/Vapor-Root/Vapor/Cargo.toml" />
    <cargoProject FILE="$PROJECT_DIR$/sources/Vapor-Root/Vapor-Examples/Cargo.toml" />
  </component>
  <component name="ChangeListManager">
    <option name="EXAMPLE" value="preserve-me" />
  </component>
  <component name="RustProjectSettings">
    <option name="toolchainHomeDirectory" value="$PROJECT_DIR$/.idea/vapor-toolchain/bin" />
    <option name="compileAllTargets" value="false" />
  </component>
</project>
"#;

        let desired = vec![
            "$PROJECT_DIR$/Vapor-Root/Vapor/Cargo.toml".to_owned(),
            "$PROJECT_DIR$/Vapor-Root/Vapor-Examples/Cargo.toml".to_owned(),
        ];

        let result = reconcile_workspace(
            Some(source),
            Path::new("/tmp/.idea/workspace.xml"),
            &desired,
            Path::new("/steam/loo-cast/rust/bin"),
            Some(Path::new("/steam/loo-cast/rust/lib/rustlib/src/rust")),
        )
        .unwrap();

        assert!(result.contains("$PROJECT_DIR$/Vapor-Root/Vapor/Cargo.toml"));

        assert!(result.contains("$PROJECT_DIR$/Vapor-Root/Vapor-Examples/Cargo.toml"));

        assert!(!result.contains("$PROJECT_DIR$/sources/Vapor-Root"));

        assert!(result.contains("<option name=\"EXAMPLE\" value=\"preserve-me\" />"));

        assert!(result.contains("<option name=\"compileAllTargets\" value=\"false\" />"));

        assert!(result.contains(
            "<option name=\"toolchainHomeDirectory\" value=\"/steam/loo-cast/rust/bin\" />"
        ));

        assert!(
            result.contains(
                "<option name=\"explicitPathToStdlib\" value=\"/steam/loo-cast/rust/lib/rustlib/src/rust\" />"
            )
        );
    }

    #[test]
    fn preserves_existing_desired_cargo_project_state() {
        let source = r#"<project version="4">
  <component name="CargoProjects">
    <cargoProject FILE="$PROJECT_DIR$/Vapor/Cargo.toml">
      <option name="exampleState" value="preserve-me" />
    </cargoProject>
  </component>
</project>
"#;

        let result = reconcile_cargo_component(
            source,
            &["$PROJECT_DIR$/Vapor/Cargo.toml".to_owned()],
            Path::new("/tmp/workspace.xml"),
        )
        .unwrap();

        assert!(result.contains("<option name=\"exampleState\" value=\"preserve-me\" />"));
    }

    #[test]
    fn workspace_reconciliation_is_idempotent() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<project version="4">
  <component name="CargoProjects">
    <cargoProject FILE="$PROJECT_DIR$/sources/Vapor/Cargo.toml" />
  </component>
  <component name="RustProjectSettings">
    <option name="toolchainHomeDirectory" value="$PROJECT_DIR$/.idea/vapor-toolchain/bin" />
    <option name="explicitPathToStdlib" value="/old/stdlib" />
    <option name="compileAllTargets" value="false" />
  </component>
  <component name="ChangeListManager">
    <option name="preserveMe" value="yes" />
  </component>
</project>
"#;

        let cargo_projects = vec![
            "$PROJECT_DIR$/Vapor-Root/Vapor/Cargo.toml".to_owned(),
            "$PROJECT_DIR$/Vapor-Root/Vapor-Examples/Cargo.toml".to_owned(),
        ];

        let workspace_path = Path::new("/tmp/.idea/workspace.xml");

        let toolchain = Path::new("/steam/loo-cast/rust/bin");

        let stdlib = Path::new("/steam/loo-cast/rust/lib/rustlib/src/rust");

        let first = reconcile_workspace(
            Some(source),
            workspace_path,
            &cargo_projects,
            toolchain,
            Some(stdlib),
        )
        .unwrap();

        let second = reconcile_workspace(
            Some(&first),
            workspace_path,
            &cargo_projects,
            toolchain,
            Some(stdlib),
        )
        .unwrap();

        assert_eq!(
            first, second,
            "JetBrains workspace reconciliation must be idempotent"
        );
    }
}
