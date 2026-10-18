use serde::{Deserialize, Serialize};
pub use crate::{CompositionAction, LibraryAction};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "family", content = "action", rename_all = "kebab-case")]
pub enum ClientAction {
    Catalog(CatalogAction),
    Composition(CompositionAction),
    Library(LibraryAction),
    Workspace(WorkspaceAction),
    Authoring(AuthoringAction),
    Workshop(WorkshopAction),
    Publishing(PublishingAction),
    Developer(DeveloperAction),
    Session(SessionAction),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum SessionAction {
    CheckpointLocalState {
        editor_drafts: Option<String>,
        operation_history: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum AuthoringAction {
    ListRegisteredSources,
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
    CreateWorkspace {
        parent: String,
        organization: String,
        name: String,
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
    },
    Publish {
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
    PublishRoot {
        review_id: String,
        confirmation: String,
    },
    DiscardRootReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum DeveloperAction {
    Format,
    Check,
    Test,
    Build,
    Clean { confirmation: String },
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
                WorkspaceAction::SetSuperworkspaceLocation { .. } => "workspace.set-superworkspace-location",
                WorkspaceAction::DiscoverSuperworkspaceRepositories => "workspace.discover-superworkspace-repositories",
                WorkspaceAction::AcquireSuperworkspaceRepositories { .. } => "workspace.acquire-superworkspace-repositories",
                WorkspaceAction::Discover { .. } => "workspace.discover",
                WorkspaceAction::DiscoverWorkspace { .. } => "workspace.discover-workspace",
                WorkspaceAction::Open { .. } => "workspace.open",
                WorkspaceAction::Validate => "workspace.validate",
                WorkspaceAction::Remember => "workspace.remember",
                WorkspaceAction::Create { .. } => "workspace.create",
                WorkspaceAction::AdoptProject { .. } => "workspace.adopt-project",
                WorkspaceAction::UnregisterProject { .. } => "workspace.unregister-project",
                WorkspaceAction::ListFiles { .. } => "workspace.list-files",
                WorkspaceAction::OpenFile { .. } => "workspace.open-file",
                WorkspaceAction::SaveFile { .. } => "workspace.save-file",
                WorkspaceAction::CreateFile { .. } => "workspace.create-file",
            },
            Self::Authoring(action) => match action {
                AuthoringAction::ListRegisteredSources => "authoring.list-registered-sources",
                AuthoringAction::LoadComposition { .. } => "authoring.load-composition",
                AuthoringAction::SaveComposition { .. } => "authoring.save-composition",
                AuthoringAction::LoadWorkshopMetadata { .. } => "authoring.load-workshop-metadata",
                AuthoringAction::SaveWorkshopMetadata { .. } => "authoring.save-workshop-metadata",
                AuthoringAction::LoadRuntimeOutputs { .. } => "authoring.load-runtime-outputs",
                AuthoringAction::ValidateContent { .. } => "authoring.validate-content",
                AuthoringAction::SaveRuntimeOutputs { .. } => "authoring.save-runtime-outputs",
                AuthoringAction::CreateContent { .. } => "authoring.create-content",
                AuthoringAction::CreateWorkspace { .. } => "authoring.create-workspace",
            },
            Self::Workshop(action) => match action {
                WorkshopAction::Search { .. } => "workshop.search",
                WorkshopAction::LoadSubscriptions => "workshop.load-subscriptions",
                WorkshopAction::Subscribe { .. } => "workshop.subscribe",
                WorkshopAction::Unsubscribe { .. } => "workshop.unsubscribe",
                WorkshopAction::Acquire { .. } => "workshop.acquire",
                WorkshopAction::Stage { .. } => "workshop.stage",
                WorkshopAction::Publish { .. } => "workshop.publish",
                WorkshopAction::DiscardReview => "workshop.discard-review",
            },
            Self::Publishing(action) => match action {
                PublishingAction::StageRoot { .. } => "publishing.stage-root",
                PublishingAction::PublishRoot { .. } => "publishing.publish-root",
                PublishingAction::DiscardRootReview => "publishing.discard-root-review",
            },
            Self::Developer(action) => match action {
                DeveloperAction::Format => "developer.format",
                DeveloperAction::Check => "developer.check",
                DeveloperAction::Test => "developer.test",
                DeveloperAction::Build => "developer.build",
                DeveloperAction::Clean { .. } => "developer.clean",
            },
            Self::Session(SessionAction::CheckpointLocalState { .. }) => {
                "session.checkpoint-local-state"
            }
        }
    }
}
