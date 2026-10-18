use serde::{Deserialize, Serialize};
use vapor_client_domain::WorkshopMetadataDraft;
pub use vapor_client_domain::{
    InstallPlanDependency as CompositionPreviewDependency,
    InstallPlanNode as CompositionPreviewNode,
    InstallPlanSource as CompositionPreviewSource,
};
pub use vapor_client_domain::ClientStateDocument as DurableStateSnapshot;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientRevision(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSummary {
    pub app_root: Option<String>,
    pub workspace: Option<String>,
    pub mode: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub steam_client_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableAction {
    pub id: String,
    pub enabled: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientSnapshot {
    pub revision: ClientRevision,
    pub host: HostSummary,
    pub active_composition_id: Option<String>,
    #[serde(default)]
    pub durable_state: DurableStateSnapshot,
    pub active_operation: Option<OperationSummary>,
    #[serde(default)]
    pub composition_preview: Option<CompositionPreviewSnapshot>,
    /// Complete retained catalog facts; a search never narrows this collection.
    pub catalog: Vec<CatalogEntry>,
    #[serde(default)]
    pub catalog_status: crate::CatalogStatus,
    #[serde(default)]
    pub catalog_search: crate::CatalogSearchSnapshot,
    #[serde(default)]
    pub workspace_content: WorkspaceContentSnapshot,
    #[serde(default)]
    pub superworkspace: SuperworkspaceSnapshot,
    #[serde(default)]
    pub workshop: WorkshopSnapshot,
    #[serde(default)]
    pub root_publishing: Option<RootPublishingSnapshot>,
    pub selected_content_id: Option<String>,
    pub available_actions: Vec<AvailableAction>,
}

/// Service-owned view of the canonical source container. This is independent
/// of the active authoring workspace and the selected playable composition.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuperworkspaceSnapshot {
    pub status: String,
    pub root: Option<String>,
    pub repositories: Vec<SuperworkspaceRepositorySnapshot>,
    #[serde(default)]
    pub available_repositories: Vec<AvailableSuperworkspaceRepositorySnapshot>,
    #[serde(default)]
    pub available_repositories_loaded: bool,
    #[serde(default)]
    pub available_repositories_error: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuperworkspaceRepositorySnapshot {
    pub name: String,
    pub root: String,
    pub kind: String,
}

/// A source repository offered by the configured Vapor registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableSuperworkspaceRepositorySnapshot {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub checkout_path: String,
    pub acquired: bool,
}

/// Service-owned preview and outcome for a first-party Steam root upload.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootPublishingSnapshot {
    pub review_id: String,
    pub planned: bool,
    pub app_id: u32,
    pub depot_id: u32,
    pub account: String,
    pub description: String,
    pub set_live: Option<String>,
    pub steamcmd: String,
    pub package_root: String,
    pub copied_files: usize,
    pub included_roots: Vec<String>,
    pub status: Option<String>,
    pub succeeded: bool,
}

/// Read-only view of the latest Workshop query and the account's subscriptions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkshopSnapshot {
    #[serde(default)]
    pub search: Option<WorkshopSearchSnapshot>,
    #[serde(default)]
    pub subscriptions: Vec<WorkshopSubscriptionSnapshot>,
    #[serde(default)]
    pub subscriptions_loaded: bool,
    #[serde(default)]
    pub subscriptions_revision: u64,
    #[serde(default)]
    pub acquisition: Option<WorkshopAcquisitionBatchSnapshot>,
    #[serde(default)]
    pub review: Option<WorkshopReviewSnapshot>,
}

/// Service-issued review token paired with the exact staged artifact facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkshopReviewSnapshot {
    pub review_id: String,
    pub selector: String,
    pub preview: WorkshopPackagePreviewSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkshopPackagePreviewSnapshot {
    pub id: String,
    pub version: Option<String>,
    pub app_id: Option<u32>,
    pub published_file_id: Option<String>,
    pub title: Option<String>,
    pub visibility: Option<String>,
    pub dependencies: Vec<String>,
    pub root: String,
    pub target: String,
    pub fingerprint: String,
    pub files: u64,
    pub bytes: u64,
    pub file_paths: Vec<String>,
    pub omitted_file_paths: usize,
}

/// Result of the latest successful import batch from Steam Workshop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkshopAcquisitionBatchSnapshot {
    pub revision: u64,
    pub requested: usize,
    pub items: Vec<WorkshopAcquisitionSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkshopAcquisitionSnapshot {
    pub id: String,
    pub version: Option<String>,
    pub kind: String,
    pub cache_root: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkshopSearchSnapshot {
    #[serde(default)]
    pub revision: u64,
    pub query: String,
    pub page: u32,
    pub total_results: u32,
    pub items: Vec<WorkshopItemSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkshopItemSnapshot {
    pub published_file_id: String,
    pub title: String,
    pub description: String,
    pub preview_url: Option<String>,
    pub updated_at_unix: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkshopSubscriptionSnapshot {
    pub published_file_id: String,
    pub title: String,
    pub description: String,
    pub preview_url: Option<String>,
    pub updated_at_unix: u32,
    pub installed: bool,
    pub needs_update: bool,
    pub downloading: bool,
    pub download_progress: Option<(u64, u64)>,
}

/// Authoritative discovery state for source projects in the active workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceContentSnapshot {
    pub source_projects: Vec<WorkspaceSourceProject>,
    pub source_projects_loaded: bool,
    pub source_projects_error: Option<String>,
    pub unregistered_projects: Vec<WorkspaceProjectCandidate>,
    pub unregistered_diagnostics: Vec<String>,
    pub unregistered_projects_loaded: bool,
    pub unregistered_projects_error: Option<String>,
    pub workspace_validation_error: Option<String>,
}

/// A content project declared by the active workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSourceProject {
    pub id: String,
    pub version: Option<String>,
    pub kind: String,
    pub root: String,
    pub manifest: String,
    pub binaries: Vec<String>,
    pub libraries: Vec<String>,
    pub dependencies: Vec<String>,
    pub workshop: WorkspaceWorkshopPolicy,
}

/// Workshop publication intent attached to a workspace source project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceWorkshopPolicy {
    pub app_id: Option<u32>,
    pub published_file_id: Option<String>,
    pub visibility: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub change_note: Option<String>,
}

/// A valid content project that can be explicitly adopted into the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceProjectCandidate {
    pub path: String,
    pub id: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    /// Human-readable content name, when declared by its publisher.
    #[serde(default)]
    pub name: Option<String>,
    pub version: Option<String>,
    pub kind: String,
    pub published_file_id: Option<String>,
    pub dependencies: Vec<String>,
    pub conflicts: Vec<CatalogConflict>,
    pub locations: Vec<CatalogLocation>,
    /// Product role used by every frontend to route this item through the
    /// correct lifecycle. This is projected by the service from Vapor's
    /// Rust-defined content vocabulary.
    #[serde(default)]
    pub role: CatalogItemRole,
    /// Service-derived lifecycle choices for the current catalog snapshot.
    #[serde(default)]
    pub actions: Vec<CatalogItemAction>,
}

/// Player/creator role of a catalog item, distinct from its Rust-defined kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CatalogItemRole {
    /// A complete Packagepack that can become a playable composition.
    PlayableComposition,
    /// Runtime content that may be installed into a composition.
    RuntimeContent,
    /// Rust-only code, available only through creator workflows.
    CreatorCrate,
    /// A recognized Vapor artifact that is not yet routed to a player action.
    #[default]
    Other,
}

/// Lifecycle action surfaced from service-owned catalog and composition facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CatalogItemAction {
    Install,
    Review,
    Play,
    SelectComposition,
    CreateComposition,
    AddToComposition,
    OpenForAuthoring,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogConflict {
    pub field: String,
    pub first_value: String,
    pub first_location: String,
    pub conflicting_value: String,
    pub conflicting_location: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogLocation {
    pub kind: CatalogLocationKind,
    pub root: String,
    pub enabled: bool,
    pub fingerprint: Option<String>,
    pub install_source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CatalogLocationKind {
    Workspace,
    Cache,
    Installed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationSummary {
    pub id: String,
    #[serde(default)]
    pub composition_id: Option<String>,
    pub action: String,
    pub phase: OperationPhase,
    pub status: OperationStatus,
    pub progress: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationEvent {
    pub revision: ClientRevision,
    pub id: String,
    #[serde(default)]
    pub composition_id: Option<String>,
    pub action: String,
    pub phase: OperationPhase,
    pub status: OperationStatus,
    pub progress: Option<f32>,
    pub message: Option<String>,
}

/// Result of a service-owned workspace editor operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", content = "payload", rename_all = "kebab-case")]
pub enum WorkspaceFileResult {
    Listed {
        operation_id: String,
        files: Vec<String>,
    },
    Opened {
        operation_id: String,
        path: String,
        contents: String,
        fingerprint: String,
    },
    Saved {
        operation_id: String,
        path: String,
        fingerprint: String,
    },
    Created {
        operation_id: String,
        path: String,
    },
    Completed {
        operation_id: String,
    },
    RememberedSources {
        operation_id: String,
        roots: Vec<String>,
    },
    UnregisteredContent {
        operation_id: String,
        content_id: String,
        version: Option<String>,
    },
}

/// Result payload for an authoring operation executed by the Studio service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthoringResult {
    pub operation_id: String,
    pub result: AuthoringResultKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", content = "payload", rename_all = "kebab-case")]
pub enum AuthoringResultKind {
    RegisteredSources {
        roots: Vec<String>,
        active: Option<String>,
    },
    Composition {
        id: String,
        version: Option<String>,
        kind: String,
        manifest: String,
        source: String,
        links: Vec<AuthoringCompositionLink>,
    },
    WorkshopMetadata {
        id: String,
        version: Option<String>,
        kind: String,
        manifest: String,
        source: String,
        policy: WorkshopMetadataDraft,
    },
    RuntimeOutputs {
        id: String,
        version: Option<String>,
        kind: String,
        manifest: String,
        source: String,
        binaries: Vec<String>,
        libraries: Vec<String>,
    },
    ContentValidated {
        id: String,
        #[serde(default)]
        summary: String,
    },
    ContentCreated {
        workspace_root: String,
        manifest: String,
        manifest_contents: String,
        files: Vec<String>,
    },
    WorkspaceCreated {
        root: String,
        files: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthoringCompositionLink {
    pub id: String,
    pub relationship: String,
    pub optional: bool,
    pub version: Option<String>,
    pub workshop_id: Option<String>,
    pub reason: Option<String>,
}

/// Resolved composition plan produced by the service for a playable composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositionPreviewSnapshot {
    pub composition_id: String,
    pub selector: String,
    pub nodes: Vec<CompositionPreviewNode>,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperationPhase {
    Queued,
    Resolving,
    Acquiring,
    Installing,
    Verifying,
    Building,
    Staging,
    Publishing,
    Running,
    Finalizing,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "detail", rename_all = "kebab-case")]
pub enum OperationStatus {
    Running,
    Succeeded,
    Failed { message: String },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "payload", rename_all = "kebab-case")]
pub enum ClientEvent {
    ServiceReady { protocol_version: u16 },
    Snapshot(Box<ClientSnapshot>),
    OperationChanged(OperationEvent),
    WorkspaceFileResult(Box<WorkspaceFileResult>),
    AuthoringResult(Box<AuthoringResult>),
    ServiceStopping,
    ProtocolError(crate::ServiceError),
}
