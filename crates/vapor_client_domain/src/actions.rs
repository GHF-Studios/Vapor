use crate::ProfileAnalysisOptions;
pub use crate::{CompositionAction, LibraryAction};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "family", content = "action", rename_all = "kebab-case")]
pub enum ClientAction {
    Catalog(CatalogAction),
    Composition(CompositionAction),
    Library(LibraryAction),
    Workspace(WorkspaceAction),
    LanguageServer(LanguageServerAction),
    Authoring(AuthoringAction),
    Workshop(WorkshopAction),
    Publishing(PublishingAction),
    Developer(DeveloperAction),
    Session(SessionAction),
}

/// An operation on the Vapor-managed language server attached to this client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum LanguageServerAction {
    /// Start the installation's pinned Rust Analyzer for the opened workspace.
    Start { workspace_root: String },
    /// Stop the language server owned by this connected client.
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum SessionAction {
    CheckpointLocalState {
        editor_drafts: Option<String>,
        operation_history: Option<String>,
    },
    CancelOperation {
        operation_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum CatalogAction {
    Refresh,
    DiscoverContentProjects,
    Search {
        query: String,
    },
    Inspect {
        content_id: String,
        version: Option<String>,
    },
    AdoptWorkspaceProject {
        path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum WorkspaceAction {
    InitializeSuperworkspace,
    SetSuperworkspaceLocation {
        root: String,
    },
    DiscoverSuperworkspaceRepositories,
    AcquireSuperworkspaceRepositories {
        repository_names: Vec<String>,
    },
    Discover {
        root: String,
    },
    DiscoverWorkspace {
        root: String,
    },
    Open {
        root: String,
    },
    Validate,
    BuildDocumentation,
    PublishDocumentation,
    RunTool {
        operation: WorkspaceToolOperation,
        #[serde(default)]
        project_selection: WorkspaceProjectSelection,
        #[serde(default)]
        target_triples: Vec<String>,
    },
    /// Run an authored workspace configuration through the shared Client service.
    RunConfiguration {
        workspace_root: String,
        configuration: String,
        #[serde(default)]
        profiling: bool,
    },
    /// Promote already-built application outputs into the local Vapor installation.
    DeployApplication {
        #[serde(default)]
        target_triples: Vec<String>,
        #[serde(default)]
        skip_documentation: bool,
    },
    /// Build release outputs and stage the local first-party application package.
    PackageApplication {
        #[serde(default)]
        target_triples: Vec<String>,
    },
    Clean {
        confirmed_workspace_root: String,
    },
    Remember,
    Create {
        root: String,
        name: String,
    },
    AdoptProject {
        path: String,
    },
    UnregisterProject {
        project_id: String,
    },
    ListFiles {
        path: String,
    },
    OpenFile {
        path: String,
    },
    SaveFile {
        path: String,
        expected_fingerprint: String,
        contents: String,
    },
    CreateFile {
        path: String,
        contents: String,
    },
}

/// Managed development action for an opened workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceToolOperation {
    /// Format workspace Rust source.
    Format,
    /// Type-check the workspace.
    Check,
    /// Run workspace tests.
    Test,
    /// Build the workspace.
    Build,
    /// Build optimized release artifacts.
    BuildRelease,
    /// Check formatting, compile, test, lint, and build documentation.
    Validate,
}

/// Selects Cargo projects for a managed workspace tool operation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "selection", content = "name", rename_all = "kebab-case")]
pub enum WorkspaceProjectSelection {
    /// Every Cargo project declared by the workspace.
    #[default]
    All,
    /// Projects that declare runtime binaries for local deployment.
    Installable,
    /// One project selected by its declared workspace name.
    Named(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum AuthoringAction {
    ListRegisteredSources,
    RememberSource {
        root: String,
    },
    OpenSource {
        root: String,
    },
    ForgetSource {
        root: String,
    },
    CloseSource,
    LoadComposition {
        project_id: String,
    },
    SaveComposition {
        project_id: String,
        expected_fingerprint: String,
        dependencies: Vec<CompositionDependency>,
    },
    LoadWorkshopMetadata {
        project_id: String,
    },
    SaveWorkshopMetadata {
        project_id: String,
        expected_fingerprint: String,
        app_id: Option<u32>,
        published_file_id: Option<String>,
        visibility: Option<String>,
        title: Option<String>,
        description: Option<String>,
        tags: Vec<String>,
        change_note: Option<String>,
    },
    LoadRuntimeOutputs {
        project_id: String,
    },
    ValidateContent {
        project_id: String,
    },
    PackageContent {
        selector: String,
        #[serde(default)]
        target_triples: Vec<String>,
        #[serde(default)]
        dry_run: bool,
    },
    SaveRuntimeOutputs {
        project_id: String,
        expected_fingerprint: String,
        binaries: Vec<String>,
        libraries: Vec<String>,
    },
    CreateContent {
        workspace_root: String,
        kind: String,
        id: String,
        version: String,
        dependencies: Vec<String>,
        #[serde(default)]
        optional_dependencies: Vec<String>,
        workshop: WorkshopMetadataDraft,
    },
    ForkContent {
        workspace_root: String,
        content_id: String,
        name: String,
    },
    CreateWorkspace {
        parent: String,
        organization: String,
        name: String,
    },
    InitializeSourceWorkspace {
        path: String,
        organization: String,
        name: String,
        app_id: Option<u32>,
    },
    RepairSourceMetadata {
        workspace_root: String,
        write_changes: bool,
    },
}

/// Editable Workshop publication metadata supplied with authored content.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkshopMetadataDraft {
    pub app_id: Option<u32>,
    pub published_file_id: Option<String>,
    pub visibility: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub change_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositionDependency {
    pub project_id: String,
    pub relationship: String,
    pub version_requirement: Option<String>,
    pub workshop_id: Option<String>,
    pub optional: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum WorkshopAction {
    Search {
        query: String,
        page: u32,
        #[serde(default)]
        required_tags: Vec<String>,
    },
    LoadSubscriptions,
    Subscribe {
        item_ids: Vec<String>,
    },
    Unsubscribe {
        item_ids: Vec<String>,
    },
    Acquire {
        item_ids: Vec<String>,
    },
    Stage {
        #[serde(alias = "project_id")]
        selector: String,
        #[serde(default)]
        target_triples: Vec<String>,
    },
    StageBatch {
        selectors: Vec<String>,
        #[serde(default)]
        target_triples: Vec<String>,
    },
    Publish {
        review_id: String,
        #[serde(default)]
        account: String,
        confirmation: String,
        #[serde(default)]
        change_note: String,
    },
    PublishBatch {
        review_id: String,
        #[serde(default)]
        account: String,
        confirmation: String,
        #[serde(default)]
        change_note: String,
    },
    DiscardReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum PublishingAction {
    StageRoot {
        account: String,
        description: String,
        set_live: Option<String>,
    },
    StageApplication {
        account: String,
        branch: Option<String>,
        description: String,
        runtime_targets: Vec<String>,
        build_release: bool,
    },
    PublishRoot {
        review_id: String,
        confirmation: String,
    },
    PublishApplication {
        review_id: String,
        confirmation: String,
    },
    DiscardRootReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum DeveloperAction {
    RefreshInstallationRole,
    PromoteInstallationRole {
        target: String,
    },
    DemoteInstallationRole {
        target: String,
    },
    SubscribeTelemetry {
        enabled: bool,
    },
    OpenProfiler,
    RefreshProfileTraces,
    ImportProfileTrace {
        path: String,
    },
    AnalyzeProfileTrace {
        trace_id: String,
        options: ProfileAnalysisOptions,
    },
}

impl ClientAction {
    /// Stable diagnostic identifier for the semantic action.
    pub fn id(&self) -> &'static str {
        match self {
            Self::Catalog(action) => match action {
                CatalogAction::Refresh => "catalog.refresh",
                CatalogAction::DiscoverContentProjects => "catalog.discover-content-projects",
                CatalogAction::Search { .. } => "catalog.search",
                CatalogAction::Inspect { .. } => "catalog.inspect",
                CatalogAction::AdoptWorkspaceProject { .. } => "catalog.adopt-workspace-project",
            },
            Self::Library(action) => match action {
                LibraryAction::SetAutoInstall { .. } => "library.set-auto-install",
                LibraryAction::Install { .. } => "library.install",
                LibraryAction::Acquire { .. } => "library.acquire",
                LibraryAction::ValidateArtifacts { .. } => "library.validate-artifacts",
                LibraryAction::Update { .. } => "library.update",
                LibraryAction::Verify { .. } => "library.verify",
                LibraryAction::Repair { .. } => "library.repair",
                LibraryAction::Disable { .. } => "library.disable",
                LibraryAction::Enable { .. } => "library.enable",
                LibraryAction::Uninstall { .. } => "library.uninstall",
            },
            Self::Composition(action) => match action {
                CompositionAction::Create { .. } => "composition.create",
                CompositionAction::Duplicate { .. } => "composition.duplicate",
                CompositionAction::Rename { .. } => "composition.rename",
                CompositionAction::Remove { .. } => "composition.remove",
                CompositionAction::Select { .. } => "composition.select",
                CompositionAction::SetRoot { .. } => "composition.set-root",
                CompositionAction::ResolveDefaultRoot { .. } => "composition.resolve-default-root",
                CompositionAction::SetLaunchArguments { .. } => "composition.set-launch-arguments",
                CompositionAction::AdoptVerifiedGraph { .. } => "composition.adopt-verified-graph",
                CompositionAction::InvalidateContent { .. } => "composition.invalidate-content",
                CompositionAction::Preview { .. } => "composition.preview",
                CompositionAction::Install { .. } => "composition.install",
                CompositionAction::Verify { .. } => "composition.verify",
                CompositionAction::Repair { .. } => "composition.repair",
                CompositionAction::Update { .. } => "composition.update",
                CompositionAction::Launch { .. } => "composition.launch",
                CompositionAction::RunSource { .. } => "composition.run-source",
            },
            Self::Workspace(action) => match action {
                WorkspaceAction::InitializeSuperworkspace => "workspace.initialize-superworkspace",
                WorkspaceAction::SetSuperworkspaceLocation { .. } => {
                    "workspace.set-superworkspace-location"
                }
                WorkspaceAction::DiscoverSuperworkspaceRepositories => {
                    "workspace.discover-superworkspace-repositories"
                }
                WorkspaceAction::AcquireSuperworkspaceRepositories { .. } => {
                    "workspace.acquire-superworkspace-repositories"
                }
                WorkspaceAction::Discover { .. } => "workspace.discover",
                WorkspaceAction::DiscoverWorkspace { .. } => "workspace.discover-workspace",
                WorkspaceAction::Open { .. } => "workspace.open",
                WorkspaceAction::Validate => "workspace.validate",
                WorkspaceAction::BuildDocumentation => "workspace.build-documentation",
                WorkspaceAction::PublishDocumentation => "workspace.publish-documentation",
                WorkspaceAction::RunTool { operation, .. } => match operation {
                    WorkspaceToolOperation::Format => "workspace.fmt",
                    WorkspaceToolOperation::Check => "workspace.check",
                    WorkspaceToolOperation::Test => "workspace.test",
                    WorkspaceToolOperation::Build => "workspace.build",
                    WorkspaceToolOperation::BuildRelease => "workspace.build-release",
                    WorkspaceToolOperation::Validate => "workspace.validate",
                },
                WorkspaceAction::RunConfiguration { .. } => "workspace.run-configuration",
                WorkspaceAction::DeployApplication { .. } => "workspace.deploy-application",
                WorkspaceAction::PackageApplication { .. } => "workspace.package-application",
                WorkspaceAction::Clean { .. } => "workspace.clean",
                WorkspaceAction::Remember => "workspace.remember",
                WorkspaceAction::Create { .. } => "workspace.create",
                WorkspaceAction::AdoptProject { .. } => "workspace.adopt-project",
                WorkspaceAction::UnregisterProject { .. } => "workspace.unregister-project",
                WorkspaceAction::ListFiles { .. } => "workspace.list-files",
                WorkspaceAction::OpenFile { .. } => "workspace.open-file",
                WorkspaceAction::SaveFile { .. } => "workspace.save-file",
                WorkspaceAction::CreateFile { .. } => "workspace.create-file",
            },
            Self::LanguageServer(action) => match action {
                LanguageServerAction::Start { .. } => "language-server.start",
                LanguageServerAction::Stop => "language-server.stop",
            },
            Self::Authoring(action) => match action {
                AuthoringAction::ListRegisteredSources => "authoring.list-registered-sources",
                AuthoringAction::RememberSource { .. } => "authoring.remember-source",
                AuthoringAction::OpenSource { .. } => "authoring.open-source",
                AuthoringAction::ForgetSource { .. } => "authoring.forget-source",
                AuthoringAction::CloseSource => "authoring.close-source",
                AuthoringAction::LoadComposition { .. } => "authoring.load-composition",
                AuthoringAction::SaveComposition { .. } => "authoring.save-composition",
                AuthoringAction::LoadWorkshopMetadata { .. } => "authoring.load-workshop-metadata",
                AuthoringAction::SaveWorkshopMetadata { .. } => "authoring.save-workshop-metadata",
                AuthoringAction::LoadRuntimeOutputs { .. } => "authoring.load-runtime-outputs",
                AuthoringAction::ValidateContent { .. } => "authoring.validate-content",
                AuthoringAction::PackageContent { .. } => "authoring.package-content",
                AuthoringAction::SaveRuntimeOutputs { .. } => "authoring.save-runtime-outputs",
                AuthoringAction::CreateContent { .. } => "authoring.create-content",
                AuthoringAction::ForkContent { .. } => "authoring.fork-content",
                AuthoringAction::CreateWorkspace { .. } => "authoring.create-workspace",
                AuthoringAction::InitializeSourceWorkspace { .. } => {
                    "authoring.initialize-source-workspace"
                }
                AuthoringAction::RepairSourceMetadata { .. } => "authoring.repair-source-metadata",
            },
            Self::Workshop(action) => match action {
                WorkshopAction::Search { .. } => "workshop.search",
                WorkshopAction::LoadSubscriptions => "workshop.load-subscriptions",
                WorkshopAction::Subscribe { .. } => "workshop.subscribe",
                WorkshopAction::Unsubscribe { .. } => "workshop.unsubscribe",
                WorkshopAction::Acquire { .. } => "workshop.acquire",
                WorkshopAction::Stage { .. } => "workshop.stage",
                WorkshopAction::StageBatch { .. } => "workshop.stage-batch",
                WorkshopAction::Publish { .. } => "workshop.publish",
                WorkshopAction::PublishBatch { .. } => "workshop.publish-batch",
                WorkshopAction::DiscardReview => "workshop.discard-review",
            },
            Self::Publishing(action) => match action {
                PublishingAction::StageRoot { .. } => "publishing.stage-root",
                PublishingAction::StageApplication { .. } => "publishing.stage-application",
                PublishingAction::PublishRoot { .. } => "publishing.publish-root",
                PublishingAction::PublishApplication { .. } => "publishing.publish-application",
                PublishingAction::DiscardRootReview => "publishing.discard-root-review",
            },
            Self::Developer(action) => match action {
                DeveloperAction::RefreshInstallationRole => "developer.refresh-installation-role",
                DeveloperAction::PromoteInstallationRole { .. } => {
                    "developer.promote-installation-role"
                }
                DeveloperAction::DemoteInstallationRole { .. } => {
                    "developer.demote-installation-role"
                }
                DeveloperAction::SubscribeTelemetry { .. } => "developer.subscribe-telemetry",
                DeveloperAction::OpenProfiler => "developer.open-profiler",
                DeveloperAction::RefreshProfileTraces => "developer.refresh-profile-traces",
                DeveloperAction::ImportProfileTrace { .. } => "developer.import-profile-trace",
                DeveloperAction::AnalyzeProfileTrace { .. } => "developer.analyze-profile-trace",
            },
            Self::Session(SessionAction::CheckpointLocalState { .. }) => {
                "session.checkpoint-local-state"
            }
            Self::Session(SessionAction::CancelOperation { .. }) => "session.cancel-operation",
        }
    }
}
