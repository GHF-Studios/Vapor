use crate::ContentReference;
use std::path::{Path, PathBuf};
use vapor_client_provider::{
    content,
    discovery::{EnvironmentPaths, InstallationPaths},
    manifest::ContentKind as ShellContentKind,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceContent {
    pub id: String,
    pub version: Option<String>,
    pub kind: String,
    pub root: PathBuf,
    pub manifest: PathBuf,
    pub binaries: Vec<String>,
    pub libraries: Vec<String>,
    pub dependencies: Vec<String>,
    pub workshop: SourceWorkshopPolicy,
}

/// Publication intent authored in a content role manifest.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceWorkshopPolicy {
    pub app_id: Option<u32>,
    pub published_file_id: Option<String>,
    pub visibility: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub change_note: Option<String>,
}

pub use vapor_client_provider::content::SourceRuntimeOutputs;
pub use vapor_client_provider::content::SourceWorkshopMetadata;
pub use vapor_client_provider::content::WorkshopPolicyDraft;

pub use vapor_client_provider::content::{
    CompositionLink as SourceCompositionLink, SourceComposition,
    SourceProjectCandidate as UnregisteredProjectCandidate,
    SourceProjectScan as UnregisteredProjectScan,
};

pub fn load_source_composition(
    application_executable: &Path,
    workspace_root: &Path,
    selector: &str,
) -> Result<SourceComposition, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    content::load_source_composition(&paths, selector)
}

pub fn save_source_composition(
    application_executable: &Path,
    workspace_root: &Path,
    selector: &str,
    expected_source: &str,
    links: &[SourceCompositionLink],
) -> Result<SourceComposition, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    content::save_source_composition(&paths, selector, expected_source, links)
}

pub fn load_source_workshop_metadata(
    application_executable: &Path,
    workspace_root: &Path,
    selector: &str,
) -> Result<SourceWorkshopMetadata, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    content::load_source_workshop_metadata(&paths, selector)
}

pub fn save_source_workshop_metadata(
    application_executable: &Path,
    workspace_root: &Path,
    selector: &str,
    expected_source: &str,
    policy: &WorkshopPolicyDraft,
) -> Result<SourceWorkshopMetadata, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    content::save_source_workshop_metadata(&paths, selector, expected_source, policy)
}

pub fn load_source_runtime_outputs(
    application_executable: &Path,
    workspace_root: &Path,
    selector: &str,
) -> Result<SourceRuntimeOutputs, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    content::load_source_runtime_outputs(&paths, selector)
}

pub fn save_source_runtime_outputs(
    application_executable: &Path,
    workspace_root: &Path,
    selector: &str,
    expected_source: &str,
    binaries: &[String],
    libraries: &[String],
) -> Result<SourceRuntimeOutputs, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    content::save_source_runtime_outputs(&paths, selector, expected_source, binaries, libraries)
}

/// Ask Vapor's shared content rules whether one source role can directly depend on another.
pub fn source_dependency_allowed(
    parent: vapor_core::ContentKind,
    child: vapor_core::ContentKind,
) -> bool {
    source_dependency_relationship(parent, child).is_some()
}

/// Return the Shell-authored relationship for one supported role pair.
pub fn source_dependency_relationship(
    parent: vapor_core::ContentKind,
    child: vapor_core::ContentKind,
) -> Option<&'static str> {
    let parent = shell_content_kind(parent);
    let child = shell_content_kind(child);
    content::source_dependency_relationship(parent, child)
}

/// Return the exclusive role slot used to prevent conflicting content choices.
pub fn source_dependency_choice_group(
    parent: vapor_core::ContentKind,
    child: vapor_core::ContentKind,
) -> Option<&'static str> {
    let parent = shell_content_kind(parent);
    let child = shell_content_kind(child);
    content::source_dependency_choice_group(parent, child)
}

/// Parse a content kind emitted by workspace project discovery.
pub fn source_project_kind(kind: &str) -> Option<ShellContentKind> {
    Some(match kind {
        "engine" => ShellContentKind::Engine,
        "game" => ShellContentKind::Game,
        "packagepack" => ShellContentKind::Packagepack,
        "enginepack" => ShellContentKind::Enginepack,
        "gamepack" => ShellContentKind::Gamepack,
        "modpack" => ShellContentKind::Modpack,
        "engine-mod" => ShellContentKind::EngineMod,
        "game-mod" => ShellContentKind::GameMod,
        "extension-mod" => ShellContentKind::ExtensionMod,
        "crate" => ShellContentKind::Crate,
        _ => return None,
    })
}

/// Return every exclusive dependency role required for a project to be valid.
pub fn source_required_dependency_choice_groups(
    parent: vapor_core::ContentKind,
) -> &'static [&'static str] {
    content::source_required_dependency_choice_groups(shell_content_kind(parent))
}

pub fn discover_source_content(
    application_executable: &Path,
    workspace_root: &Path,
) -> Result<Vec<SourceContent>, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    Ok(content::discover(&paths)?
        .artifacts()
        .iter()
        .map(|artifact| SourceContent {
            id: artifact.id().to_owned(),
            version: artifact.version().map(ToOwned::to_owned),
            kind: artifact.kind().to_string(),
            root: artifact.root().to_path_buf(),
            manifest: artifact.manifest().to_path_buf(),
            binaries: artifact.binaries().to_vec(),
            libraries: artifact.libraries().to_vec(),
            dependencies: artifact
                .dependencies()
                .iter()
                .map(|dependency| format!("{} · {}", dependency.relationship(), dependency.id()))
                .collect(),
            workshop: SourceWorkshopPolicy {
                app_id: artifact.workshop().app_id(),
                published_file_id: artifact
                    .workshop()
                    .published_file_id()
                    .map(ToOwned::to_owned),
                visibility: artifact.workshop().visibility().map(ToOwned::to_owned),
                title: artifact.workshop().title().map(ToOwned::to_owned),
                description: artifact.workshop().description().map(ToOwned::to_owned),
                tags: artifact.workshop().tags().to_vec(),
                change_note: artifact.workshop().change_note().map(ToOwned::to_owned),
            },
        })
        .collect())
}

/// Validate the active Vapor Workspace manifest without requiring every
/// content project inside it to be valid.
pub fn validate_workspace(workspace_root: &Path) -> Result<(), String> {
    vapor_core::VaporWorkspace::load(workspace_root)
        .map(|_| ())
        .map_err(|error| format!("{}: {error}", workspace_root.display()))
}

pub fn validate_composition_source(
    application_executable: &Path,
    workspace_root: &Path,
    selector: &str,
) -> Result<String, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    let report = content::validate(&paths, Some(selector))?;
    Ok(format!(
        "Validated {} source artifact{}; {} diagnostic{}.",
        report.checked().len(),
        if report.checked().len() == 1 { "" } else { "s" },
        report.diagnostics().len(),
        if report.diagnostics().len() == 1 {
            ""
        } else {
            "s"
        },
    ))
}

/// Validate every authored content project in a workspace without building it.
pub fn validate_workspace_source(
    application_executable: &Path,
    workspace_root: &Path,
) -> Result<String, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    let report = content::validate(&paths, None)?;
    let mut summary = format!(
        "Validated {} authored content project{}.",
        report.checked().len(),
        if report.checked().len() == 1 { "" } else { "s" },
    );
    for diagnostic in report.diagnostics() {
        summary.push_str("\n• ");
        summary.push_str(diagnostic);
    }
    Ok(summary)
}

pub fn create_content_workspace(
    application_executable: &Path,
    workspace_root: &Path,
    id: &str,
    version: &str,
    kind: vapor_core::ContentKind,
    dependency_selectors: &[String],
) -> Result<PathBuf, String> {
    create_content_workspace_with_workshop(
        application_executable,
        workspace_root,
        id,
        version,
        kind,
        dependency_selectors,
        &WorkshopPolicyDraft::default(),
    )
}

pub fn create_content_workspace_with_workshop(
    application_executable: &Path,
    workspace_root: &Path,
    id: &str,
    version: &str,
    kind: vapor_core::ContentKind,
    dependency_selectors: &[String],
    workshop: &WorkshopPolicyDraft,
) -> Result<PathBuf, String> {
    create_content_workspace_with_dependency_policies(
        application_executable,
        workspace_root,
        id,
        version,
        kind,
        dependency_selectors,
        &[],
        workshop,
    )
}

/// Create source content with explicit optional dependency selections.
pub fn create_content_workspace_with_dependency_policies(
    application_executable: &Path,
    workspace_root: &Path,
    id: &str,
    version: &str,
    kind: vapor_core::ContentKind,
    dependency_selectors: &[String],
    optional_dependency_selectors: &[String],
    workshop: &WorkshopPolicyDraft,
) -> Result<PathBuf, String> {
    let kind = shell_content_kind(kind);
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    content::create_source_project_with_dependency_policies(
        &paths,
        id,
        version,
        kind,
        dependency_selectors,
        optional_dependency_selectors,
        workshop,
    )
}

/// Scaffold the maintained Engine/Game/Packagepack starter workspace.
pub fn create_basic_content_workspace(
    application_executable: &Path,
    destination: &Path,
    organization: &str,
    name: &str,
) -> Result<PathBuf, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    content::create_basic_content_workspace(
        &installation,
        destination.to_path_buf(),
        organization,
        name,
    )
}

fn shell_content_kind(kind: vapor_core::ContentKind) -> ShellContentKind {
    match kind {
        vapor_core::ContentKind::Engine => ShellContentKind::Engine,
        vapor_core::ContentKind::Game => ShellContentKind::Game,
        vapor_core::ContentKind::Packagepack => ShellContentKind::Packagepack,
        vapor_core::ContentKind::Enginepack => ShellContentKind::Enginepack,
        vapor_core::ContentKind::Gamepack => ShellContentKind::Gamepack,
        vapor_core::ContentKind::Modpack => ShellContentKind::Modpack,
        vapor_core::ContentKind::EngineMod => ShellContentKind::EngineMod,
        vapor_core::ContentKind::GameMod => ShellContentKind::GameMod,
        vapor_core::ContentKind::ExtensionMod => ShellContentKind::ExtensionMod,
        vapor_core::ContentKind::Crate => ShellContentKind::Crate,
    }
}

/// Add a validated authored Workspace to this Vapor Installation's known sources.
pub fn remember_workspace(workspace_root: &Path) -> Result<Vec<PathBuf>, String> {
    let installation =
        vapor_core::VaporInstallation::discover().map_err(|error| error.to_string())?;
    let state = vapor_core::remember_source(&installation, workspace_root)
        .map_err(|error| error.to_string())?;
    Ok(state.known)
}

/// Register an existing content project through Vapor Client without moving it.
pub fn adopt_workspace_content_project(
    application_executable: &Path,
    workspace_root: &Path,
    project_path: &Path,
) -> Result<(String, Vec<SourceContent>), String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    let summary = content::adopt_source_project(&paths, project_path)?;
    let contents = discover_source_content(application_executable, workspace_root)?;
    Ok((summary, contents))
}

/// Remove a source content project from Vapor discovery while retaining its files and Cargo membership.
pub fn unregister_workspace_content_project(
    application_executable: &Path,
    workspace_root: &Path,
    id: &str,
) -> Result<(String, Vec<SourceContent>, ContentReference), String> {
    let before = discover_source_content(application_executable, workspace_root)?;
    let identity = before
        .iter()
        .find(|content| content.id == id)
        .map(|content| ContentReference {
            id: content.id.clone(),
            version: content.version.clone(),
        })
        .ok_or_else(|| {
            format!("content project is not currently discoverable in this Workspace: {id}")
        })?;
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    let summary = content::unregister_source_project(&paths, id)?;
    let contents = discover_source_content(application_executable, workspace_root)?;
    Ok((summary, contents, identity))
}

/// Find valid, unregistered content folders without changing the Workspace.
pub fn scan_unregistered_content_projects(
    application_executable: &Path,
    workspace_root: &Path,
) -> Result<UnregisteredProjectScan, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    content::scan_unregistered_source_projects(&paths)
}
