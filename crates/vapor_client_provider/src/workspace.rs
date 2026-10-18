//! Source-root policy loaded from Vapor manifests and adjacent source structure.
//!
//! `[root]` repositories are pure Vapor super-repositories; their direct Git
//! submodules provide Cargo workspaces. `[workspace]` repositories are normal
//! Vapor/Cargo workspaces rooted in the same directory. Vapor-managed content
//! artifacts inside a workspace are registered by the workspace manifest instead
//! of being inferred from arbitrary nested Vapor manifest files.

use crate::{
    discovery::EnvironmentPaths,
    manifest::{self, ContentKind, VaporEntity},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
};

/// Source root kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceRootKind {
    /// Vapor application source/depot root.
    Root,
    /// Normal source workspace.
    Workspace,
}

/// Source-root configuration and its governed Cargo workspaces.
#[derive(Debug, Clone)]
pub struct WorkspaceManifest {
    kind: SourceRootKind,
    id: String,
    name: String,
    organization: String,
    runtime_targets: Vec<String>,
    cargo: Vec<CargoProject>,
    projects: Vec<WorkspaceProject>,
}

/// One Cargo workspace that Vapor can route workflows through.
#[derive(Debug, Clone)]
pub struct CargoProject {
    name: String,
    manifest: PathBuf,
    documentation: bool,
    binaries: Vec<String>,
    libraries: BTreeMap<String, Vec<String>>,
}

/// One Vapor-managed child content artifact registered by a workspace.
#[derive(Debug, Clone)]
pub struct WorkspaceProject {
    path: PathBuf,
    manifest: PathBuf,
    id: String,
    name: String,
    kind: WorkspaceProjectKind,
}

/// Canonical role of a registered workspace child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceProjectKind {
    /// Content artifact project.
    Content(ContentKind),
}

impl WorkspaceManifest {
    /// Load and validate source-root policy.
    ///
    /// # Errors
    ///
    /// Fails for malformed Vapor identity, unsafe submodule paths, missing
    /// required Cargo manifests, duplicate project names, or an identity
    /// mismatch with startup discovery.
    pub fn load(paths: &EnvironmentPaths) -> Result<Self, String> {
        let source_root = paths.source().root();
        let marker = manifest::source_root_marker(source_root)?;
        let entity = manifest::read(&marker, source_root)?;
        let (kind, id, name, organization) = match entity {
            VaporEntity::Root {
                id,
                name,
                organization,
            } => (SourceRootKind::Root, id, name, organization),
            VaporEntity::Workspace {
                id,
                name,
                organization,
            } => (SourceRootKind::Workspace, id, name, organization),
            VaporEntity::Registry { id, .. } => {
                return Err(format!("registry '{id}' is not a buildable source root"));
            }
            VaporEntity::Content { kind, id, .. } => {
                return Err(format!("{kind} '{id}' is not a source root"));
            }
        };

        if id != paths.source().identity_id() {
            return Err(format!(
                "source identity changed during startup: discovered '{}' but loaded '{}'",
                paths.source().identity_id(),
                id
            ));
        }

        let runtime_targets = source_runtime_targets(&marker, kind)?;
        let cargo = match kind {
            SourceRootKind::Root => discover_root_cargo_workspaces(source_root)?,
            SourceRootKind::Workspace => discover_workspace_cargo(source_root, &name)?,
        };
        validate_projects(&cargo)?;
        let projects = match kind {
            SourceRootKind::Root => Vec::new(),
            SourceRootKind::Workspace => discover_workspace_projects(source_root)?,
        };
        validate_workspace_projects(&projects)?;

        Ok(Self {
            kind,
            id,
            name,
            organization,
            runtime_targets,
            cargo,
            projects,
        })
    }

    /// Source root kind.
    pub fn kind(&self) -> SourceRootKind {
        self.kind
    }

    /// Inferred source-root identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Local source-root name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Owning organization namespace.
    pub fn organization(&self) -> &str {
        &self.organization
    }

    /// Declared release/runtime target matrix for this source root.
    pub fn runtime_targets(&self) -> &[String] {
        &self.runtime_targets
    }

    /// Cargo workspaces governed by this source root.
    pub fn cargo_projects(&self) -> &[CargoProject] {
        &self.cargo
    }

    /// Vapor-managed child projects registered by this source root.
    pub fn projects(&self) -> &[WorkspaceProject] {
        &self.projects
    }
}

impl CargoProject {
    /// Stable command-line project name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Cargo manifest path relative to the source root.
    pub fn manifest(&self) -> &Path {
        &self.manifest
    }

    /// Whether this project contributes installed Rustdoc.
    pub fn documentation(&self) -> bool {
        self.documentation
    }

    /// Binary package names promoted into the installation after a root build.
    pub fn binaries(&self) -> &[String] {
        &self.binaries
    }

    /// Runtime support files promoted beside this project's binaries.
    pub fn libraries(&self, target: &str) -> &[String] {
        self.libraries
            .get(target)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

impl WorkspaceProject {
    /// Project path relative to the source root.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Project manifest path relative to the source root.
    pub fn manifest(&self) -> &Path {
        &self.manifest
    }

    /// Inferred fully-qualified project or content ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Local project name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Canonical project role.
    pub fn kind(&self) -> WorkspaceProjectKind {
        self.kind
    }
}

impl WorkspaceProjectKind {
    /// Return the content kind when this project is a content artifact.
    pub fn content_kind(self) -> ContentKind {
        match self {
            Self::Content(kind) => kind,
        }
    }
}

impl std::fmt::Display for WorkspaceProjectKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Content(kind) => write!(formatter, "{kind}"),
        }
    }
}

fn discover_workspace_cargo(root: &Path, name: &str) -> Result<Vec<CargoProject>, String> {
    let manifest = PathBuf::from("Cargo.toml");
    let absolute = root.join(&manifest);
    if !absolute.is_file() {
        return Err(format!(
            "Vapor workspace '{}' must also be a Cargo workspace with '{}'",
            root.display(),
            absolute.display()
        ));
    }
    let marker = manifest::source_root_marker(root)?;
    Ok(vec![CargoProject {
        name: name.to_owned(),
        manifest,
        documentation: true,
        binaries: workspace_binaries(&marker)?,
        libraries: workspace_libraries(&marker)?,
    }])
}

fn source_runtime_targets(path: &Path, kind: SourceRootKind) -> Result<Vec<String>, String> {
    #[derive(Deserialize)]
    struct SourceManifest {
        root: Option<RuntimeOwner>,
        workspace: Option<RuntimeOwner>,
    }
    #[derive(Deserialize)]
    struct RuntimeOwner {
        runtime: Option<RuntimeSection>,
    }
    #[derive(Deserialize)]
    struct RuntimeSection {
        #[serde(default)]
        targets: Vec<String>,
    }

    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    let manifest: SourceManifest = toml::from_str(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", path.display()))?;
    let targets = match kind {
        SourceRootKind::Root => manifest.root.and_then(|owner| owner.runtime),
        SourceRootKind::Workspace => manifest.workspace.and_then(|owner| owner.runtime),
    }
    .map(|runtime| runtime.targets)
    .unwrap_or_default();
    validate_runtime_targets(path, targets)
}

fn validate_runtime_targets(path: &Path, targets: Vec<String>) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    for target in &targets {
        let valid = !target.is_empty()
            && target.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            });
        if !valid {
            return Err(format!(
                "runtime target must be a Rust target triple such as x86_64-pc-windows-gnullvm in '{}': {target}",
                path.display()
            ));
        }
        if !seen.insert(target.clone()) {
            return Err(format!(
                "duplicate runtime target in '{}': {target}",
                path.display()
            ));
        }
    }
    Ok(targets)
}

fn discover_root_cargo_workspaces(root: &Path) -> Result<Vec<CargoProject>, String> {
    root_cargo_workspace_paths(root)?
        .into_iter()
        .filter_map(|path| {
            let cargo_manifest = root.join(&path).join("Cargo.toml");
            cargo_manifest.is_file().then_some(path)
        })
        .map(|path| {
            let marker = manifest::source_root_marker_candidates(&root.join(&path))
                .into_iter()
                .find(|marker| marker.is_file());
            let name = if let Some(marker) = &marker {
                match manifest::read(marker, root)? {
                    VaporEntity::Workspace { name, .. } => name,
                    VaporEntity::Root { id, .. } => {
                        return Err(format!(
                            "root submodule '{}' declares nested root '{id}'; direct app children must be [workspace]",
                            path.display()
                        ));
                    }
                    VaporEntity::Registry { id, .. } => {
                        return Err(format!(
                            "root submodule '{}' declares registry '{id}'; direct app children must be [workspace]",
                            path.display()
                        ));
                    }
                    VaporEntity::Content { kind, id, .. } => {
                        return Err(format!(
                            "root submodule '{}' declares {kind} '{id}'; direct app children must be [workspace]",
                            path.display()
                        ));
                    }
                }
            } else {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| format!("invalid submodule path '{}'", path.display()))?
                    .to_ascii_lowercase()
            };

            let binaries = if let Some(marker) = &marker {
                workspace_binaries(marker)?
            } else {
                Vec::new()
            };
            let libraries = if let Some(marker) = &marker {
                workspace_libraries(marker)?
            } else {
                BTreeMap::new()
            };

            Ok(CargoProject {
                name,
                manifest: path.join("Cargo.toml"),
                documentation: true,
                binaries,
                libraries,
            })
        })
        .collect()
}

fn root_cargo_workspace_paths(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = submodule_paths(root)?;
    let mut local = fs::read_dir(root)
        .map_err(|error| format!("failed to read root source '{}': {error}", root.display()))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            (name == "Vapor" || name.starts_with("Vapor-")).then(|| PathBuf::from(name.as_ref()))
        })
        .filter(|path| {
            root.join(path)
                .join(manifest::WORKSPACE_FILE_NAME)
                .is_file()
                && root.join(path).join("Cargo.toml").is_file()
        })
        .collect::<Vec<_>>();
    local.sort();
    for path in local {
        validate_relative(&path)?;
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn workspace_binaries(marker: &Path) -> Result<Vec<String>, String> {
    let policy = read_workspace_policy(marker)?;
    validate_binary_names(&policy.binaries, marker)?;
    Ok(policy.binaries)
}

fn workspace_libraries(marker: &Path) -> Result<BTreeMap<String, Vec<String>>, String> {
    let policy = read_workspace_policy(marker)?;
    for (target, libraries) in &policy.runtime.libraries {
        let valid = !target.is_empty()
            && target.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            });
        if !valid {
            return Err(format!(
                "runtime library target must be a Rust target triple in '{}': {target}",
                marker.display()
            ));
        }
        validate_runtime_file_names(libraries, marker, "library")?;
    }
    Ok(policy.runtime.libraries)
}

fn discover_workspace_projects(root: &Path) -> Result<Vec<WorkspaceProject>, String> {
    let marker = manifest::source_root_marker(root)?;
    read_workspace_policy(&marker)?
        .projects
        .into_iter()
        .map(|project| load_workspace_project(root, &project))
        .collect()
}

fn load_workspace_project(
    root: &Path,
    project: &WorkspaceProjectConfig,
) -> Result<WorkspaceProject, String> {
    validate_relative(&project.path)?;
    let absolute = root.join(&project.path);
    if !absolute.is_dir() {
        return Err(format!(
            "registered workspace project '{}' is not a directory",
            project.path.display()
        ));
    }
    let Some(marker) = manifest::content_marker_at(&absolute) else {
        return Err(format!(
            "registered workspace project '{}' is missing a role-specific content manifest",
            project.path.display(),
        ));
    };
    let manifest = marker
        .strip_prefix(root)
        .map_err(|error| {
            format!(
                "registered workspace project manifest '{}' is outside source root '{}': {error}",
                marker.display(),
                root.display()
            )
        })?
        .to_path_buf();
    let entity = manifest::read(&marker, root)?;
    let (id, name, kind) = match entity {
        VaporEntity::Content { kind, id, name } => (id, name, WorkspaceProjectKind::Content(kind)),
        VaporEntity::Root { id, .. } => {
            return Err(format!(
                "registered workspace project '{}' declares root '{id}'; workspace projects must use a content manifest",
                project.path.display()
            ));
        }
        VaporEntity::Workspace { id, .. } => {
            return Err(format!(
                "registered workspace project '{}' declares workspace '{id}'; nested source roots are not workspace projects",
                project.path.display()
            ));
        }
        VaporEntity::Registry { id, .. } => {
            return Err(format!(
                "registered workspace project '{}' declares registry '{id}'; registries are not workspace projects",
                project.path.display()
            ));
        }
    };
    Ok(WorkspaceProject {
        path: project.path.clone(),
        manifest,
        id,
        name,
        kind,
    })
}

fn read_workspace_policy(marker: &Path) -> Result<WorkspacePolicy, String> {
    let source = fs::read_to_string(marker)
        .map_err(|error| format!("failed to read '{}': {error}", marker.display()))?;
    let document: WorkspaceToml = toml::from_str(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", marker.display()))?;
    Ok(document.workspace.unwrap_or_default())
}

#[derive(Debug, Deserialize)]
struct WorkspaceToml {
    workspace: Option<WorkspacePolicy>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct WorkspacePolicy {
    #[serde(default)]
    binaries: Vec<String>,
    #[serde(default)]
    runtime: WorkspaceRuntimePolicy,
    #[serde(default)]
    projects: Vec<WorkspaceProjectConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct WorkspaceRuntimePolicy {
    #[serde(default)]
    libraries: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct WorkspaceProjectConfig {
    path: PathBuf,
}

fn submodule_paths(root: &Path) -> Result<Vec<PathBuf>, String> {
    let path = root.join(".gitmodules");
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix("path")
                .and_then(|tail| tail.trim_start().strip_prefix('='))
                .map(str::trim)
        })
        .map(PathBuf::from)
        .map(|path| {
            validate_relative(&path)?;
            Ok(path)
        })
        .collect()
}

fn validate_projects(projects: &[CargoProject]) -> Result<(), String> {
    let mut names = HashSet::new();
    for project in projects {
        if project.name.is_empty()
            || project.name.chars().any(|character| {
                !character.is_ascii_lowercase() && !character.is_ascii_digit() && character != '-'
            })
        {
            return Err(format!("invalid Cargo workspace name: {}", project.name));
        }
        if !names.insert(&project.name) {
            return Err(format!("duplicate Cargo workspace name: {}", project.name));
        }
        validate_relative(&project.manifest)?;
    }
    Ok(())
}

fn validate_workspace_projects(projects: &[WorkspaceProject]) -> Result<(), String> {
    let mut paths = HashSet::new();
    let mut ids = HashSet::new();
    for project in projects {
        validate_relative(&project.path)?;
        validate_relative(&project.manifest)?;
        if !paths.insert(&project.path) {
            return Err(format!(
                "duplicate workspace project path: {}",
                project.path.display()
            ));
        }
        if !ids.insert(&project.id) {
            return Err(format!("duplicate workspace project id: {}", project.id));
        }
    }
    Ok(())
}

fn validate_binary_names(names: &[String], marker: &Path) -> Result<(), String> {
    let mut seen = HashSet::new();
    for name in names {
        if name.is_empty()
            || name.chars().any(|character| {
                !character.is_ascii_lowercase()
                    && !character.is_ascii_digit()
                    && character != '-'
                    && character != '_'
            })
        {
            return Err(format!(
                "workspace binary names in '{}' must be non-empty lowercase ASCII names",
                marker.display()
            ));
        }
        if !seen.insert(name) {
            return Err(format!(
                "duplicate workspace binary '{}' in '{}'",
                name,
                marker.display()
            ));
        }
    }
    Ok(())
}

fn validate_runtime_file_names(names: &[String], marker: &Path, kind: &str) -> Result<(), String> {
    let mut seen = HashSet::new();
    for name in names {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.chars().any(|character| {
                !character.is_ascii_alphanumeric() && !matches!(character, '-' | '_' | '.')
            })
        {
            return Err(format!(
                "workspace {kind} names in '{}' must be safe file names",
                marker.display()
            ));
        }
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(format!(
                "duplicate workspace {kind} '{}' in '{}'",
                name,
                marker.display()
            ));
        }
    }
    Ok(())
}

fn validate_relative(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        Err(format!(
            "source path must be safe and relative: {}",
            path.display()
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::validate_runtime_file_names;
    use std::path::Path;

    #[test]
    fn runtime_file_names_are_portable_basenames() {
        let marker = Path::new("Workspace.vapor.toml");
        assert!(
            validate_runtime_file_names(
                &["libsteam_api.so".to_owned(), "steam_api64.dll".to_owned()],
                marker,
                "library"
            )
            .is_ok()
        );
        assert!(
            validate_runtime_file_names(&["../steam_api.so".to_owned()], marker, "library")
                .is_err()
        );
        assert!(
            validate_runtime_file_names(
                &["Steam_API.dll".to_owned(), "steam_api.dll".to_owned()],
                marker,
                "library"
            )
            .is_err()
        );
    }
}
