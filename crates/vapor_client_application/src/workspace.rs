use crate::ContentReference;
use std::path::{Path, PathBuf};
use vapor_client_provider::{
    content,
    discovery::{EnvironmentPaths, InstallationPaths},
};

pub use vapor_client_provider::content::{BasicSourceWorkspaceReport, SourceMetadataRepairReport};

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

/// Required and optional dependencies selected while authoring a content project.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContentDependencyDraft {
    /// Required dependencies that participate in the default resolved graph.
    pub required: Vec<String>,
    /// Optional dependencies available for later explicit selection.
    pub optional: Vec<String>,
}

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
    vapor_client_domain::dependency_kind_allowed(parent, child)
}

/// Return the Shell-authored relationship for one supported role pair.
pub fn source_dependency_relationship(
    parent: vapor_core::ContentKind,
    child: vapor_core::ContentKind,
) -> Option<&'static str> {
    vapor_client_domain::dependency_relationship(parent, child)
}

/// Return the exclusive role slot used to prevent conflicting content choices.
pub fn source_dependency_choice_group(
    parent: vapor_core::ContentKind,
    child: vapor_core::ContentKind,
) -> Option<&'static str> {
    vapor_client_domain::dependency_choice_group(parent, child)
}

/// Return every exclusive dependency role required for a project to be valid.
pub fn source_required_dependency_choice_groups(
    parent: vapor_core::ContentKind,
) -> &'static [&'static str] {
    vapor_client_domain::required_dependency_choice_groups(parent)
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
        ContentDependencyDraft {
            required: dependency_selectors.to_vec(),
            optional: Vec::new(),
        },
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
    dependencies: ContentDependencyDraft,
    workshop: &WorkshopPolicyDraft,
) -> Result<PathBuf, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    content::create_source_project_with_dependency_policies(
        &paths,
        id,
        version,
        kind,
        &dependencies.required,
        &dependencies.optional,
        workshop,
    )
}

/// Fork acquired Workshop content into a distinct source project inside the
/// selected Workspace. The provider rejects packages without their declared
/// Rust source files instead of manufacturing an editable project.
pub fn fork_acquired_content(
    application_executable: &Path,
    workspace_root: &Path,
    selector: &str,
    new_name: &str,
) -> Result<PathBuf, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    vapor_client_provider::content::fork_cached_artifact(&paths, selector, new_name)
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

/// Initialize the maintained starter workspace at an exact destination.
///
/// # Errors
///
/// Returns validation, installation, or filesystem errors from the shared
/// content provider.
pub fn initialize_source_workspace(
    application_executable: &Path,
    destination: &Path,
    organization: &str,
    name: &str,
    app_id: Option<u32>,
) -> Result<BasicSourceWorkspaceReport, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    content::initialize_basic_source_workspace(
        &installation,
        destination.to_path_buf(),
        organization,
        name,
        app_id,
    )
}

/// Preview or apply Workshop ID repairs in a creator workspace.
pub fn repair_source_metadata(
    application_executable: &Path,
    workspace_root: &Path,
    write_changes: bool,
) -> Result<SourceMetadataRepairReport, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    content::repair_source_metadata(&paths, write_changes)
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
