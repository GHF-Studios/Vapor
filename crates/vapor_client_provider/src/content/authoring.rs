//! Source-workspace discovery and authored-content editing.
//!
//! This module owns operations over creator-authored workspace files. Installed
//! content, package builds, and Workshop transport remain in the parent provider.

use super::*;
use std::io::Write;
use vapor_content::{ContentCandidates, VaporId};

/// One valid, unregistered content project that may be explicitly adopted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProjectCandidate {
    /// Root directory of the unregistered content project.
    pub path: PathBuf,
    /// ID declared by the project's content manifest.
    pub id: String,
    /// Content kind declared by the project.
    pub kind: ContentKind,
}

/// Candidates found by a non-mutating project scan. Invalid folders are
/// returned as diagnostics and never enter the content catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceProjectScan {
    /// Valid unregistered projects available for explicit adoption.
    pub candidates: Vec<SourceProjectCandidate>,
    /// Reasons project folders were excluded from the candidate list.
    pub diagnostics: Vec<String>,
}

/// Identity and location of a newly initialized source workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicSourceWorkspaceReport {
    /// Canonical generated workspace folder.
    pub root: PathBuf,
    /// Generated Vapor workspace identity.
    pub workspace_id: String,
    /// Generated playable Packagepack identity.
    pub packagepack_id: String,
}

/// One missing or stale Workshop identifier repaired in authored dependencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMetadataRepair {
    /// Manifest containing the source dependency.
    pub manifest: PathBuf,
    /// TOML section containing that dependency.
    pub section: String,
    /// Identity of the referenced source project.
    pub reference_id: String,
    /// Workshop identifier copied from the referenced project.
    pub workshop_id: String,
}

/// Source metadata repair preview or write result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceMetadataRepairReport {
    /// Changes found or applied, depending on the requested mode.
    pub actions: Vec<SourceMetadataRepair>,
}

/// Find unregistered content folders for an explicit creator adoption action.
pub fn scan_unregistered_source_projects(
    paths: &EnvironmentPaths,
) -> Result<SourceProjectScan, String> {
    let workspace = WorkspaceManifest::load(paths)?;
    if workspace.kind() != crate::workspace::SourceRootKind::Workspace {
        return Err("content projects can only be discovered inside a Vapor Workspace".to_owned());
    }
    let source_root = fs::canonicalize(paths.source().root())
        .map_err(io("resolve source workspace", paths.source().root()))?;
    workspace_version(&source_root)?;
    let registered_ids = workspace
        .projects()
        .iter()
        .map(|project| project.id().to_owned())
        .collect::<BTreeSet<_>>();
    let registered_paths = workspace
        .projects()
        .iter()
        .filter_map(|project| fs::canonicalize(source_root.join(project.path())).ok())
        .collect::<BTreeSet<_>>();
    let mut scan = SourceProjectScan::default();
    let mut pending = vec![source_root.clone()];
    let mut candidates_by_id = BTreeMap::<String, SourceProjectCandidate>::new();
    let mut duplicate_ids = BTreeSet::new();
    let mut visited = 0usize;
    while let Some(directory) = pending.pop() {
        visited += 1;
        if visited > 100_000 {
            scan.diagnostics.push("Stopped scanning after 100,000 folders; narrow the Workspace or move generated files under an excluded folder.".to_owned());
            break;
        }
        let mut entries = Vec::new();
        match fs::read_dir(&directory) {
            Ok(directory_entries) => {
                for entry in directory_entries {
                    match entry {
                        Ok(entry) => entries.push(entry),
                        Err(error) => scan.diagnostics.push(format!(
                            "Could not inspect an entry in {}: {error}",
                            directory.display()
                        )),
                    }
                }
            }
            Err(error) => {
                scan.diagnostics.push(format!(
                    "Could not inspect {}: {error}",
                    directory.display()
                ));
                continue;
            }
        }
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries.into_iter().rev() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.')
                || matches!(
                    name.as_ref(),
                    "target" | "node_modules" | "vendor" | "output"
                )
            {
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    scan.diagnostics.push(format!(
                        "Could not inspect {}: {error}",
                        entry.path().display()
                    ));
                    continue;
                }
            };
            if !file_type.is_dir() {
                continue;
            }
            let path = match fs::canonicalize(entry.path()) {
                Ok(path) if path.starts_with(&source_root) => path,
                Ok(_) => continue,
                Err(error) => {
                    scan.diagnostics.push(format!(
                        "Could not resolve {}: {error}",
                        entry.path().display()
                    ));
                    continue;
                }
            };
            if registered_paths.contains(&path) {
                continue;
            }
            if let Some(marker) = manifest::content_marker_at(&path) {
                let marker_metadata = fs::symlink_metadata(&marker);
                let entity = match marker_metadata {
                    Ok(metadata)
                        if metadata.file_type().is_file() && !metadata.file_type().is_symlink() =>
                    {
                        manifest::read(&marker, &source_root)
                    }
                    Ok(_) => Err(format!(
                        "content manifest must be a regular non-symlink file: {}",
                        marker.display()
                    )),
                    Err(error) => Err(format!("could not inspect {}: {error}", marker.display())),
                };
                match entity {
                    Ok(VaporEntity::Content { id, kind, .. }) if !registered_ids.contains(&id) => {
                        match read_authored_content(&marker, kind) {
                            Ok(_) => match path.strip_prefix(&source_root).ok().and_then(Path::to_str) {
                                Some(relative) => {
                                    let candidate = SourceProjectCandidate {
                                        path: PathBuf::from(relative),
                                        id: id.clone(),
                                        kind,
                                    };
                                    if candidates_by_id.insert(id.clone(), candidate).is_some() {
                                        duplicate_ids.insert(id.clone());
                                    }
                                }
                                None => scan.diagnostics.push(format!("Project path is not valid Unicode and cannot be written to the Workspace manifest: {}", path.display())),
                            },
                            Err(error) => scan.diagnostics.push(error),
                        }
                    }
                    Ok(VaporEntity::Content { id, .. }) => scan.diagnostics.push(format!("{} duplicates registered content identity {id}", path.display())),
                    Ok(_) => scan.diagnostics.push(format!("{} is not a content project", marker.display())),
                    Err(error) => scan.diagnostics.push(error),
                }
                continue;
            }
            pending.push(path);
        }
    }
    for id in duplicate_ids {
        candidates_by_id.remove(&id);
        scan.diagnostics.push(format!("More than one unregistered folder declares content identity {id}; resolve the duplicate before adoption."));
    }
    scan.candidates = candidates_by_id.into_values().collect();
    scan.candidates
        .sort_by(|left, right| left.path.cmp(&right.path));
    scan.diagnostics.sort();
    Ok(scan)
}

fn workspace_version(root: &Path) -> Result<Option<semver::Version>, String> {
    let path = root.join(manifest::WORKSPACE_FILE_NAME);
    let source = fs::read_to_string(&path).map_err(io("read source manifest", &path))?;
    #[derive(Deserialize)]
    struct Root {
        workspace: Option<WorkspaceMeta>,
    }
    #[derive(Deserialize)]
    struct WorkspaceMeta {
        version: Option<String>,
    }
    let parsed: Root = toml::from_str(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", path.display()))?;
    parsed
        .workspace
        .and_then(|workspace| workspace.version)
        .map(|version| {
            semver::Version::parse(&version).map_err(|error| {
                format!(
                    "invalid workspace semantic version '{version}' in '{}': {error}",
                    path.display()
                )
            })
        })
        .transpose()
}

/// Discover source-authored content artifacts in the active workspace.
///
/// # Errors
///
/// Returns filesystem, TOML, or manifest validation diagnostics.
pub fn discover(paths: &EnvironmentPaths) -> Result<ContentCatalog, String> {
    let source_root = paths.source().root();
    let workspace_version = workspace_version(source_root)?;
    let workspace = WorkspaceManifest::load(paths)?;

    let mut artifacts = Vec::new();
    for project in workspace.projects() {
        let kind = project.kind().content_kind();
        let manifest_path = source_root.join(project.manifest());
        match manifest::read(&manifest_path, source_root)? {
            VaporEntity::Content {
                kind: actual_kind,
                id,
                name,
            } => {
                if actual_kind != kind {
                    return Err(format!(
                        "registered workspace project '{}' changed kind from {kind} to {actual_kind}",
                        project.path().display()
                    ));
                }
                let root = manifest_path
                    .parent()
                    .ok_or_else(|| format!("manifest has no parent: {}", manifest_path.display()))?
                    .to_path_buf();
                let authored = read_authored_content(&manifest_path, kind)?;
                artifacts.push(ContentArtifact {
                    id,
                    name,
                    kind,
                    root,
                    manifest: manifest_path,
                    version: authored
                        .version
                        .resolve(workspace_version.as_ref())
                        .map(|version| version.to_string()),
                    binaries: authored.binaries,
                    libraries: authored.libraries,
                    runtime: Vec::new(),
                    presentation: authored.presentation,
                    dependencies: authored.dependencies,
                    conflicts: authored.conflicts,
                    workshop: authored.workshop,
                });
            }
            VaporEntity::Root { id, .. } | VaporEntity::Workspace { id, .. } => {
                return Err(format!(
                    "registered workspace project '{}' declares nested source root '{id}'",
                    project.path().display()
                ));
            }
            VaporEntity::Registry { id, .. } => {
                return Err(format!(
                    "registered workspace project '{}' declares registry '{id}'",
                    project.path().display()
                ));
            }
        }
    }
    artifacts.sort_by(|left, right| left.id.cmp(&right.id));
    let candidates = source_candidates(&artifacts)?;
    Ok(ContentCatalog {
        artifacts,
        candidates,
    })
}

pub(super) fn source_candidates(
    artifacts: &[ContentArtifact],
) -> Result<ContentCandidates, String> {
    let mut candidates = ContentCandidates::new();
    for artifact in artifacts {
        let Some(version) = artifact.version() else {
            continue;
        };
        let identity = ContentVersionId {
            id: artifact
                .id()
                .parse::<VaporId>()
                .map_err(|error| format!("invalid content ID '{}': {error}", artifact.id()))?,
            version: semver::Version::parse(version).map_err(|error| {
                format!(
                    "invalid content version '{version}' for '{}': {error}",
                    artifact.id()
                )
            })?,
        };
        let mut dependencies = BTreeMap::new();
        for (index, reference) in artifact.dependencies().iter().enumerate() {
            let id = reference
                .id()
                .parse::<VaporId>()
                .map_err(|error| format!("invalid dependency ID '{}': {error}", reference.id()))?;
            let requirement = reference
                .version()
                .unwrap_or("*")
                .parse::<semver::VersionReq>()
                .map_err(|error| {
                    format!(
                        "invalid dependency version requirement '{}' for '{}': {error}",
                        reference.version().unwrap_or("*"),
                        reference.id()
                    )
                })?;
            dependencies.insert(
                format!("{}#{index}", reference.relationship()),
                DependencySpec {
                    id,
                    version: requirement,
                    optional: reference.optional(),
                },
            );
        }
        candidates
            .insert(ContentCandidate {
                identity,
                kind: artifact.kind(),
                dependencies,
            })
            .map_err(|error| error.to_string())?;
    }
    Ok(candidates)
}

/// Create and register a role-specific content project in an authored workspace.
///
/// The generated project uses Vapor's manifest vocabulary and becomes
/// visible to the same catalog used by package, install, and Workshop flows.
pub fn create_source_project(
    paths: &EnvironmentPaths,
    content_id: &str,
    version: &str,
    kind: ContentKind,
    dependency_selectors: &[String],
) -> Result<PathBuf, String> {
    create_source_project_with_workshop(
        paths,
        content_id,
        version,
        kind,
        dependency_selectors,
        &WorkshopPolicyDraft::default(),
    )
}

/// Create a role-specific content project with optional Workshop publication metadata.
pub fn create_source_project_with_workshop(
    paths: &EnvironmentPaths,
    content_id: &str,
    version: &str,
    kind: ContentKind,
    dependency_selectors: &[String],
    workshop: &WorkshopPolicyDraft,
) -> Result<PathBuf, String> {
    create_source_project_with_dependency_policies(
        paths,
        content_id,
        version,
        kind,
        dependency_selectors,
        &[],
        workshop,
    )
}

/// Create a source project with selected optional dependency edges.
pub fn create_source_project_with_dependency_policies(
    paths: &EnvironmentPaths,
    content_id: &str,
    version: &str,
    kind: ContentKind,
    dependency_selectors: &[String],
    optional_dependency_selectors: &[String],
    workshop: &WorkshopPolicyDraft,
) -> Result<PathBuf, String> {
    validate_workshop_policy_draft(workshop)?;
    let workspace_manifest = paths.source().root().join(manifest::WORKSPACE_FILE_NAME);
    let _workspace_lock = acquire_workspace_project_lock(&workspace_manifest)?;
    let workspace = WorkspaceManifest::load(paths)?;
    if workspace.kind() != crate::workspace::SourceRootKind::Workspace {
        return Err("new content can only be added to a Vapor workspace".to_owned());
    }
    let prefix = format!("{}/", workspace.id());
    let name = content_id
        .strip_prefix(&prefix)
        .filter(|name| !name.is_empty() && !name.contains('/'))
        .ok_or_else(|| {
            format!("content ID must start with '{prefix}' and contain one local name")
        })?;
    if !name.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
    }) {
        return Err("content name must use lowercase ASCII letters, digits, '-' or '_'".to_owned());
    }
    let version = semver::Version::parse(version)
        .map_err(|error| format!("invalid content version: {error}"))?;
    if workspace
        .projects()
        .iter()
        .any(|project| project.id() == content_id)
    {
        return Err(format!(
            "content project is already registered: {content_id}"
        ));
    }

    let catalog = discover(paths)?;
    let mut children = Vec::new();
    let optional_selectors = optional_dependency_selectors
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut optional_identities = BTreeSet::new();
    for selector in optional_selectors {
        let child = catalog
            .find(selector)
            .ok_or_else(|| format!("optional content is no longer available: {selector}"))?;
        if source_dependency_relationship(kind, child.kind()) != Some("dependency") {
            return Err(format!(
                "{} is required for the '{}' role and cannot be optional",
                child.id(),
                source_dependency_relationship(kind, child.kind()).unwrap_or("dependency")
            ));
        }
        optional_identities.insert((
            child.id().to_owned(),
            child.version().map(ToOwned::to_owned),
        ));
    }
    for selector in dependency_selectors {
        let child = catalog
            .find(selector)
            .ok_or_else(|| format!("selected content is no longer available: {selector}"))?;
        if !source_dependency_allowed(kind, child.kind()) {
            return Err(format!("{} cannot be included in a {kind}", child.kind()));
        }
        if child.id() == content_id {
            return Err("content cannot depend on itself".to_owned());
        }
        children.push(child);
    }
    if optional_identities.iter().any(|(id, version)| {
        !children.iter().any(|child| {
            child.id() == id && child.version().map(ToOwned::to_owned).as_ref() == version.as_ref()
        })
    }) {
        return Err("optional content must also be selected for this project".to_owned());
    }
    validate_composition_children(kind, &children)?;

    let root = paths.source().root();
    let relative_root = PathBuf::from("content").join(name);
    let output = root.join(&relative_root);
    let original_workspace = fs::read_to_string(&workspace_manifest)
        .map_err(io("read workspace manifest", &workspace_manifest))?;
    let updated_workspace = format!(
        "{}\n\n[[workspace.projects]]\npath = \"content/{name}\"\n",
        original_workspace.trim_end(),
    );
    let cargo_transaction = if crate::source::content_project_output(kind).is_some() {
        let cargo_manifest = root.join("Cargo.toml");
        let original = fs::read_to_string(&cargo_manifest)
            .map_err(io("read Cargo workspace manifest", &cargo_manifest))?;
        let updated = cargo_workspace_with_member(&original, &relative_root)?;
        Some((cargo_manifest, original, updated))
    } else {
        None
    };
    let content_directory = root.join("content");
    let created_content_directory = match fs::symlink_metadata(&content_directory) {
        Ok(metadata) if metadata.file_type().is_dir() => false,
        Ok(_) => {
            return Err(format!(
                "{} is not a regular directory",
                content_directory.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&content_directory)
                .map_err(io("create workspace content directory", &content_directory))?;
            true
        }
        Err(error) => return Err(format!("{}: {error}", content_directory.display())),
    };
    let content_root = match fs::canonicalize(&content_directory) {
        Ok(path) => path,
        Err(error) => {
            if created_content_directory {
                let _ = fs::remove_dir(&content_directory);
            }
            return Err(format!(
                "failed to resolve workspace content directory '{}': {error}",
                content_directory.display()
            ));
        }
    };
    if !content_root.starts_with(root) {
        if created_content_directory {
            let _ = fs::remove_dir(&content_directory);
        }
        return Err(
            "workspace content directory resolves outside the selected workspace".to_owned(),
        );
    }
    if let Err(error) = fs::create_dir(&output) {
        if created_content_directory {
            let _ = fs::remove_dir(&content_directory);
        }
        return Err(io("create content project directory", &output)(error));
    }
    let manifest_path = output.join(kind.manifest_file_name());
    let create_result = (|| -> Result<(), String> {
        let template = crate::source::scaffold_content_project(
            &output,
            content_id,
            &version.to_string(),
            kind,
        )?;
        let body = source_project_manifest(
            name,
            &version.to_string(),
            kind,
            &children,
            &optional_identities,
            &template,
            workshop,
        );
        let mut manifest_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&manifest_path)
            .map_err(io("create content manifest", &manifest_path))?;
        manifest_file
            .write_all(body.as_bytes())
            .map_err(io("write content manifest", &manifest_path))?;
        if matches!(
            &template,
            crate::source::GeneratedContentProject::Rust { .. }
        ) {
            let readme_path = output.join("README.md");
            let mut readme = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&readme_path)
                .map_err(io("create content README", &readme_path))?;
            let role_manifest = manifest_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "content.vapor.toml".to_owned());
            writeln!(
                readme,
                "# {name}\n\nVapor {kind} content `{content_id}`.\n\n- `{role_manifest}` defines this content's identity and dependencies.\n- `Cargo.toml` registers the Rust project in the workspace.\n- `src/` contains generated starting code. Crate projects provide static Rust code; other code roles may declare runtime binaries or shared libraries in their role manifest.\n\nUse Vapor Studio to edit and validate this project, then package or deploy it through Vapor."
            )
            .map_err(io("write content README", &readme_path))?;
        }
        if let Some((cargo_manifest, original_cargo, _)) = &cargo_transaction {
            let current_cargo = fs::read_to_string(cargo_manifest)
                .map_err(io("re-read Cargo workspace manifest", cargo_manifest))?;
            if current_cargo != *original_cargo {
                return Err("Cargo workspace manifest changed while content was being prepared; retry the action".to_owned());
            }
        }
        let current_workspace = fs::read_to_string(&workspace_manifest)
            .map_err(io("re-read Workspace manifest", &workspace_manifest))?;
        if current_workspace != original_workspace {
            return Err(
                "Workspace manifest changed while content was being prepared; retry the action"
                    .to_owned(),
            );
        }
        if let Some((cargo_manifest, original_cargo, updated_cargo)) = &cargo_transaction {
            replace_source_manifest(
                cargo_manifest,
                original_cargo,
                updated_cargo,
                "Cargo workspace membership",
                || {
                    WorkspaceManifest::load(paths)?;
                    vapor_core::VaporWorkspace::load(root).map_err(|error| error.to_string())?;
                    Ok(())
                },
            )?;
        }
        replace_workspace_manifest_locked(
            &workspace_manifest,
            &original_workspace,
            &updated_workspace,
            "content project registration",
            || {
                WorkspaceManifest::load(paths)?;
                vapor_core::VaporWorkspace::load(root).map_err(|error| error.to_string())?;
                discover(paths)?;
                Ok(())
            },
        )?;
        Ok(())
    })();
    if let Err(error) = create_result {
        let mut recovery = Vec::new();
        match fs::read_to_string(&workspace_manifest) {
            Ok(current) if current == updated_workspace => {
                if let Err(restore_error) = replace_workspace_manifest_locked(
                    &workspace_manifest,
                    &updated_workspace,
                    &original_workspace,
                    "content project registration rollback",
                    || Ok(()),
                ) {
                    recovery.push(format!(
                        "could not restore {}: {restore_error}",
                        workspace_manifest.display()
                    ));
                }
            }
            Ok(current) if current == original_workspace => {}
            Ok(_) => recovery.push(format!(
                "{} changed during recovery and was left untouched",
                workspace_manifest.display()
            )),
            Err(error) => recovery.push(format!(
                "could not inspect {} during recovery: {error}",
                workspace_manifest.display()
            )),
        }
        if let Some((cargo_manifest, original_cargo, updated_cargo)) = &cargo_transaction {
            match fs::read_to_string(cargo_manifest) {
                Ok(current) if current == *updated_cargo => {
                    if let Err(restore_error) = replace_source_manifest(
                        cargo_manifest,
                        updated_cargo,
                        original_cargo,
                        "Cargo workspace membership rollback",
                        || Ok(()),
                    ) {
                        recovery.push(format!(
                            "could not restore {}: {restore_error}",
                            cargo_manifest.display()
                        ));
                    }
                }
                Ok(current) if current == *original_cargo => {}
                Ok(_) => recovery.push(format!(
                    "{} changed during recovery and was left untouched",
                    cargo_manifest.display()
                )),
                Err(error) => recovery.push(format!(
                    "could not inspect {} during recovery: {error}",
                    cargo_manifest.display()
                )),
            }
        }
        if let Err(cleanup_error) = fs::remove_dir_all(&output) {
            recovery.push(format!(
                "could not remove incomplete project {}: {cleanup_error}",
                output.display()
            ));
        }
        if created_content_directory
            && let Err(error) = fs::remove_dir(&content_directory)
            && error.kind() != std::io::ErrorKind::DirectoryNotEmpty
            && error.kind() != std::io::ErrorKind::NotFound
        {
            recovery.push(format!(
                "could not remove empty workspace content directory {}: {error}",
                content_directory.display()
            ));
        }
        if recovery.is_empty() {
            return Err(error);
        }
        return Err(format!(
            "{error}; recovery also reported: {}",
            recovery.join("; ")
        ));
    }
    Ok(manifest_path)
}

/// Fork an acquired deployed artifact into a distinct editable project in the
/// selected Vapor Workspace. The package must contain real source whenever
/// its role declares Rust build outputs.
pub fn fork_cached_artifact(
    paths: &EnvironmentPaths,
    selector: &str,
    new_name: &str,
) -> Result<PathBuf, String> {
    if !is_workspace_slug(new_name) {
        return Err("fork name must use lowercase ASCII letters, digits, '-' or '_'".to_owned());
    }
    let workspace_manifest = paths.source().root().join(manifest::WORKSPACE_FILE_NAME);
    let _workspace_lock = acquire_workspace_project_lock(&workspace_manifest)?;
    let workspace = WorkspaceManifest::load(paths)?;
    if workspace.kind() != crate::workspace::SourceRootKind::Workspace {
        return Err("Workshop content can only be forked into a Vapor Workspace".to_owned());
    }

    let cached = cached_artifact(paths.installation(), selector)?;
    let source_id = cached.artifact_id.clone();
    let kind = cached
        .kind
        .parse::<ContentKind>()
        .map_err(|error| format!("cached content has an unsupported type: {error}"))?;
    let new_id = format!("{}/{new_name}", workspace.id());
    if workspace
        .projects()
        .iter()
        .any(|project| project.id() == new_id)
    {
        return Err(format!("content project is already registered: {new_id}"));
    }

    let package_manifest = manifest::content_marker_at(&cached.installed_root)
        .ok_or_else(|| format!("acquired content {source_id} has no role manifest"))?;
    let deployed = fs::read_to_string(&package_manifest)
        .map_err(io("read acquired content manifest", &package_manifest))?;
    let deployed_artifact = read_deployed_manifest(&cached.installed_root)?;
    let fork_manifest = source_manifest_for_fork(&deployed, kind, new_name)?;
    let source_declares_rust = kind == ContentKind::Crate
        || !deployed_artifact.binaries().is_empty()
        || !deployed_artifact.libraries().is_empty();
    let package_cargo = cached.installed_root.join("Cargo.toml");
    let has_cargo = package_cargo.is_file();
    if source_declares_rust && (!has_cargo || !cached.installed_root.join("src").is_dir()) {
        return Err(format!(
            "Workshop item {source_id} contains built output but no complete Rust source project; it cannot be forked as editable content"
        ));
    }
    let old_crate_name = if has_cargo {
        Some(read_package_name(&package_cargo)?)
    } else {
        None
    };
    let new_crate_name = old_crate_name
        .as_ref()
        .map(|_| format!("vapor_{}", new_id.replace(['/', '-'], "_")));

    let source_root = fs::canonicalize(paths.source().root())
        .map_err(io("resolve source workspace", paths.source().root()))?;
    let content_directory = source_root.join("content");
    match fs::symlink_metadata(&content_directory) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(format!(
                "{} is not a regular directory",
                content_directory.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&content_directory)
                .map_err(io("create workspace content directory", &content_directory))?;
        }
        Err(error) => return Err(format!("{}: {error}", content_directory.display())),
    }
    let content_directory = fs::canonicalize(&content_directory).map_err(io(
        "resolve workspace content directory",
        &content_directory,
    ))?;
    if !content_directory.starts_with(&source_root) {
        return Err(
            "workspace content directory resolves outside the selected workspace".to_owned(),
        );
    }
    let relative_project = PathBuf::from("content").join(new_name);
    let output = source_root.join(&relative_project);
    if fs::symlink_metadata(&output).is_ok() {
        return Err(format!(
            "fork destination already exists: {}",
            output.display()
        ));
    }
    fs::create_dir(&output).map_err(io("create fork destination", &output))?;
    let cargo_workspace_path = source_root.join("Cargo.toml");
    let cargo_transaction = if has_cargo {
        let original = fs::read_to_string(&cargo_workspace_path)
            .map_err(io("read Cargo workspace manifest", &cargo_workspace_path))?;
        let updated = cargo_workspace_with_member(&original, &relative_project)?;
        Some((cargo_workspace_path.clone(), original, updated))
    } else {
        None
    };

    let prepare = (|| -> Result<(), String> {
        copy_fork_tree(&cached.installed_root, &output, &cached.installed_root)?;
        let manifest_path = output.join(kind.manifest_file_name());
        fs::write(&manifest_path, fork_manifest)
            .map_err(io("write fork source manifest", &manifest_path))?;
        let _ = fs::remove_file(output.join("Cargo.lock"));
        if let (Some(old_name), Some(new_name)) = (&old_crate_name, &new_crate_name) {
            rewrite_fork_source(&output, &source_id, &new_id, old_name, new_name)?;
        } else {
            rewrite_fork_source(&output, &source_id, &new_id, "", "")?;
        }
        if let Some((cargo_path, original, updated)) = &cargo_transaction {
            replace_source_manifest(
                cargo_path,
                original,
                updated,
                "Cargo workspace membership for content fork",
                || {
                    vapor_core::VaporWorkspace::load(&source_root)
                        .map_err(|error| error.to_string())?;
                    Ok(())
                },
            )?;
        }
        adopt_source_project_locked(paths, &relative_project)?;
        Ok(())
    })();
    if let Err(error) = prepare {
        if let Some((cargo_path, original, updated)) = &cargo_transaction
            && fs::read_to_string(cargo_path).is_ok_and(|current| current == *updated)
        {
            let _ = replace_source_manifest(
                cargo_path,
                updated,
                original,
                "Cargo workspace membership rollback for content fork",
                || Ok(()),
            );
        }
        let _ = fs::remove_dir_all(&output);
        return Err(error);
    }
    Ok(output.join(kind.manifest_file_name()))
}

fn is_workspace_slug(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

fn read_package_name(path: &Path) -> Result<String, String> {
    let source = fs::read_to_string(path).map_err(io("read content Cargo manifest", path))?;
    let document = source
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("content Cargo.toml is invalid: {error}"))?;
    if document.get("workspace").is_some() {
        return Err(
            "nested Cargo workspaces cannot be forked into another Cargo workspace".to_owned(),
        );
    }
    document
        .get("package")
        .and_then(toml_edit::Item::as_table)
        .and_then(|package| package.get("name"))
        .and_then(toml_edit::Item::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| "Rust source project has no [package].name in Cargo.toml".to_owned())
}

fn copy_fork_tree(source: &Path, target: &Path, root: &Path) -> Result<(), String> {
    let relative = source.strip_prefix(root).unwrap_or(Path::new(""));
    if [".git", "target", "output"]
        .iter()
        .any(|name| relative.starts_with(name))
    {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(source).map_err(io("inspect fork source", source))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "source fork refuses symlinks in the acquired package: {}",
            source.display()
        ));
    }
    if metadata.is_dir() {
        fs::create_dir_all(target).map_err(io("create fork folder", target))?;
        for entry in fs::read_dir(source).map_err(io("read fork source", source))? {
            let entry =
                entry.map_err(|error| format!("failed to read fork source entry: {error}"))?;
            copy_fork_tree(&entry.path(), &target.join(entry.file_name()), root)?;
        }
    } else if metadata.is_file() {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io("create fork folder", parent))?;
        }
        fs::copy(source, target).map_err(io("copy fork source file", source))?;
    }
    Ok(())
}

pub(super) fn rewrite_fork_source(
    root: &Path,
    source_id: &str,
    new_id: &str,
    old_crate: &str,
    new_crate: &str,
) -> Result<(), String> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    for relative_path in files {
        let path = root.join(relative_path);
        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        let mut updated = source.replace(source_id, new_id);
        if !old_crate.is_empty() && old_crate != new_crate {
            updated = updated.replace(old_crate, new_crate);
        }
        if updated != source {
            fs::write(&path, updated).map_err(io("rewrite fork source identity", &path))?;
        }
    }
    Ok(())
}

/// Register an existing in-workspace content project without moving its files.
pub fn adopt_source_project(
    paths: &EnvironmentPaths,
    project_path: &Path,
) -> Result<String, String> {
    adopt_source_project_impl(paths, project_path, false)
}

fn adopt_source_project_locked(
    paths: &EnvironmentPaths,
    project_path: &Path,
) -> Result<String, String> {
    adopt_source_project_impl(paths, project_path, true)
}

fn adopt_source_project_impl(
    paths: &EnvironmentPaths,
    project_path: &Path,
    workspace_lock_held: bool,
) -> Result<String, String> {
    let workspace = WorkspaceManifest::load(paths)?;
    if workspace.kind() != crate::workspace::SourceRootKind::Workspace {
        return Err("content projects can only be adopted into a Vapor Workspace".to_owned());
    }
    if project_path.as_os_str().is_empty()
        || project_path.is_absolute()
        || project_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(
            "project path must be a non-empty relative path inside the Workspace".to_owned(),
        );
    }

    let source_root = fs::canonicalize(paths.source().root())
        .map_err(io("resolve source workspace", paths.source().root()))?;
    let requested_root = source_root.join(project_path);
    let metadata = fs::symlink_metadata(&requested_root)
        .map_err(io("inspect project folder", &requested_root))?;
    if !metadata.file_type().is_dir() {
        return Err(format!(
            "project path must name a regular directory: {}",
            project_path.display()
        ));
    }
    let project_root =
        fs::canonicalize(&requested_root).map_err(io("resolve project folder", &requested_root))?;
    if !project_root.starts_with(&source_root) {
        return Err("project folder resolves outside the selected Workspace".to_owned());
    }
    let marker = manifest::content_marker_at(&project_root).ok_or_else(|| {
        format!(
            "no role-specific Vapor content manifest found in {}",
            project_path.display()
        )
    })?;
    let marker_metadata =
        fs::symlink_metadata(&marker).map_err(io("inspect content manifest", &marker))?;
    if !marker_metadata.file_type().is_file() || marker_metadata.file_type().is_symlink() {
        return Err(format!(
            "content manifest must be a regular non-symlink file: {}",
            marker.display()
        ));
    }
    let (id, kind) = match manifest::read(&marker, &source_root)? {
        VaporEntity::Content { id, kind, .. } => (id, kind),
        _ => {
            return Err(format!(
                "{} is not a content project manifest",
                marker.display()
            ));
        }
    };
    if workspace
        .projects()
        .iter()
        .any(|project| project.path() == project_path || project.id() == id)
    {
        return Err(format!("content project is already registered: {id}"));
    }

    let workspace_manifest = source_root.join(manifest::WORKSPACE_FILE_NAME);
    let workspace_metadata = fs::symlink_metadata(&workspace_manifest)
        .map_err(io("inspect Workspace manifest", &workspace_manifest))?;
    if !workspace_metadata.file_type().is_file() || workspace_metadata.file_type().is_symlink() {
        return Err(format!(
            "Workspace manifest must be a regular non-symlink file: {}",
            workspace_manifest.display()
        ));
    }
    let original = fs::read_to_string(&workspace_manifest)
        .map_err(io("read Workspace manifest", &workspace_manifest))?;
    let relative_project = project_root
        .strip_prefix(&source_root)
        .map_err(|error| format!("project folder is outside the Workspace: {error}"))?;
    let relative_project = relative_project
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let mut document = original
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("Workspace manifest is not valid TOML: {error}"))?;
    let workspace_table = document
        .get_mut("workspace")
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| "Workspace manifest has no [workspace] table".to_owned())?;
    match workspace_table.get_mut("projects") {
        Some(toml_edit::Item::ArrayOfTables(projects)) => {
            let mut project = toml_edit::Table::new();
            project.insert("path", toml_edit::value(relative_project));
            projects.push(project);
        }
        Some(_) => return Err("workspace.projects must be an array of tables".to_owned()),
        None => {
            let mut projects = toml_edit::ArrayOfTables::new();
            let mut project = toml_edit::Table::new();
            project.insert("path", toml_edit::value(relative_project));
            projects.push(project);
            workspace_table.insert("projects", toml_edit::Item::ArrayOfTables(projects));
        }
    }

    let updated = document.to_string();
    let register = || {
        WorkspaceManifest::load(paths)?;
        vapor_core::VaporWorkspace::load(&source_root).map_err(|error| error.to_string())?;
        let catalog = discover(paths)?;
        if !catalog
            .artifacts()
            .iter()
            .any(|artifact| artifact.id() == id && artifact.kind() == kind)
        {
            return Err(format!(
                "adopted content project {id} is not visible after registration"
            ));
        }
        Ok(())
    };
    if workspace_lock_held {
        replace_workspace_manifest_locked(
            &workspace_manifest,
            &original,
            &updated,
            "content project registration",
            register,
        )?;
    } else {
        replace_workspace_manifest(
            &workspace_manifest,
            &original,
            &updated,
            "content project registration",
            register,
        )?;
    }
    Ok(format!(
        "Registered {kind} content {id} from {}.",
        project_path.display()
    ))
}

/// Remove a content project from Vapor discovery without touching its files or Cargo membership.
pub fn unregister_source_project(paths: &EnvironmentPaths, id: &str) -> Result<String, String> {
    let workspace = WorkspaceManifest::load(paths)?;
    if workspace.kind() != crate::workspace::SourceRootKind::Workspace {
        return Err("content projects can only be unregistered from a Vapor Workspace".to_owned());
    }
    let project = workspace
        .projects()
        .iter()
        .find(|project| project.id() == id)
        .ok_or_else(|| format!("content project is not registered in this Workspace: {id}"))?;
    let project_path = project.path().to_path_buf();
    let project_kind = project.kind().to_string();
    let source_root = fs::canonicalize(paths.source().root())
        .map_err(io("resolve source workspace", paths.source().root()))?;
    let workspace_manifest = source_root.join(manifest::WORKSPACE_FILE_NAME);
    let metadata = fs::symlink_metadata(&workspace_manifest)
        .map_err(io("inspect Workspace manifest", &workspace_manifest))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "Workspace manifest must be a regular non-symlink file: {}",
            workspace_manifest.display()
        ));
    }
    let original = fs::read_to_string(&workspace_manifest)
        .map_err(io("read Workspace manifest", &workspace_manifest))?;
    let mut document = original
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("Workspace manifest is not valid TOML: {error}"))?;
    let workspace_table = document
        .get_mut("workspace")
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| "Workspace manifest has no [workspace] table".to_owned())?;
    let projects_item = workspace_table
        .get("projects")
        .ok_or_else(|| "Workspace has no registered content projects".to_owned())?;
    let projects = projects_item
        .as_array_of_tables()
        .ok_or_else(|| "workspace.projects must be an array of tables".to_owned())?;
    let mut retained = toml_edit::ArrayOfTables::new();
    let mut removed = 0usize;
    let mut retained_count = 0usize;
    for entry in projects.iter() {
        let matches = entry
            .get("path")
            .and_then(toml_edit::Item::as_str)
            .is_some_and(|path| Path::new(path) == project_path.as_path());
        if matches {
            removed += 1;
        } else {
            retained.push(entry.clone());
            retained_count += 1;
        }
    }
    if removed != 1 {
        return Err(format!(
            "expected one Workspace registration for {id}, found {removed}; no change made"
        ));
    }
    if retained_count == 0 {
        workspace_table.remove("projects");
    } else {
        workspace_table.insert("projects", toml_edit::Item::ArrayOfTables(retained));
    }
    let updated = document.to_string();
    replace_workspace_manifest(
        &workspace_manifest,
        &original,
        &updated,
        "content project unregistration",
        || {
            WorkspaceManifest::load(paths)?;
            vapor_core::VaporWorkspace::load(&source_root).map_err(|error| error.to_string())?;
            let catalog = discover(paths)?;
            if catalog
                .artifacts()
                .iter()
                .any(|artifact| artifact.id() == id)
            {
                return Err(format!(
                    "content project {id} remains visible after unregistration"
                ));
            }
            Ok(())
        },
    )?;
    Ok(format!(
        "Unregistered {project_kind} content {id} from the Vapor project registry. Its files and Cargo membership were left unchanged."
    ))
}

fn replace_workspace_manifest<T>(
    manifest_path: &Path,
    expected_source: &str,
    updated_source: &str,
    action: &str,
    validate: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _lock = acquire_workspace_project_lock(manifest_path)?;
    replace_workspace_manifest_locked(
        manifest_path,
        expected_source,
        updated_source,
        action,
        validate,
    )
}

fn replace_workspace_manifest_locked<T>(
    manifest_path: &Path,
    expected_source: &str,
    updated_source: &str,
    action: &str,
    validate: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let current = fs::read_to_string(manifest_path)
        .map_err(io("re-read Workspace manifest", manifest_path))?;
    if current != expected_source {
        return Err("Workspace manifest changed while the project registry edit was being prepared; retry the action".to_owned());
    }
    let temporary = source_manifest_temp_path(manifest_path);
    if let Err(error) = write_source_manifest_temp(&temporary, updated_source, manifest_path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let backup = source_manifest_temp_path_with_suffix(manifest_path, "backup");
    if let Err(error) = fs::rename(manifest_path, &backup) {
        let _ = fs::remove_file(&temporary);
        return Err(io("prepare Workspace manifest replacement", manifest_path)(
            error,
        ));
    }
    if let Err(error) = fs::rename(&temporary, manifest_path) {
        let restore = fs::rename(&backup, manifest_path);
        let _ = fs::remove_file(&temporary);
        return Err(match restore {
            Ok(()) => io("replace Workspace manifest", manifest_path)(error),
            Err(restore) => {
                format!("failed to apply {action}: {error}; recovery failed: {restore}")
            }
        });
    }
    match validate() {
        Ok(result) => {
            let _ = fs::remove_file(&backup);
            Ok(result)
        }
        Err(error) => {
            let _ = fs::remove_file(manifest_path);
            Err(match fs::rename(&backup, manifest_path) {
                Ok(()) => format!(
                    "{action} failed validation: {error}; original Workspace manifest restored"
                ),
                Err(restore) => {
                    format!("{action} failed validation: {error}; recovery failed: {restore}")
                }
            })
        }
    }
}

fn acquire_workspace_project_lock(manifest_path: &Path) -> Result<SourceManifestEditLock, String> {
    let lock_path = manifest_path.with_file_name(".Workspace.vapor-project.lock");
    let lock_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|error| {
            format!(
                "cannot acquire Workspace edit lock '{}': {error}",
                lock_path.display()
            )
        })?;
    Ok(SourceManifestEditLock {
        path: lock_path,
        file: Some(lock_file),
    })
}

/// Load the editable dependency graph for a locally authored content project.
pub fn load_source_composition(
    paths: &EnvironmentPaths,
    selector: &str,
) -> Result<SourceComposition, String> {
    let (artifact, manifest, source) = load_source_manifest_snapshot(paths, selector)?;
    Ok(SourceComposition {
        id: artifact.id().to_owned(),
        version: artifact.version().map(ToOwned::to_owned),
        kind: artifact.kind().to_string(),
        manifest,
        source,
        links: artifact
            .dependencies()
            .iter()
            .map(CompositionLink::from)
            .collect(),
    })
}

/// Replace a source content project's dependency graph after validating the exact
/// source snapshot and each selected edge against the live workspace catalog.
pub fn save_source_composition(
    paths: &EnvironmentPaths,
    selector: &str,
    expected_source: &str,
    links: &[CompositionLink],
) -> Result<SourceComposition, String> {
    let current = load_source_composition(paths, selector)?;
    if current.source != expected_source {
        return Err(
            "content manifest changed on disk since it was opened; reload it before saving"
                .to_owned(),
        );
    }
    let catalog = discover(paths)?;
    let root = catalog.find(selector).ok_or_else(|| {
        format!("source content is no longer available in this workspace: {selector}")
    })?;
    let root_id = root.id().to_owned();
    let root_kind = root.kind();
    let mut selected = Vec::with_capacity(links.len());
    let mut seen = BTreeSet::new();
    for link in links {
        let selector_key = format!("{}\0{}", link.relationship, link.id);
        if !seen.insert(selector_key) {
            return Err(format!(
                "duplicate composition edge: {} ({})",
                link.id, link.relationship
            ));
        }
        let requirement = link
            .version
            .as_deref()
            .map(semver::VersionReq::parse)
            .transpose()
            .map_err(|error| format!("invalid version requirement for {}: {error}", link.id))?;
        let child = find_local_source(&catalog, &link.id, requirement.as_ref())
            .ok_or_else(|| format!("selected content is no longer available at the requested version in this workspace: {} {}", link.id, link.version.as_deref().unwrap_or("")))?;
        if child.id() == root_id {
            return Err("source content cannot depend on itself".to_owned());
        }
        let expected_relationship = source_dependency_relationship(root_kind, child.kind())
            .ok_or_else(|| format!("{} cannot be included in a {root_kind}", child.kind()))?;
        validate_dependency_edge(root_kind, &link.relationship, child)?;
        let preserves_existing_relationship = expected_relationship == "dependency"
            && current.links.iter().any(|existing| {
                existing.id == link.id && existing.relationship == link.relationship
            });
        if link.relationship != expected_relationship && !preserves_existing_relationship {
            return Err(format!(
                "{} must use the '{}' relationship, not '{}'",
                child.id(),
                expected_relationship,
                link.relationship
            ));
        }
        if link.optional && matches!(link.relationship.as_str(), "engine" | "enginepack") {
            return Err(format!(
                "{} cannot mark its required Engine relationship optional",
                root_kind
            ));
        }
        if link.optional && root_kind == ContentKind::Gamepack && link.relationship == "game" {
            return Err(
                "a Gamepack cannot mark its required Game relationship optional".to_owned(),
            );
        }
        selected.push(child);
    }
    validate_composition_children(root_kind, &selected)?;
    validate_local_source_graph(&catalog, &root_id, &selected)?;

    let mut document = expected_source
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("content manifest is not valid TOML: {error}"))?;
    let role = root_kind.to_string();
    let role_table = document
        .get_mut(role.as_str())
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| format!("source manifest has no [{role}] table"))?;
    let mut existing_direct_edges = BTreeMap::new();
    for relationship in ["engine", "enginepack", "game", "gamepack"] {
        if let Some(toml_edit::Item::Table(table)) = role_table.remove(relationship) {
            existing_direct_edges.insert(relationship.to_owned(), table);
        }
    }
    let mut existing_dependencies = BTreeMap::new();
    if let Some(toml_edit::Item::ArrayOfTables(dependencies)) = role_table.remove("dependencies") {
        for table in dependencies.iter() {
            let Some(id) = table.get("id").and_then(toml_edit::Item::as_str) else {
                continue;
            };
            let relationship = table
                .get("relationship")
                .and_then(toml_edit::Item::as_str)
                .unwrap_or("dependency");
            existing_dependencies.insert((id.to_owned(), relationship.to_owned()), table.clone());
        }
    }
    let mut dependencies = toml_edit::ArrayOfTables::new();
    for (link, child) in links.iter().zip(&selected) {
        let expected_relationship = source_dependency_relationship(root_kind, child.kind())
            .ok_or_else(|| format!("{} cannot be included in a {root_kind}", child.kind()))?;
        if expected_relationship != "dependency" {
            let mut reference = existing_direct_edges
                .remove(&link.relationship)
                .unwrap_or_else(toml_edit::Table::new);
            update_composition_link_fields(&mut reference, link, false);
            role_table.insert(
                link.relationship.as_str(),
                toml_edit::Item::Table(reference),
            );
        } else {
            let mut reference = existing_dependencies
                .remove(&(link.id.clone(), link.relationship.clone()))
                .unwrap_or_else(toml_edit::Table::new);
            update_composition_link_fields(&mut reference, link, true);
            dependencies.push(reference);
        }
    }
    if !dependencies.is_empty() {
        role_table.insert("dependencies", toml_edit::Item::ArrayOfTables(dependencies));
    }
    let updated = document.to_string();
    replace_source_manifest(
        &current.manifest,
        expected_source,
        &updated,
        "content graph",
        || load_source_composition(paths, selector),
    )
}

/// Load the authored Workshop policy and exact source snapshot for a content project.
pub fn load_source_workshop_metadata(
    paths: &EnvironmentPaths,
    selector: &str,
) -> Result<SourceWorkshopMetadata, String> {
    let (artifact, manifest, source) = load_source_manifest_snapshot(paths, selector)?;
    let policy = artifact.workshop();
    Ok(SourceWorkshopMetadata {
        id: artifact.id().to_owned(),
        version: artifact.version().map(ToOwned::to_owned),
        kind: artifact.kind().to_string(),
        manifest,
        source,
        policy: WorkshopPolicyDraft {
            app_id: policy.app_id(),
            published_file_id: policy.published_file_id().map(ToOwned::to_owned),
            visibility: policy.visibility().map(ToOwned::to_owned),
            title: policy.title().map(ToOwned::to_owned),
            description: policy.description().map(ToOwned::to_owned),
            tags: policy.tags().to_vec(),
            change_note: policy.change_note().map(ToOwned::to_owned),
        },
    })
}

/// Load the typed runtime output declarations and exact source snapshot.
pub fn load_source_runtime_outputs(
    paths: &EnvironmentPaths,
    selector: &str,
) -> Result<SourceRuntimeOutputs, String> {
    let (artifact, manifest, source) = load_source_manifest_snapshot(paths, selector)?;
    Ok(SourceRuntimeOutputs {
        id: artifact.id().to_owned(),
        version: artifact.version().map(ToOwned::to_owned),
        kind: artifact.kind().to_string(),
        manifest,
        source,
        binaries: artifact.binaries().to_vec(),
        libraries: artifact.libraries().to_vec(),
    })
}

/// Save runtime output declarations through the shared guarded manifest writer.
pub fn save_source_runtime_outputs(
    paths: &EnvironmentPaths,
    selector: &str,
    expected_source: &str,
    binaries: &[String],
    libraries: &[String],
) -> Result<SourceRuntimeOutputs, String> {
    let current = load_source_runtime_outputs(paths, selector)?;
    if current.source != expected_source {
        return Err(
            "content manifest changed on disk since it was opened; reload before saving".to_owned(),
        );
    }
    validate_runtime_output_names("binaries", binaries)?;
    validate_runtime_output_names("libraries", libraries)?;
    let mut document = expected_source
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("content manifest is not valid TOML: {error}"))?;
    let role = &current.kind;
    let role_table = document
        .get_mut(role.as_str())
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| format!("content manifest has no [{role}] table"))?;
    set_string_array(role_table, "binaries", binaries);
    set_string_array(role_table, "libraries", libraries);
    let updated = document.to_string();
    replace_source_manifest(
        &current.manifest,
        expected_source,
        &updated,
        "runtime output declarations",
        || load_source_runtime_outputs(paths, selector),
    )
}

fn load_source_manifest_snapshot(
    paths: &EnvironmentPaths,
    selector: &str,
) -> Result<(ContentArtifact, PathBuf, String), String> {
    let catalog = discover(paths)?;
    let artifact = catalog.find(selector).cloned().ok_or_else(|| {
        format!("source content is no longer available in this workspace: {selector}")
    })?;
    let root = fs::canonicalize(paths.source().root())
        .map_err(io("resolve source workspace", paths.source().root()))?;
    let metadata = fs::symlink_metadata(artifact.manifest())
        .map_err(io("inspect content manifest", artifact.manifest()))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "content manifest must be a regular file: {}",
            artifact.manifest().display()
        ));
    }
    let manifest = fs::canonicalize(artifact.manifest())
        .map_err(io("resolve content manifest", artifact.manifest()))?;
    if !manifest.starts_with(&root) {
        return Err("content manifest resolves outside the selected workspace".to_owned());
    }
    let source = fs::read_to_string(&manifest).map_err(io("read content manifest", &manifest))?;
    Ok((artifact, manifest, source))
}

fn replace_source_manifest<T>(
    manifest: &Path,
    expected_source: &str,
    updated_source: &str,
    subject: &str,
    reopen: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let lock_path = manifest.with_extension("vapor-edit.lock");
    let lock_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|error| {
            format!(
                "cannot acquire content manifest edit lock '{}': {error}",
                lock_path.display()
            )
        })?;
    let _lock = SourceManifestEditLock {
        path: lock_path,
        file: Some(lock_file),
    };
    let locked_source =
        fs::read_to_string(manifest).map_err(io("re-read content manifest", manifest))?;
    if locked_source != expected_source {
        return Err(
            "content manifest changed while the edit was being prepared; reload before saving"
                .to_owned(),
        );
    }
    let temporary = source_manifest_temp_path(manifest);
    if let Err(error) = write_source_manifest_temp(&temporary, updated_source, manifest) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let backup = source_manifest_temp_path_with_suffix(manifest, "backup");
    if let Err(error) = fs::rename(manifest, &backup) {
        let _ = fs::remove_file(&temporary);
        return Err(io("prepare content manifest replacement", manifest)(error));
    }
    if let Err(error) = fs::rename(&temporary, manifest) {
        let restore = fs::rename(&backup, manifest);
        let _ = fs::remove_file(&temporary);
        return Err(match restore {
            Ok(()) => io("replace content manifest", manifest)(error),
            Err(restore) => {
                format!("failed to replace content manifest: {error}; recovery failed: {restore}")
            }
        });
    }
    let reopened = reopen()
        .map_err(|error| format!("updated {subject} could not be discovered or reopened: {error}"));
    match reopened {
        Ok(value) => {
            let _ = fs::remove_file(&backup);
            Ok(value)
        }
        Err(error) => {
            let _ = fs::remove_file(manifest);
            Err(match fs::rename(&backup, manifest) {
                Ok(()) => format!(
                    "updated {subject} could not be reopened: {error}; original manifest restored"
                ),
                Err(restore) => format!(
                    "updated {subject} could not be reopened: {error}; recovery failed: {restore}"
                ),
            })
        }
    }
}

fn set_string_array(table: &mut toml_edit::Table, key: &str, values: &[String]) {
    if values.is_empty() {
        table.remove(key);
        return;
    }
    let mut array = toml_edit::Array::new();
    for value in values {
        array.push(value.as_str());
    }
    table.insert(key, toml_edit::Item::Value(toml_edit::Value::Array(array)));
}

/// Save Workshop policy through a stale-snapshot-checked, atomic manifest replacement.
pub fn save_source_workshop_metadata(
    paths: &EnvironmentPaths,
    selector: &str,
    expected_source: &str,
    policy: &WorkshopPolicyDraft,
) -> Result<SourceWorkshopMetadata, String> {
    let current = load_source_workshop_metadata(paths, selector)?;
    if current.source != expected_source {
        return Err(
            "content manifest changed on disk since it was opened; reload before saving".to_owned(),
        );
    }
    validate_workshop_policy_draft(policy)?;
    let mut document = expected_source
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("content manifest is not valid TOML: {error}"))?;
    let role = &current.kind;
    let role_table = document
        .get_mut(role.as_str())
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| format!("content manifest has no [{role}] table"))?;
    if role_table.get("steam").is_none() {
        role_table.insert("steam", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let steam_item = role_table
        .get_mut("steam")
        .ok_or_else(|| format!("[{role}.steam] could not be created"))?;
    let steam = steam_item
        .as_table_mut()
        .ok_or_else(|| format!("[{role}.steam] must be a table"))?;
    set_optional_toml_value(
        steam,
        "app-id",
        policy
            .app_id
            .map(|app_id| toml_edit::value(i64::from(app_id))),
    );
    set_optional_toml_value(
        steam,
        "published-file-id",
        policy.published_file_id.as_deref().map(toml_edit::value),
    );
    set_optional_toml_value(
        steam,
        "visibility",
        policy.visibility.as_deref().map(toml_edit::value),
    );
    set_optional_toml_value(
        steam,
        "title",
        policy.title.as_deref().map(toml_edit::value),
    );
    set_optional_toml_value(
        steam,
        "description",
        policy.description.as_deref().map(toml_edit::value),
    );
    let tags = if policy.tags.is_empty() {
        None
    } else {
        let mut tags = toml_edit::Array::new();
        for tag in &policy.tags {
            tags.push(tag.as_str());
        }
        Some(toml_edit::Item::Value(toml_edit::Value::Array(tags)))
    };
    set_optional_toml_value(steam, "tags", tags);
    set_optional_toml_value(
        steam,
        "change-note",
        policy.change_note.as_deref().map(toml_edit::value),
    );
    let remove_steam_table = steam.is_empty();
    if remove_steam_table {
        role_table.remove("steam");
    }
    replace_source_manifest(
        &current.manifest,
        expected_source,
        &document.to_string(),
        "Workshop metadata",
        || load_source_workshop_metadata(paths, selector),
    )
}

fn set_optional_toml_value(
    table: &mut toml_edit::Table,
    key: &str,
    value: Option<toml_edit::Item>,
) {
    if let Some(value) = value {
        table.insert(key, value);
    } else {
        table.remove(key);
    }
}

fn update_composition_link_fields(
    table: &mut toml_edit::Table,
    link: &CompositionLink,
    write_relationship: bool,
) {
    table.insert("id", toml_edit::value(link.id.clone()));
    set_optional_toml_value(
        table,
        "version",
        link.version.as_deref().map(toml_edit::value),
    );
    set_optional_toml_value(
        table,
        "optional",
        link.optional.then(|| toml_edit::value(true)),
    );
    set_optional_toml_value(
        table,
        "workshop_id",
        link.workshop_id.as_deref().map(toml_edit::value),
    );
    set_optional_toml_value(
        table,
        "reason",
        link.reason.as_deref().map(toml_edit::value),
    );
    if write_relationship {
        set_optional_toml_value(
            table,
            "relationship",
            (link.relationship != "dependency")
                .then(|| toml_edit::value(link.relationship.clone())),
        );
    }
}

fn validate_workshop_policy_draft(policy: &WorkshopPolicyDraft) -> Result<(), String> {
    if policy.app_id.is_some_and(|app_id| app_id == 0) {
        return Err("Steam AppID must be positive".to_owned());
    }
    if policy
        .published_file_id
        .as_deref()
        .is_some_and(|id| id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err("PublishedFileId must contain only decimal digits".to_owned());
    }
    Ok(())
}

pub(super) fn find_local_source<'a>(
    catalog: &'a ContentCatalog,
    id: &str,
    requirement: Option<&semver::VersionReq>,
) -> Option<&'a ContentArtifact> {
    catalog.artifacts().iter().find(|artifact| {
        artifact.id() == id
            && requirement.is_none_or(|requirement| {
                artifact
                    .version()
                    .and_then(|version| semver::Version::parse(version).ok())
                    .is_some_and(|version| requirement.matches(&version))
            })
    })
}

fn validate_local_source_graph(
    catalog: &ContentCatalog,
    root_id: &str,
    roots: &[&ContentArtifact],
) -> Result<(), String> {
    fn visit(
        catalog: &ContentCatalog,
        root_id: &str,
        artifact: &ContentArtifact,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) -> Result<(), String> {
        let id = artifact.id();
        if id == root_id {
            return Err(format!(
                "composition graph would cycle back to root content {root_id}"
            ));
        }
        if visited.contains(id) {
            return Ok(());
        }
        if !visiting.insert(id.to_owned()) {
            return Err(format!("source content dependency cycle reaches {id}"));
        }
        for dependency in artifact.dependencies() {
            let requirement = dependency
                .version()
                .map(semver::VersionReq::parse)
                .transpose()
                .map_err(|error| {
                    format!(
                        "{} has an invalid version requirement for {}: {error}",
                        artifact.id(),
                        dependency.id()
                    )
                })?;
            let child = find_local_source(catalog, dependency.id(), requirement.as_ref());
            let Some(child) = child else {
                if dependency.optional() {
                    // An absent optional node is not in this resolved local graph.
                    continue;
                }
                return Err(format!(
                    "local composition rule: {} requires {} {}, but no matching version is authored in this workspace",
                    artifact.id(),
                    dependency.id(),
                    dependency.version().unwrap_or_default()
                ));
            };
            // Traverse optional nodes too when they are locally available: their
            // own required descendants and cycles are still part of this graph.
            visit(catalog, root_id, child, visiting, visited)?;
        }
        visiting.remove(id);
        visited.insert(id.to_owned());
        Ok(())
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for root in roots {
        visit(catalog, root_id, root, &mut visiting, &mut visited)?;
    }
    Ok(())
}

struct SourceManifestEditLock {
    path: PathBuf,
    file: Option<fs::File>,
}
impl Drop for SourceManifestEditLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

fn source_manifest_temp_path(manifest: &Path) -> PathBuf {
    source_manifest_temp_path_with_suffix(manifest, "tmp")
}
fn source_manifest_temp_path_with_suffix(manifest: &Path, suffix: &str) -> PathBuf {
    let name = manifest
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("manifest"));
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    manifest.with_file_name(format!(
        ".{}.{}.{nonce}.{suffix}",
        name.to_string_lossy(),
        std::process::id()
    ))
}
fn write_source_manifest_temp(path: &Path, contents: &str, original: &Path) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io("create temporary source manifest", path))?;
    file.write_all(contents.as_bytes())
        .map_err(io("write temporary source manifest", path))?;
    file.sync_all()
        .map_err(io("sync temporary source manifest", path))?;
    if let Ok(metadata) = fs::metadata(original) {
        fs::set_permissions(path, metadata.permissions())
            .map_err(io("preserve source manifest permissions", path))?;
    }
    Ok(())
}

/// Whether a source content role can directly depend on another role.
///
/// Packs expose their documented member kinds, while non-pack content may
/// compose other capabilities through ordinary dependencies.
pub fn source_dependency_allowed(parent: ContentKind, child: ContentKind) -> bool {
    vapor_content::dependency_kind_allowed(parent, child)
}

pub(super) fn validate_artifact_dependency_roles(artifact: &ContentArtifact) -> Result<(), String> {
    let relationships = artifact
        .dependencies()
        .iter()
        .map(|dependency| dependency.relationship().to_owned())
        .collect::<Vec<_>>();
    validate_source_relationship_cardinality(artifact.kind(), &relationships)
        .map_err(|error| format!("{}: {error}", artifact.id()))?;
    for dependency in artifact
        .dependencies()
        .iter()
        .filter(|edge| edge.optional())
    {
        if matches!(
            (artifact.kind(), dependency.relationship()),
            (
                ContentKind::Packagepack,
                "engine" | "enginepack" | "game" | "gamepack"
            ) | (ContentKind::Game, "engine")
                | (ContentKind::Enginepack, "engine")
                | (ContentKind::Gamepack, "game")
        ) {
            return Err(format!(
                "{} cannot mark its required '{}' relationship optional",
                artifact.id(),
                dependency.relationship()
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_dependency_edge(
    parent: ContentKind,
    relationship: &str,
    child: &ContentArtifact,
) -> Result<(), String> {
    if !source_dependency_allowed(parent, child.kind()) {
        return Err(format!(
            "{parent} cannot depend on {} content {}",
            child.kind(),
            child.id()
        ));
    }
    if let Some(expected) = source_dependency_relationship(parent, child.kind())
        && expected != "dependency"
        && relationship != expected
    {
        return Err(format!(
            "{} dependency '{}' must use the '{}' relationship, not '{}'",
            parent,
            child.id(),
            expected,
            relationship
        ));
    }
    if source_dependency_relationship(parent, child.kind()) == Some("dependency")
        && matches!(relationship, "engine" | "enginepack" | "game" | "gamepack")
    {
        return Err(format!(
            "{} content {} cannot use the '{}' composition role; use 'dependency'",
            child.kind(),
            child.id(),
            relationship
        ));
    }
    Ok(())
}

/// Return the authored edge name for a supported parent/child role pair.
pub fn source_dependency_relationship(
    parent: ContentKind,
    child: ContentKind,
) -> Option<&'static str> {
    vapor_content::dependency_relationship(parent, child)
}

/// Return the exclusive authored role slot for dependency choices with bounded
/// cardinality. Dependencies outside these role slots remain freely composable.
pub fn source_dependency_choice_group(
    parent: ContentKind,
    child: ContentKind,
) -> Option<&'static str> {
    vapor_content::dependency_choice_group(parent, child)
}

/// Return every exclusive role slot that must be populated before this content
/// kind can form a valid project.
pub fn source_required_dependency_choice_groups(parent: ContentKind) -> &'static [&'static str] {
    vapor_content::required_dependency_choice_groups(parent)
}

/// Create the maintained Engine/Game/Packagepack starter workspace at a new
/// or empty destination, using the installed app's Steam AppID.
pub fn create_basic_content_workspace(
    installation: &InstallationPaths,
    path: PathBuf,
    organization: &str,
    name: &str,
) -> Result<PathBuf, String> {
    initialize_basic_source_workspace(installation, path, organization, name, None)
        .map(|report| report.root)
}

/// Initialize the Engine, Game, and Packagepack starter at an exact path.
///
/// An explicit AppID is preserved for developer-authored templates; otherwise
/// the installed Vapor application's Steam AppID is used.
pub fn initialize_basic_source_workspace(
    installation: &InstallationPaths,
    path: PathBuf,
    organization: &str,
    name: &str,
    app_id: Option<u32>,
) -> Result<BasicSourceWorkspaceReport, String> {
    let app_id = match app_id {
        Some(app_id) => app_id,
        None => crate::source::installation_app_id(installation)?,
    };
    let report = crate::source::init_basic_content(crate::source::BasicContentInit {
        path,
        organization: organization.to_owned(),
        name: name.to_owned(),
        app_id,
    })?;
    Ok(BasicSourceWorkspaceReport {
        root: report.root().to_path_buf(),
        workspace_id: report.workspace_id().to_owned(),
        packagepack_id: report.packagepack_id().to_owned(),
    })
}

/// Find missing Workshop IDs in authored dependency declarations and optionally write them.
pub fn repair_source_metadata(
    paths: &EnvironmentPaths,
    write_changes: bool,
) -> Result<SourceMetadataRepairReport, String> {
    let catalog = discover(paths)?;
    let workshop_ids = catalog
        .artifacts()
        .iter()
        .filter_map(|artifact| {
            artifact
                .workshop()
                .published_file_id()
                .map(|id| (artifact.id().to_owned(), id.to_owned()))
        })
        .collect::<BTreeMap<_, _>>();

    let mut actions = Vec::new();
    for artifact in catalog.artifacts() {
        let mut source = fs::read_to_string(artifact.manifest()).map_err(|error| {
            format!(
                "failed to read '{}': {error}",
                artifact.manifest().display()
            )
        })?;
        let mut changed = false;
        for dependency in artifact.dependencies() {
            let Some(workshop_id) = workshop_ids.get(dependency.id()) else {
                continue;
            };
            let Some(section) = source_dependency_section(artifact.kind(), dependency) else {
                continue;
            };
            let (updated, repaired) =
                repair_dependency_workshop_id(&source, &section, dependency.id(), workshop_id)?;
            if repaired {
                source = updated;
                changed = true;
                actions.push(SourceMetadataRepair {
                    manifest: artifact.manifest().to_path_buf(),
                    section,
                    reference_id: dependency.id().to_owned(),
                    workshop_id: workshop_id.clone(),
                });
            }
        }
        if changed && write_changes {
            fs::write(artifact.manifest(), source).map_err(|error| {
                format!(
                    "failed to write '{}': {error}",
                    artifact.manifest().display()
                )
            })?;
        }
    }

    Ok(SourceMetadataRepairReport { actions })
}

fn source_dependency_section(kind: ContentKind, reference: &ContentReference) -> Option<String> {
    match (kind, reference.relationship()) {
        (ContentKind::Game, "engine") => Some("game.engine".to_owned()),
        (ContentKind::Packagepack, "engine") => Some("packagepack.engine".to_owned()),
        (ContentKind::Packagepack, "game") => Some("packagepack.game".to_owned()),
        _ => None,
    }
}

fn repair_dependency_workshop_id(
    source: &str,
    section: &str,
    reference_id: &str,
    workshop_id: &str,
) -> Result<(String, bool), String> {
    let lines = source.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let mut section_start = None;
    let mut section_end = lines.len();
    let header = format!("[{section}]");

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if section_start.is_some() {
                section_end = index;
                break;
            }
            if trimmed == header {
                section_start = Some(index);
            }
        }
    }

    let Some(start) = section_start else {
        return Ok((source.to_owned(), false));
    };
    let mut id_line = None;
    let mut existing_workshop_line = None;
    for (index, line) in lines.iter().enumerate().take(section_end).skip(start + 1) {
        if let Some((key, value)) = line.trim().split_once('=') {
            match key.trim() {
                "id" if unquote(value.trim()) == Some(reference_id) => id_line = Some(index),
                "workshop-id" => existing_workshop_line = Some(index),
                _ => {}
            }
        }
    }
    let Some(id_index) = id_line else {
        return Ok((source.to_owned(), false));
    };

    let mut updated = lines;
    let id_indent =
        &updated[id_index][..updated[id_index].len() - updated[id_index].trim_start().len()];
    let replacement = format!("{id_indent}workshop-id = \"{workshop_id}\"");
    if let Some(index) = existing_workshop_line {
        let existing_value = updated[index]
            .split_once('#')
            .map_or(updated[index].as_str(), |(value, _)| value)
            .trim();
        if existing_value == replacement.trim() {
            return Ok((source.to_owned(), false));
        }
        let comment = updated[index]
            .find('#')
            .map(|comment_start| updated[index][comment_start..].to_owned());
        updated[index] = comment.map_or(replacement.clone(), |comment| {
            format!("{replacement} {comment}")
        });
    } else {
        updated.insert(id_index + 1, replacement);
    }
    let mut text = updated.join("\n");
    if source.ends_with('\n') {
        text.push('\n');
    }
    toml::from_str::<toml::Value>(&text)
        .map_err(|error| format!("source metadata repair produced invalid TOML: {error}"))?;
    Ok((text, true))
}

fn unquote(value: &str) -> Option<&str> {
    value.strip_prefix('\"')?.strip_suffix('\"')
}

fn source_relationship_choice_group(kind: ContentKind, relationship: &str) -> Option<&'static str> {
    vapor_content::dependency_choice_group_for_relationship(kind, relationship)
}

fn validate_composition_children(
    kind: ContentKind,
    children: &[&ContentArtifact],
) -> Result<(), String> {
    let relationships = children
        .iter()
        .filter_map(|child| source_dependency_relationship(kind, child.kind()))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    validate_source_relationship_cardinality(kind, &relationships)
}

pub(super) fn validate_source_relationship_cardinality(
    kind: ContentKind,
    relationships: &[String],
) -> Result<(), String> {
    let count_group = |group: &str| {
        relationships
            .iter()
            .filter(|relationship| {
                source_relationship_choice_group(kind, relationship.as_str()) == Some(group)
            })
            .count()
    };
    for required_group in source_required_dependency_choice_groups(kind) {
        if count_group(required_group) != 1 {
            let message = match *required_group {
                "engine-choice" => {
                    "a Packagepack needs exactly one Engine or Enginepack dependency"
                }
                "game-choice" => "a Packagepack needs exactly one Game or Gamepack dependency",
                "engine" if kind == ContentKind::Game => {
                    "a Game needs exactly one Engine dependency"
                }
                "engine" => "an Enginepack needs exactly one Engine",
                "game" => "a Gamepack needs exactly one Game",
                _ => "content is missing a required role dependency",
            };
            return Err(message.to_owned());
        }
    }
    Ok(())
}

fn cargo_workspace_with_member(source: &str, member: &Path) -> Result<String, String> {
    let member = member.to_string_lossy().replace('\\', "/");
    let mut document = source
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("Cargo.toml is not valid TOML: {error}"))?;
    if document.get("workspace").is_none() {
        document.insert("workspace", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let workspace = document
        .get_mut("workspace")
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| {
            "Cargo.toml [workspace] must be a table before content can be added".to_owned()
        })?;

    let excluded = workspace
        .get("exclude")
        .and_then(toml_edit::Item::as_array)
        .is_some_and(|patterns| {
            patterns
                .iter()
                .filter_map(toml_edit::Value::as_str)
                .any(|pattern| cargo_member_pattern_matches(pattern, &member))
        });
    if excluded {
        return Err(format!(
            "Cargo.toml excludes '{member}' from the workspace; remove that exclusion before creating the content project"
        ));
    }
    if workspace.get("members").is_none() {
        workspace.insert(
            "members",
            toml_edit::Item::Value(toml_edit::Value::Array(toml_edit::Array::new())),
        );
    }
    let members = workspace
        .get_mut("members")
        .and_then(toml_edit::Item::as_array_mut)
        .ok_or_else(|| "Cargo.toml [workspace].members must be an array".to_owned())?;
    let covered = members
        .iter()
        .filter_map(toml_edit::Value::as_str)
        .any(|pattern| cargo_member_pattern_matches(pattern, &member));
    if !covered {
        members.push(member);
    }
    Ok(document.to_string())
}

fn cargo_member_pattern_matches(pattern: &str, member: &str) -> bool {
    fn segment_matches(pattern: &str, value: &str) -> bool {
        let pattern = pattern.chars().collect::<Vec<_>>();
        let value = value.chars().collect::<Vec<_>>();
        let (mut pattern_index, mut value_index) = (0, 0);
        let mut last_star = None;
        let mut star_value_index = 0;
        while value_index < value.len() {
            if pattern_index < pattern.len()
                && (pattern[pattern_index] == '?' || pattern[pattern_index] == value[value_index])
            {
                pattern_index += 1;
                value_index += 1;
            } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
                last_star = Some(pattern_index);
                pattern_index += 1;
                star_value_index = value_index;
            } else if let Some(star) = last_star {
                pattern_index = star + 1;
                star_value_index += 1;
                value_index = star_value_index;
            } else {
                return false;
            }
        }
        while pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            pattern_index += 1;
        }
        pattern_index == pattern.len()
    }
    fn match_segments(
        patterns: &[&str],
        values: &[&str],
        pattern_index: usize,
        value_index: usize,
    ) -> bool {
        if pattern_index == patterns.len() {
            return value_index == values.len();
        }
        if patterns[pattern_index] == "**" {
            return (value_index..=values.len())
                .any(|next| match_segments(patterns, values, pattern_index + 1, next));
        }
        value_index < values.len()
            && segment_matches(patterns[pattern_index], values[value_index])
            && match_segments(patterns, values, pattern_index + 1, value_index + 1)
    }
    let patterns = pattern.split('/').collect::<Vec<_>>();
    let values = member.split('/').collect::<Vec<_>>();
    match_segments(&patterns, &values, 0, 0)
}

fn source_project_manifest(
    name: &str,
    version: &str,
    kind: ContentKind,
    children: &[&ContentArtifact],
    optional_identities: &BTreeSet<(String, Option<String>)>,
    template: &crate::source::GeneratedContentProject,
    workshop: &WorkshopPolicyDraft,
) -> String {
    let mut output =
        format!("schema = 1\n\n[{kind}]\nname = \"{name}\"\nversion = \"{version}\"\n");
    match template {
        crate::source::GeneratedContentProject::Rust {
            crate_name,
            output: output_kind,
        } => match output_kind {
            crate::source::GeneratedOutputKind::Binary => {
                output.push_str(&format!("binaries = [\"{crate_name}\"]\n"));
            }
            crate::source::GeneratedOutputKind::RuntimeLibrary => {
                output.push_str(&format!("libraries = [\"{crate_name}\"]\n"));
            }
            crate::source::GeneratedOutputKind::StaticCrate => {}
        },
        crate::source::GeneratedContentProject::MetadataOnly => {}
    }
    let mut used = BTreeSet::new();
    for child in children {
        let edge = match (kind, child.kind()) {
            (ContentKind::Packagepack, ContentKind::Engine) => Some("engine"),
            (ContentKind::Packagepack, ContentKind::Enginepack) => Some("enginepack"),
            (ContentKind::Packagepack, ContentKind::Game) => Some("game"),
            (ContentKind::Packagepack, ContentKind::Gamepack) => Some("gamepack"),
            (ContentKind::Game, ContentKind::Engine) => Some("engine"),
            (ContentKind::Enginepack, ContentKind::Engine) => Some("engine"),
            (ContentKind::Gamepack, ContentKind::Game) => Some("game"),
            _ => None,
        };
        if let Some(edge) = edge {
            if used.insert(edge) {
                output.push_str(&format!("\n[{kind}.{edge}]\nid = \"{}\"\n", child.id()));
                if let Some(version) = child.version() {
                    output.push_str(&format!("version = \"={version}\"\n"));
                }
            }
        } else {
            output.push_str(&format!(
                "\n[[{kind}.dependencies]]\nid = \"{}\"\n",
                child.id()
            ));
            if let Some(version) = child.version() {
                output.push_str(&format!("version = \"={version}\"\n"));
            }
            if optional_identities.contains(&(
                child.id().to_owned(),
                child.version().map(ToOwned::to_owned),
            )) {
                output.push_str("optional = true\n");
            }
        }
    }
    if !workshop.is_empty() {
        output.push_str(&format!("\n[{kind}.steam]\n"));
        if let Some(app_id) = workshop.app_id {
            output.push_str(&format!("app-id = {app_id}\n"));
        }
        if let Some(published_file_id) = &workshop.published_file_id {
            output.push_str(&format!(
                "published-file-id = {}\n",
                toml_edit::value(published_file_id)
            ));
        }
        if let Some(visibility) = &workshop.visibility {
            output.push_str(&format!("visibility = {}\n", toml_edit::value(visibility)));
        }
        if let Some(title) = &workshop.title {
            output.push_str(&format!("title = {}\n", toml_edit::value(title)));
        }
        if let Some(description) = &workshop.description {
            output.push_str(&format!(
                "description = {}\n",
                toml_edit::value(description)
            ));
        }
        if !workshop.tags.is_empty() {
            let mut tags = toml_edit::Array::new();
            for tag in &workshop.tags {
                tags.push(tag.as_str());
            }
            output.push_str(&format!("tags = {tags}\n"));
        }
        if let Some(change_note) = &workshop.change_note {
            output.push_str(&format!(
                "change-note = {}\n",
                toml_edit::value(change_note)
            ));
        }
    }
    output
}

/// Turn one deployed role manifest back into a source-authored manifest for a
/// new local identity. Runtime payloads describe a built artifact and are not
/// source declarations; a published-file ID belongs to the publisher's item.
pub(super) fn source_manifest_for_fork(
    deployed: &str,
    kind: ContentKind,
    name: &str,
) -> Result<String, String> {
    let mut document = deployed
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("deployed content manifest is not valid TOML: {error}"))?;
    document.remove("schema");
    let role = document
        .get_mut(kind.as_str())
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| format!("deployed manifest has no [{}] table", kind.as_str()))?;
    role.remove("id");
    role.remove("runtime");
    role.insert("name", toml_edit::value(name));
    if let Some(steam) = role
        .get_mut("steam")
        .and_then(toml_edit::Item::as_table_mut)
    {
        steam.remove("published-file-id");
    }
    let source = document.to_string();
    vapor_content::authored_manifest::AuthoredContentManifest::parse(&source)
        .map_err(|error| format!("forked source manifest is invalid: {error}"))?;
    Ok(source)
}

#[cfg(test)]
mod source_metadata_repair_tests {
    use super::repair_dependency_workshop_id;

    #[test]
    fn repair_adds_workshop_id_to_the_matching_dependency_section() {
        let source = "[game]\nid = \"ghf/demo/game\"\n\n[game.engine]\nid = \"ghf/demo/engine\"\n";

        let (repaired, changed) =
            repair_dependency_workshop_id(source, "game.engine", "ghf/demo/engine", "12345")
                .expect("repair valid source TOML");

        assert!(changed);
        assert!(
            repaired.contains("[game.engine]\nid = \"ghf/demo/engine\"\nworkshop-id = \"12345\"\n")
        );
    }

    #[test]
    fn repair_ignores_other_dependency_ids() {
        let source = "[game.engine]\nid = \"ghf/demo/other-engine\"\nworkshop-id = \"12345\"\n";

        let (repaired, changed) =
            repair_dependency_workshop_id(source, "game.engine", "ghf/demo/engine", "12345")
                .expect("inspect valid source TOML");

        assert!(!changed);
        assert_eq!(repaired, source);
    }

    #[test]
    fn repair_preserves_inline_comments_and_is_idempotent() {
        let source =
            "[game.engine]\nid = \"ghf/demo/engine\"\nworkshop-id = \"111\" # publisher note\n";

        let (repaired, changed) =
            repair_dependency_workshop_id(source, "game.engine", "ghf/demo/engine", "222")
                .expect("repair stale Workshop ID");
        let (again, changed_again) =
            repair_dependency_workshop_id(&repaired, "game.engine", "ghf/demo/engine", "222")
                .expect("repeat metadata repair");

        assert!(changed);
        assert!(!changed_again);
        assert_eq!(again, repaired);
        assert!(repaired.contains("workshop-id = \"222\" # publisher note"));
    }
}
