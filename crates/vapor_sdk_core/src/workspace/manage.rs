use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use toml::Value;
use vapor_core::ContentType;

use crate::GlobalOptions;

use super::error::WorkspaceCommandError;
use super::identity::{WORKSPACE_MANIFEST_FILE, WorkspaceIdentity, discover_workspace_identity};
use super::report::{WorkspaceStatusReport, WorkspaceSyncReport};

const GENERATED_HEADER: &str = "# MANAGED BY VAPOR. EDIT Workspace.vapor.toml OR A ROLE-SPECIFIC CONTENT MANIFEST, THEN RUN `vapor sdk workspace sync`.\n";
const GENERATED_FOOTER: &str = "# END MANAGED BY VAPOR.\n";
const GENERATED_RS_HEADER: &str = "// MANAGED BY VAPOR. EDIT Workspace.vapor.toml OR A ROLE-SPECIFIC CONTENT MANIFEST, THEN RUN `vapor sdk workspace sync`.\n";
const GENERATED_RS_FOOTER: &str = "// END MANAGED BY VAPOR.\n";
const SEEDED_RS_HEADER: &str = "// SEEDED BY VAPOR. THIS FILE IS USER-OWNED AFTER CREATION.\n";
const SEEDED_RS_FOOTER: &str = "// END VAPOR SEED.\n";

#[derive(Debug, Deserialize)]
struct WorkspaceManifest {
    workspace: Option<WorkspaceSection>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct WorkspaceSection {
    #[serde(default)]
    projects: Vec<WorkspaceProjectEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct WorkspaceProjectEntry {
    path: PathBuf,
}

#[derive(Debug)]
struct ContentEntry {
    kind: ContentType,
    name: String,
    id: String,
    path: PathBuf,
}

pub(super) fn workspace_status(
    globals: &GlobalOptions,
) -> Result<WorkspaceStatusReport, WorkspaceCommandError> {
    Ok(status_from_identity(discover_workspace_identity(globals)?))
}

pub(super) fn workspace_sync(
    globals: &GlobalOptions,
) -> Result<WorkspaceSyncReport, WorkspaceCommandError> {
    let identity = discover_workspace_identity(globals)?;
    let content = read_content_graph(&identity)?;
    let mut changed_paths = Vec::new();
    let cargo_manifest = identity.workspace_root.join("Cargo.toml");

    sync_managed_file(
        &cargo_manifest,
        &workspace_cargo_toml(&content),
        GENERATED_HEADER,
        GENERATED_FOOTER,
        &mut changed_paths,
    )?;

    for entry in &content {
        let crate_dir = identity.workspace_root.join(&entry.path);
        let src_dir = crate_dir.join("src");

        if !src_dir.exists() {
            fs::create_dir_all(&src_dir)?;
            changed_paths.push(src_dir.clone());
        }

        sync_managed_file(
            &crate_dir.join("Cargo.toml"),
            &crate_cargo_toml(entry),
            GENERATED_HEADER,
            GENERATED_FOOTER,
            &mut changed_paths,
        )?;
        sync_seed_file(
            &src_dir.join("lib.rs"),
            &content_lib_rs(entry),
            &legacy_content_lib_rs(entry),
            &mut changed_paths,
        )?;
    }

    Ok(WorkspaceSyncReport {
        status: status_from_identity(identity),
        changed_paths,
    })
}

fn read_content_graph(
    identity: &WorkspaceIdentity,
) -> Result<Vec<ContentEntry>, WorkspaceCommandError> {
    let manifest = fs::read_to_string(identity.workspace_root.join(WORKSPACE_MANIFEST_FILE))?;
    let manifest: WorkspaceManifest = toml::from_str(&manifest)?;
    let projects = manifest
        .workspace
        .map(|workspace| workspace.projects)
        .unwrap_or_default();

    if projects.is_empty() {
        return Err(WorkspaceCommandError::MissingContentGraph);
    }

    let mut package_names = HashSet::new();
    let mut content = Vec::new();
    for project in projects {
        let entry = read_project_content(identity, project)?;
        if !valid_cargo_package_name(&entry.name) {
            return Err(WorkspaceCommandError::InvalidContentGraph(format!(
                "content `{}` has invalid Cargo package name `{}`",
                entry.id, entry.name
            )));
        }
        if !package_names.insert(entry.name.clone()) {
            return Err(WorkspaceCommandError::InvalidContentGraph(format!(
                "multiple content entries use the generated Cargo package name `{}`",
                entry.name
            )));
        }
        content.push(entry);
    }

    Ok(content)
}

fn read_project_content(
    identity: &WorkspaceIdentity,
    project: WorkspaceProjectEntry,
) -> Result<ContentEntry, WorkspaceCommandError> {
    let project_root = identity.workspace_root.join(&project.path);
    let manifest = content_manifest_at(&project_root)?.ok_or_else(|| {
        WorkspaceCommandError::InvalidContentGraph(format!(
            "workspace project `{}` has no role-specific content manifest",
            project.path.display()
        ))
    })?;
    let source = fs::read_to_string(&manifest)?;
    let value = source.parse::<Value>()?;
    let kind = content_type_for_manifest(&manifest)?;
    let section = value.get(kind.as_str()).ok_or_else(|| {
        WorkspaceCommandError::InvalidContentGraph(format!(
            "{} must declare [{}]",
            manifest.display(),
            kind.as_str()
        ))
    })?;
    let name = section
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            WorkspaceCommandError::InvalidContentGraph(format!(
                "{} [{}].name is required",
                manifest.display(),
                kind.as_str()
            ))
        })?
        .to_owned();
    Ok(ContentEntry {
        kind,
        id: format!("{}/{}", identity.id, name),
        name,
        path: project.path,
    })
}

fn content_manifest_at(root: &Path) -> Result<Option<PathBuf>, WorkspaceCommandError> {
    let mut found = Vec::new();
    for kind in [
        ContentType::Packagepack,
        ContentType::Enginepack,
        ContentType::Gamepack,
        ContentType::Modpack,
        ContentType::Engine,
        ContentType::Game,
        ContentType::EngineMod,
        ContentType::GameMod,
        ContentType::ExtensionMod,
    ] {
        let candidate = root.join(content_manifest_file_name(kind));
        if candidate.is_file() {
            found.push(candidate);
        }
    }
    match found.len() {
        0 => Ok(None),
        1 => Ok(found.pop()),
        _ => Err(WorkspaceCommandError::InvalidContentGraph(format!(
            "workspace project `{}` has multiple role-specific content manifests",
            root.display()
        ))),
    }
}

fn content_type_for_manifest(path: &Path) -> Result<ContentType, WorkspaceCommandError> {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Err(WorkspaceCommandError::InvalidContentGraph(format!(
            "content manifest path has no filename: {}",
            path.display()
        )));
    };
    match file_name {
        "Packagepack.vapor.toml" => Ok(ContentType::Packagepack),
        "Enginepack.vapor.toml" => Ok(ContentType::Enginepack),
        "Gamepack.vapor.toml" => Ok(ContentType::Gamepack),
        "Modpack.vapor.toml" => Ok(ContentType::Modpack),
        "Engine.vapor.toml" => Ok(ContentType::Engine),
        "Game.vapor.toml" => Ok(ContentType::Game),
        "Engine-Mod.vapor.toml" => Ok(ContentType::EngineMod),
        "Game-Mod.vapor.toml" => Ok(ContentType::GameMod),
        "Extension-Mod.vapor.toml" => Ok(ContentType::ExtensionMod),
        "Library.vapor.toml" => Ok(ContentType::Library),
        other => Err(WorkspaceCommandError::InvalidContentGraph(format!(
            "unknown role-specific content manifest filename `{other}`"
        ))),
    }
}

fn content_manifest_file_name(kind: ContentType) -> &'static str {
    match kind {
        ContentType::Packagepack => "Packagepack.vapor.toml",
        ContentType::Enginepack => "Enginepack.vapor.toml",
        ContentType::Gamepack => "Gamepack.vapor.toml",
        ContentType::Modpack => "Modpack.vapor.toml",
        ContentType::Engine => "Engine.vapor.toml",
        ContentType::Game => "Game.vapor.toml",
        ContentType::EngineMod => "Engine-Mod.vapor.toml",
        ContentType::GameMod => "Game-Mod.vapor.toml",
        ContentType::ExtensionMod => "Extension-Mod.vapor.toml",
        ContentType::Library => "Library.vapor.toml",
    }
}

fn workspace_cargo_toml(content: &[ContentEntry]) -> String {
    let members = content
        .iter()
        .map(|entry| format!("    \"{}\"", entry.path.display()))
        .collect::<Vec<_>>()
        .join(",\n");

    format!(
        "{GENERATED_HEADER}[workspace]\nresolver = \"3\"\nmembers = [\n{members}\n]\n\n[workspace.package]\nversion = \"0.5.0\"\nedition = \"2024\"\n{GENERATED_FOOTER}"
    )
}

fn crate_cargo_toml(entry: &ContentEntry) -> String {
    format!(
        "{GENERATED_HEADER}[package]\nname = \"{}\"\nversion.workspace = true\nedition.workspace = true\n{GENERATED_FOOTER}",
        entry.name
    )
}

fn content_lib_rs(entry: &ContentEntry) -> String {
    format!(
        "{SEEDED_RS_HEADER}#![forbid(unsafe_code)]\n\npub const CONTENT_KIND: &str = \"{}\";\npub const CONTENT_ID: &str = \"{}\";\n{SEEDED_RS_FOOTER}",
        entry.kind, entry.id
    )
}

fn legacy_content_lib_rs(entry: &ContentEntry) -> String {
    format!(
        "{GENERATED_RS_HEADER}#![forbid(unsafe_code)]\n\npub const CONTENT_KIND: &str = \"{}\";\npub const CONTENT_ID: &str = \"{}\";\n{GENERATED_RS_FOOTER}",
        entry.kind, entry.id
    )
}

fn valid_cargo_package_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        })
}

fn sync_managed_file(
    path: &Path,
    content: &str,
    header: &str,
    footer: &str,
    changed_paths: &mut Vec<PathBuf>,
) -> Result<(), WorkspaceCommandError> {
    if path.exists() && !path.is_file() {
        return Err(WorkspaceCommandError::GeneratedPathIsNotFile(
            path.to_path_buf(),
        ));
    }

    if path.is_file() {
        let current = fs::read_to_string(path)?;
        if current == content {
            return Ok(());
        }
        if !is_managed_file(&current, header, footer) {
            return Err(WorkspaceCommandError::UnmanagedGeneratedFile(
                path.to_path_buf(),
            ));
        }
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, content)?;
    changed_paths.push(path.to_path_buf());
    Ok(())
}

fn sync_seed_file(
    path: &Path,
    content: &str,
    legacy_content: &str,
    changed_paths: &mut Vec<PathBuf>,
) -> Result<(), WorkspaceCommandError> {
    if path.exists() && !path.is_file() {
        return Err(WorkspaceCommandError::GeneratedPathIsNotFile(
            path.to_path_buf(),
        ));
    }

    if path.is_file() {
        let current = fs::read_to_string(path)?;
        if current == content {
            return Ok(());
        }
        if current != legacy_content {
            return Ok(());
        }
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, content)?;
    changed_paths.push(path.to_path_buf());
    Ok(())
}

fn is_managed_file(content: &str, header: &str, footer: &str) -> bool {
    content.starts_with(header) && content.ends_with(footer)
}

fn status_from_identity(identity: WorkspaceIdentity) -> WorkspaceStatusReport {
    WorkspaceStatusReport {
        cargo_manifest_exists: identity.workspace_root.join("Cargo.toml").is_file(),
        crates_dir_exists: identity.workspace_root.join("crates").is_dir(),
        invocation_directory: identity.invocation_directory,
        workspace_root: identity.workspace_root,
        workspace_name: identity.name,
        workspace_id: identity.id,
    }
}
