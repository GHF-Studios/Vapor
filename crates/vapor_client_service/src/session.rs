use crate::{
    catalog::CatalogState,
    host::ClientHostContext,
    persistence::{ClientPersistentState, ClientStateStore},
};
mod authoring;
mod catalog;
mod composition;
mod library;
mod operation_registry;
mod publishing;
mod snapshot;
mod workshop;
mod workspace;
#[cfg(test)]
use composition::fixture_composition_result;
use composition::{protocol_phase, protocol_status};
use operation_registry::{OperationRegistry, OperationReservationError, OperationResourceKey};
use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
};
use vapor_client::ClientHostConfig;
use vapor_client_application as application;
use vapor_client_protocol::{
    ActionProfile, ApplicationReleaseSnapshot, AuthoringAction, AuthoringCompositionLink,
    AuthoringResult, AuthoringResultKind, AvailableSuperworkspaceRepositorySnapshot, CatalogAction,
    CatalogLocationKind, ClientAction, ClientEvent, ClientRequest, ClientResponse,
    ClientResponseKind, ClientRevision, Composition, CompositionAction, ContentReference,
    DeveloperAction, InstallationRoleSnapshot, InstallationState, LegacyStorageSeed, LibraryAction,
    LocalCapability, OperationEvent, OperationOutputEvent, OperationPhase, OperationStatus,
    OperationSummary, ProfileReportSnapshot, ProfileTraceSnapshot, PublishingAction,
    RootPublishingSnapshot, ServiceError, ServiceErrorCode, SessionAction,
    SourceMetadataRepairAction, WorkshopAcquisitionBatchSnapshot, WorkshopAcquisitionSnapshot,
    WorkshopAction, WorkshopItemSnapshot, WorkshopMetadataDraft, WorkshopPackagePreviewSnapshot,
    WorkshopReviewSnapshot, WorkshopSearchSnapshot, WorkshopSnapshot, WorkshopSubscriptionSnapshot,
    WorkspaceAction, WorkspaceAssociation, WorkspaceContentSnapshot, WorkspaceFileResult,
    WorkspaceProjectSelection, WorkspaceToolOperation,
};
#[cfg(test)]
use vapor_client_protocol::{CatalogEntry, CatalogItemRole, CatalogLocation};
use workspace::is_isolated_fixture_workspace;

const MAX_WORKSPACE_PROJECTS: usize = 10_000;
const MAX_WORKSPACE_DIAGNOSTICS: usize = 1_000;
const MAX_WORKSHOP_REVIEW_FILE_PATHS: usize = 50;
// Output is also bounded at the provider reader and session event queue. This
// middle queue applies backpressure between those layers instead of retaining
// an unbounded burst when a frontend is slow to poll.
const MAX_PENDING_BACKGROUND_OPERATION_MESSAGES: usize = 256;
pub(super) const MAX_BUFFERED_OPERATION_OUTPUT_EVENTS: usize = 512;
const OPERATION_OUTPUT_OMISSION_NOTICE: &str =
    "Older output omitted because the service buffer reached its limit.";

fn operation_cancellation_error(message: &str) -> ServiceError {
    ServiceError {
        code: ServiceErrorCode::ActionUnavailable,
        message: message.to_owned(),
        retryable: true,
    }
}

fn action_capability_error(
    profile: ActionProfile,
    readiness: &InstallationRoleSnapshot,
) -> Option<ServiceError> {
    let missing = profile
        .local_capabilities
        .iter()
        .find(|capability| match capability {
            LocalCapability::ManagedRustToolchain => !readiness.rust_ready,
            LocalCapability::RustAnalyzer => !readiness.rust_analyzer_ready,
            LocalCapability::TracyProfiler => !readiness.tracy_profiler_ready,
        })?;
    let label = match missing {
        LocalCapability::ManagedRustToolchain => "Vapor-managed Rust/Cargo",
        LocalCapability::RustAnalyzer => "Vapor-managed rust-analyzer",
        LocalCapability::TracyProfiler => "Vapor-managed Tracy profiler",
    };
    Some(ServiceError {
        code: ServiceErrorCode::ActionUnavailable,
        message: readiness.error.clone().unwrap_or_else(|| {
            format!("{label} is not ready. Set it up in Vapor Installer first.")
        }),
        retryable: true,
    })
}

fn queue_operation_output(events: &mut VecDeque<ClientEvent>, output: OperationOutputEvent) {
    let buffered_outputs = events
        .iter()
        .filter(|event| matches!(event, ClientEvent::OperationOutput(_)))
        .count();
    let has_omission_notice = events.iter().any(|event| {
        matches!(event, ClientEvent::OperationOutput(output) if output.text == OPERATION_OUTPUT_OMISSION_NOTICE)
    });

    let mut to_evict = buffered_outputs
        .saturating_add(1)
        .saturating_sub(MAX_BUFFERED_OPERATION_OUTPUT_EVENTS);
    if to_evict == 0 {
        events.push_back(ClientEvent::OperationOutput(output));
        return;
    }
    if !has_omission_notice {
        to_evict += 1;
    }

    while to_evict > 0 {
        let Some(index) = events.iter().position(|event| {
            matches!(event, ClientEvent::OperationOutput(output) if output.text != OPERATION_OUTPUT_OMISSION_NOTICE)
        }) else {
            break;
        };
        events.remove(index);
        to_evict -= 1;
    }

    if !has_omission_notice {
        let notice_index = events
            .iter()
            .position(|event| matches!(event, ClientEvent::OperationOutput(_)))
            .unwrap_or(events.len());
        events.insert(
            notice_index,
            ClientEvent::OperationOutput(OperationOutputEvent {
                id: output.id.clone(),
                stream: vapor_client_protocol::OperationOutputStream::Stderr,
                text: OPERATION_OUTPUT_OMISSION_NOTICE.to_owned(),
            }),
        );
    }
    events.push_back(ClientEvent::OperationOutput(output));
}

#[cfg(test)]
fn library_action_can_invalidate_composition(action: &LibraryAction) -> bool {
    library::library_action_can_invalidate_composition(action)
}

pub struct ClientSession {
    bootstrap: ClientHostConfig,
    revision: ClientRevision,
    active_composition_id: Option<String>,
    selected_content_id: Option<String>,
    catalog: CatalogState,
    workspace_content: WorkspaceContentSnapshot,
    superworkspace_setup_error: Option<String>,
    superworkspace_sources: Vec<AvailableSuperworkspaceRepositorySnapshot>,
    superworkspace_sources_loaded: bool,
    superworkspace_sources_error: Option<String>,
    durable_state: ClientPersistentState,
    state_store: Option<ClientStateStore>,
    host_context: Option<ClientHostContext>,
    operation_registry: OperationRegistry,
    pending_catalog_refresh: bool,
    pending_workspace_discovery: bool,
    composition_preview: Option<vapor_client_protocol::CompositionPreviewSnapshot>,
    workshop: WorkshopSnapshot,
    workshop_review: Option<WorkshopReviewState>,
    workshop_batch_review: Option<WorkshopBatchReviewState>,
    root_publish_review: Option<RootPublishReviewState>,
    application_release_review: Option<application::ApplicationReleaseReview>,
    application_release_snapshot: Option<ApplicationReleaseSnapshot>,
    profile_traces: Vec<ProfileTraceSnapshot>,
    profile_trace_error: Option<String>,
    fixture_workshop_subscriptions: BTreeSet<String>,
    fixture_acquisitions: BTreeMap<String, WorkshopAcquisitionSnapshot>,
    operation_events: VecDeque<ClientEvent>,
    operation_receiver: Receiver<BackgroundOperationMessage>,
    operation_sender: SyncSender<BackgroundOperationMessage>,
}

enum BackgroundOperationMessage {
    Lifecycle(application::CoreOperationEvent),
    Panicked { id: String, message: String },
    Complete(Box<CompositionOperationCompletion>),
    CompleteWorkshop(Box<WorkshopOperationCompletion>),
    CompleteRootPublish(Box<RootPublishOperationCompletion>),
    CompleteApplicationRelease(Box<ApplicationReleaseOperationCompletion>),
    CompleteWorkspace(Box<WorkspaceOperationCompletion>),
    CompleteAuthoring(Box<AuthoringOperationCompletion>),
    CompleteLibrary(Box<LibraryOperationCompletion>),
    CompleteCatalogRefresh(Box<CatalogRefreshCompletion>),
    CompleteWorkspaceDiscovery(Box<WorkspaceDiscoveryCompletion>),
    CompleteProfileAnalysis(Box<ProfileAnalysisCompletion>),
    CompleteRoleTransition(Box<RoleTransitionCompletion>),
}

impl BackgroundOperationMessage {
    fn completion_id(&self) -> Option<&str> {
        match self {
            Self::Lifecycle(_) => None,
            Self::Panicked { id, .. } => Some(id),
            Self::Complete(completion) => Some(&completion.id),
            Self::CompleteWorkshop(completion) => Some(&completion.id),
            Self::CompleteRootPublish(completion) => Some(&completion.id),
            Self::CompleteApplicationRelease(completion) => Some(&completion.id),
            Self::CompleteWorkspace(completion) => Some(&completion.id),
            Self::CompleteAuthoring(completion) => Some(&completion.id),
            Self::CompleteLibrary(completion) => Some(&completion.id),
            Self::CompleteCatalogRefresh(completion) => Some(&completion.id),
            Self::CompleteWorkspaceDiscovery(completion) => Some(&completion.id),
            Self::CompleteProfileAnalysis(completion) => Some(&completion.id),
            Self::CompleteRoleTransition(completion) => Some(&completion.id),
        }
    }
}

fn background_operation_panic_message(payload: Box<dyn Any + Send>) -> String {
    let detail = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("unexpected panic");
    let detail = detail.chars().take(320).collect::<String>();
    format!("Operation stopped after an internal error: {detail}")
}

struct BackgroundOperationSpec {
    domain: &'static str,
    action: &'static str,
    thread_prefix: &'static str,
    failure_label: &'static str,
    composition_id: Option<String>,
    phase: OperationPhase,
}

struct LibraryOperationCompletion {
    id: String,
    action: LibraryAction,
    result: Result<String, String>,
}

struct CatalogRefreshCompletion {
    id: String,
    result: Result<Vec<vapor_client_protocol::CatalogEntry>, String>,
}

struct WorkspaceDiscoveryCompletion {
    id: String,
    workspace: PathBuf,
    result: vapor_client_protocol::WorkspaceContentSnapshot,
}

struct ProfileAnalysisCompletion {
    id: String,
    result: Result<ProfileReportSnapshot, String>,
}

struct RoleTransitionCompletion {
    id: String,
    result: Result<String, String>,
}

struct CompositionOperationCompletion {
    id: String,
    composition_id: String,
    action: CompositionOperationKind,
    inputs: CompositionOperationInputs,
    result: CompositionOperationResult,
}

/// Composition values whose change makes an in-flight composition result unsafe to apply.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CompositionOperationInputs {
    root_package: Option<ContentReference>,
    resolved_graph: Option<vapor_client_protocol::ResolvedCompositionGraph>,
    workspace: Option<WorkspaceAssociation>,
    launch_arguments: Option<Vec<String>>,
}

impl CompositionOperationInputs {
    fn capture(composition: &Composition, action: CompositionOperationKind) -> Self {
        Self {
            root_package: composition.root_package.clone(),
            resolved_graph: composition.resolved_graph.clone(),
            workspace: composition.workspace.clone(),
            launch_arguments: matches!(
                action,
                CompositionOperationKind::Launch | CompositionOperationKind::RunSource
            )
            .then(|| composition.launch.arguments.clone()),
        }
    }
}

#[derive(Clone, Copy)]
enum CompositionOperationKind {
    Preview,
    Install,
    Verify,
    Repair,
    Update,
    Launch,
    RunSource,
}

enum CompositionOperationResult {
    Preview(Result<vapor_client_application::CompositionPreview, String>),
    Install(Result<vapor_client_application::InstalledComposition, String>),
    Verify(Result<vapor_client_application::InstallationCheck, String>),
    Repair(Result<vapor_client_application::InstalledComposition, String>),
    Update(Result<vapor_client_application::InstalledComposition, String>),
    Launch(Result<vapor_client_application::CompositionRunResult, String>),
    RunSource(Result<vapor_client_application::CompositionRunResult, String>),
}

struct WorkshopOperationCompletion {
    id: String,
    action: WorkshopOperationKind,
    result: Result<WorkshopOperationResult, String>,
}

struct WorkshopAcquisitionResult {
    items: Vec<vapor_client_application::WorkshopAcquisition>,
    requested: usize,
    dependencies_acquired: usize,
    updates: Vec<AutoInstallCompositionUpdate>,
    auto_install_warning: Option<String>,
}

struct AutoInstallCompositionUpdate {
    composition_id: String,
    inputs: CompositionOperationInputs,
    result: Result<vapor_client_application::InstalledComposition, String>,
}

#[derive(Debug, Clone, Default)]
struct AutoInstallContext {
    constraints: BTreeMap<String, Vec<String>>,
    compositions: BTreeMap<String, Vec<AutoInstallCompositionTarget>>,
    warning: Option<String>,
}

#[derive(Debug, Clone)]
struct AutoInstallCompositionTarget {
    composition: Composition,
    inputs: CompositionOperationInputs,
}

struct RootPublishReviewState {
    request: vapor_sdk_core::root::types::RootPublishRequest,
    snapshot: RootPublishingSnapshot,
}

struct RootPublishOperationCompletion {
    id: String,
    review_id: String,
    staged: bool,
    request: vapor_sdk_core::root::types::RootPublishRequest,
    result: Result<vapor_sdk_core::root::types::RootPublishReport, String>,
}

enum ApplicationReleaseTask {
    Stage {
        account: String,
        branch: Option<String>,
        description: String,
        runtime_targets: Vec<String>,
        build_release: bool,
    },
    Publish(application::ApplicationReleaseReview),
}

struct ApplicationReleaseOperationCompletion {
    id: String,
    review_id: String,
    staged: bool,
    result: Result<application::ApplicationReleaseReview, String>,
}

struct WorkspaceOperationCompletion {
    id: String,
    action: WorkspaceOperationKind,
    root: PathBuf,
    result: Result<WorkspaceOperationResult, String>,
}

struct AuthoringOperationCompletion {
    id: String,
    action: String,
    workspace: Option<PathBuf>,
    workspace_bound: bool,
    result: Result<AuthoringResultKind, String>,
}

#[derive(Clone)]
enum WorkspaceOperationKind {
    DiscoverSuperworkspaceRepositories,
    AcquireSuperworkspaceRepositories(Vec<String>),
    ListFiles,
    OpenFile(PathBuf),
    SaveFile(Box<WorkspaceFileSave>),
    CreateFile(PathBuf),
    Validate,
    BuildDocumentation,
    PublishDocumentation,
    RunTool(
        WorkspaceToolOperation,
        WorkspaceProjectSelection,
        Vec<String>,
    ),
    RunConfiguration {
        name: String,
        profiling: bool,
    },
    DeployApplication {
        target_triples: Vec<String>,
        skip_documentation: bool,
    },
    PackageApplication {
        target_triples: Vec<String>,
    },
    Clean,
    Remember,
    AdoptProject(PathBuf),
    UnregisterProject(String),
}

#[derive(Clone)]
struct WorkspaceFileSave {
    path: PathBuf,
    expected_fingerprint: String,
    contents: String,
}

enum WorkspaceOperationResult {
    SourceRepositories(Vec<AvailableSuperworkspaceRepositorySnapshot>),
    AcquiredSourceRepositories(Vec<String>),
    Listed(Vec<PathBuf>),
    Opened(Box<WorkspaceFileOpen>),
    Saved { path: PathBuf, fingerprint: String },
    Created(PathBuf),
    Completed,
    CompletedWithSummary(String),
    Remembered(Vec<PathBuf>),
    Unregistered(vapor_client_application::ContentReference),
}

struct WorkspaceFileOpen {
    path: PathBuf,
    contents: String,
    fingerprint: String,
}

#[derive(Debug, Clone)]
struct WorkshopReviewState {
    review_id: String,
    selector: String,
    target_triples: Vec<String>,
    workspace_root: PathBuf,
    preview: vapor_client_application::WorkshopPackagePreview,
}

#[derive(Debug, Clone)]
struct WorkshopBatchReviewState {
    review_id: String,
    target_triples: Vec<String>,
    workspace_root: PathBuf,
    preview: application::WorkshopBatchReview,
}

#[derive(Clone)]
enum WorkshopOperationKind {
    Search {
        query: String,
        page: u32,
        content_kind: Option<vapor_client_protocol::ContentKind>,
        required_tags: Vec<String>,
    },
    LoadSubscriptions,
    ChangeSubscriptions {
        ids: Vec<u64>,
        subscribe: bool,
    },
    Acquire {
        ids: Vec<String>,
    },
    Stage {
        selector: String,
        target_triples: Vec<String>,
        workspace_root: PathBuf,
    },
    StageBatch {
        selectors: Vec<String>,
        target_triples: Vec<String>,
        workspace_root: PathBuf,
    },
    Publish {
        review: Box<WorkshopReviewState>,
        account: String,
        change_note: Option<String>,
    },
    PublishBatch {
        review: Box<WorkshopBatchReviewState>,
        account: String,
        change_note: Option<String>,
    },
}

enum WorkshopOperationResult {
    Search(vapor_client_application::WorkshopPage),
    Subscriptions(Vec<vapor_client_application::WorkshopSubscription>),
    SubscriptionWithAcquisition {
        subscriptions: Vec<vapor_client_application::WorkshopSubscription>,
        acquisition: Result<WorkshopAcquisitionResult, String>,
    },
    Acquisitions(WorkshopAcquisitionResult),
    Review(vapor_client_application::WorkshopPackagePreview),
    BatchReview(vapor_client_application::WorkshopBatchReview),
    Publication(vapor_client_application::WorkshopPublication),
    BatchPublication(Vec<vapor_client_application::WorkshopPublication>),
}

impl ClientSession {
    pub fn new(bootstrap: ClientHostConfig) -> Self {
        let mut durable_state = ClientPersistentState::default();
        if let Some(active_composition_id) = bootstrap.active_composition_id.as_ref()
            && durable_state
                .compositions
                .compositions
                .iter()
                .any(|composition| composition.id == *active_composition_id)
        {
            durable_state.active_composition_id = active_composition_id.clone();
        }
        durable_state.authoring_workspace_root = bootstrap
            .workspace_root
            .as_ref()
            .map(|root| root.display().to_string());
        Self::with_state(bootstrap, durable_state, None, None, None)
    }

    pub fn with_persistent_state(
        mut bootstrap: ClientHostConfig,
        host_context: ClientHostContext,
        store: ClientStateStore,
        legacy_storage: Option<&LegacyStorageSeed>,
    ) -> Result<Self, String> {
        ensure_workshop_app_id(&mut bootstrap);
        let mut loaded = store.load_or_import(legacy_storage)?;
        if !bootstrap.fixture_mode
            && loaded.recovery_message.is_none()
            && seed_legacy_empty_selection(
                &mut loaded.state,
                application::default_packagepack_selector(
                    &bootstrap.application_executable,
                    "default",
                )
                .ok()
                .as_deref(),
            )
        {
            store.save(&loaded.state)?;
        }
        if let Some(workspace_root) = bootstrap.workspace_root.as_ref() {
            let workspace = Some(workspace_root.display().to_string());
            if loaded.state.authoring_workspace_root != workspace {
                loaded.state.authoring_workspace_root = workspace;
                store.save(&loaded.state)?;
            }
        }
        if bootstrap.workspace_root.is_none()
            && let Some(root) = loaded.state.authoring_workspace_root.as_deref()
        {
            let root = PathBuf::from(root);
            if is_installed_application_root(&root) {
                // The installed app root is runtime content. A persisted
                // source selection must never turn it into an authoring
                // workspace just because every frontend shares this state.
                loaded.state.authoring_workspace_root = None;
                store.save(&loaded.state)?;
            } else {
                bootstrap.workspace_root = Some(root);
            }
        }
        let superworkspace_setup_error = if bootstrap.fixture_mode {
            None
        } else {
            match vapor_core::VaporInstallation::discover() {
                Ok(installation) => vapor_core::ensure_canonical_superworkspace(&installation)
                    .err()
                    .map(|error| error.to_string()),
                Err(error) => Some(error.to_string()),
            }
        };
        bootstrap.active_composition_id = Some(loaded.state.active_composition_id.clone());
        let mut session = Self::with_state(
            bootstrap,
            loaded.state,
            Some(store),
            Some(host_context),
            superworkspace_setup_error,
        );
        if let Some(message) = loaded.recovery_message {
            session.catalog.fail(message);
        } else {
            session.refresh_all_content_views();
        }
        Ok(session)
    }

    fn with_state(
        bootstrap: ClientHostConfig,
        durable_state: ClientPersistentState,
        state_store: Option<ClientStateStore>,
        host_context: Option<ClientHostContext>,
        superworkspace_setup_error: Option<String>,
    ) -> Self {
        let active_composition_id = Some(durable_state.active_composition_id.clone());
        let (operation_sender, operation_receiver) =
            mpsc::sync_channel(MAX_PENDING_BACKGROUND_OPERATION_MESSAGES);
        Self {
            bootstrap,
            revision: ClientRevision(0),
            active_composition_id,
            selected_content_id: None,
            catalog: CatalogState::default(),
            workspace_content: WorkspaceContentSnapshot::default(),
            superworkspace_setup_error,
            superworkspace_sources: Vec::new(),
            superworkspace_sources_loaded: false,
            superworkspace_sources_error: None,
            durable_state,
            state_store,
            host_context,
            operation_registry: OperationRegistry::default(),
            pending_catalog_refresh: false,
            pending_workspace_discovery: false,
            composition_preview: None,
            workshop: WorkshopSnapshot::default(),
            workshop_review: None,
            workshop_batch_review: None,
            root_publish_review: None,
            application_release_review: None,
            application_release_snapshot: None,
            profile_traces: Vec::new(),
            profile_trace_error: None,
            fixture_workshop_subscriptions: BTreeSet::from(["1001".to_owned()]),
            fixture_acquisitions: BTreeMap::new(),
            operation_events: VecDeque::new(),
            operation_receiver,
            operation_sender,
        }
    }

    pub fn has_active_operation(&self) -> bool {
        !self.operation_registry.is_empty()
    }

    pub(super) fn finish_operation(
        &mut self,
        operation_id: &str,
        status: OperationStatus,
    ) -> Option<OperationSummary> {
        let completed = self.operation_registry.complete(operation_id, status);
        if completed.is_some() {
            if self.pending_catalog_refresh
                && !self.bootstrap.fixture_mode
                && self.start_catalog_refresh_operation().is_ok()
            {
                self.pending_catalog_refresh = false;
            }
            if self.pending_workspace_discovery
                && !self.bootstrap.fixture_mode
                && self.start_workspace_discovery_operation().is_ok()
            {
                self.pending_workspace_discovery = false;
            }
        }
        completed
    }

    fn remove_operation(&mut self, operation_id: &str) {
        self.operation_registry.remove(operation_id);
    }

    fn operation_resources(&self, spec: &BackgroundOperationSpec) -> Vec<OperationResourceKey> {
        let workspace = self.bootstrap.workspace_root.clone().unwrap_or_default();
        if spec.domain == "studio-composition" && spec.action == "composition.run-source" {
            return vec![
                OperationResourceKey::Composition(
                    spec.composition_id
                        .clone()
                        .or_else(|| self.active_composition_id.clone())
                        .unwrap_or_else(|| "selected".to_owned()),
                ),
                OperationResourceKey::Workspace(workspace),
            ];
        }
        let resource = match spec.domain {
            "studio-composition" => OperationResourceKey::Composition(
                spec.composition_id
                    .clone()
                    .or_else(|| self.active_composition_id.clone())
                    .unwrap_or_else(|| "selected".to_owned()),
            ),
            "studio-authoring" if spec.action == "authoring.list-registered-sources" => {
                OperationResourceKey::WorkspaceScan(workspace)
            }
            "studio-authoring" if spec.action == "authoring.discover-content-projects" => {
                OperationResourceKey::WorkspaceScan(workspace)
            }
            "studio-workspace"
                if matches!(spec.action, "workspace.save-file" | "workspace.create-file") =>
            {
                OperationResourceKey::WorkspaceEdit(workspace)
            }
            "studio-authoring" | "studio-workspace" => OperationResourceKey::Workspace(workspace),
            "studio-workshop" => OperationResourceKey::WorkshopAccount,
            "vapor-library" => OperationResourceKey::CatalogRefresh,
            "studio-root-publishing" | "application-release" => OperationResourceKey::Publishing,
            "installation-role" => OperationResourceKey::InstallationTools,
            "profiling" => OperationResourceKey::Profiling,
            domain => OperationResourceKey::Exclusive(domain.to_owned()),
        };
        vec![resource]
    }

    pub(in crate::session) fn can_start_operation(&self, resource: OperationResourceKey) -> bool {
        self.operation_registry
            .conflicts_with(&[resource])
            .is_none()
    }

    fn validate_action_capabilities(&self, action: &ClientAction) -> Result<(), ServiceError> {
        if self.bootstrap.fixture_mode {
            return Ok(());
        }

        let profile = action.profile();
        if profile.local_capabilities.is_empty() {
            return Ok(());
        }

        let readiness = application::installation_role_snapshot();
        if let Some(error) = action_capability_error(profile, &readiness) {
            return Err(error);
        }

        // Authority requirements are represented for shared UI projection, but
        // are resolved by the concrete publish handler against its account and
        // target before any external effect occurs.
        Ok(())
    }

    pub fn dispatch(&mut self, request: ClientRequest) -> ClientResponse {
        self.poll_background_operation_messages();
        let active_operation_ids_before = self
            .operation_registry
            .active_summaries()
            .into_iter()
            .map(|operation| operation.id)
            .collect::<std::collections::HashSet<_>>();
        let is_registered_source_list = is_registered_source_list_query(&request.action);
        let mut response_kind = ClientResponseKind::Applied;
        let mut error = None;
        let is_cancel_request = matches!(
            &request.action,
            ClientAction::Session(SessionAction::CancelOperation { .. })
        );
        if !is_cancel_request
            && !is_registered_source_list_query(&request.action)
            && !action_can_rebase_on_current_state(&request.action)
            && request
                .expected_revision
                .is_some_and(|expected| expected != self.revision)
        {
            response_kind = ClientResponseKind::Rejected;
            error = Some(ServiceError {
                code: ServiceErrorCode::StaleRevision,
                message: format!(
                    "Vapor Client state changed from revision {} to {}; refresh before retrying.",
                    request.expected_revision.map_or(0, |revision| revision.0),
                    self.revision.0
                ),
                retryable: true,
            });
        } else if let Err(prerequisite_error) = self.validate_action_capabilities(&request.action) {
            response_kind = ClientResponseKind::Rejected;
            error = Some(prerequisite_error);
        } else {
            match request.action {
                ClientAction::Catalog(CatalogAction::Refresh) => {
                    if self.bootstrap.fixture_mode {
                        self.refresh_catalog();
                        self.revision.0 = self.revision.0.saturating_add(1);
                    } else {
                        match self.start_catalog_refresh_operation() {
                            Ok(()) => {
                                response_kind = ClientResponseKind::OperationAccepted;
                                self.revision.0 = self.revision.0.saturating_add(1);
                            }
                            Err(operation_error) => {
                                response_kind = ClientResponseKind::Rejected;
                                error = Some(operation_error);
                            }
                        }
                    }
                }
                ClientAction::Catalog(CatalogAction::DiscoverContentProjects)
                    if self.bootstrap.workspace_root.is_some() =>
                {
                    if self.bootstrap.fixture_mode {
                        self.refresh_workspace_content();
                        self.revision.0 = self.revision.0.saturating_add(1);
                    } else {
                        match self.start_workspace_discovery_operation() {
                            Ok(()) => {
                                response_kind = ClientResponseKind::OperationAccepted;
                                self.revision.0 = self.revision.0.saturating_add(1);
                            }
                            Err(operation_error) => {
                                response_kind = ClientResponseKind::Rejected;
                                error = Some(operation_error);
                            }
                        }
                    }
                }
                ClientAction::Catalog(CatalogAction::DiscoverContentProjects) => {
                    response_kind = ClientResponseKind::Rejected;
                    error = Some(ServiceError {
                        code: ServiceErrorCode::ActionUnavailable,
                        message: "Open a workspace before discovering its content projects."
                            .to_owned(),
                        retryable: false,
                    });
                }
                ClientAction::Workspace(WorkspaceAction::InitializeSuperworkspace) => {
                    if self.bootstrap.fixture_mode {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(ServiceError {
                            code: ServiceErrorCode::ActionUnavailable,
                            message: "Fixture sessions cannot initialize a real Superworkspace."
                                .to_owned(),
                            retryable: false,
                        });
                    } else {
                        let result = vapor_core::VaporInstallation::discover()
                            .map_err(|failure| failure.to_string())
                            .and_then(|installation| {
                                vapor_core::ensure_canonical_superworkspace(&installation)
                                    .map(|_| ())
                                    .map_err(|failure| failure.to_string())
                            });
                        match result {
                            Ok(()) => self.superworkspace_setup_error = None,
                            Err(message) => {
                                self.superworkspace_setup_error = Some(message.clone());
                                response_kind = ClientResponseKind::Rejected;
                                error = Some(ServiceError {
                                    code: ServiceErrorCode::ProviderFailure,
                                    message,
                                    retryable: true,
                                });
                            }
                        }
                        self.revision.0 = self.revision.0.saturating_add(1);
                    }
                }
                ClientAction::Workspace(WorkspaceAction::SetSuperworkspaceLocation { root }) => {
                    if self.bootstrap.fixture_mode {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(ServiceError {
                            code: ServiceErrorCode::ActionUnavailable,
                            message:
                                "Fixture sessions cannot change a real Superworkspace location."
                                    .to_owned(),
                            retryable: false,
                        });
                    } else {
                        let result = vapor_core::VaporInstallation::discover()
                            .map_err(|failure| failure.to_string())
                            .and_then(|installation| {
                                vapor_core::configure_superworkspace(
                                    &installation,
                                    Path::new(&root),
                                )
                                .map(|_| ())
                                .map_err(|failure| failure.to_string())
                            });
                        match result {
                            Ok(()) => self.superworkspace_setup_error = None,
                            Err(message) => {
                                self.superworkspace_setup_error = Some(message.clone());
                                response_kind = ClientResponseKind::Rejected;
                                error = Some(ServiceError {
                                    code: ServiceErrorCode::InvalidRequest,
                                    message,
                                    retryable: true,
                                });
                            }
                        }
                        self.revision.0 = self.revision.0.saturating_add(1);
                    }
                }
                ClientAction::Workspace(
                    action @ (WorkspaceAction::DiscoverSuperworkspaceRepositories
                    | WorkspaceAction::AcquireSuperworkspaceRepositories { .. }),
                ) => match self.start_workspace_operation(action) {
                    Ok(()) => {
                        response_kind = ClientResponseKind::OperationAccepted;
                        self.revision.0 = self.revision.0.saturating_add(1);
                    }
                    Err(operation_error) => {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(operation_error);
                    }
                },
                ClientAction::Workspace(vapor_client_protocol::WorkspaceAction::Open { root }) => {
                    let requested = PathBuf::from(root);
                    if self.bootstrap.fixture_mode
                        && (!requested.is_absolute()
                            || !requested.starts_with(std::env::temp_dir()))
                    {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(ServiceError {
                            code: ServiceErrorCode::ActionUnavailable,
                            message:
                                "Fixture mode accepts only absolute temporary test workspace paths."
                                    .to_owned(),
                            retryable: false,
                        });
                    } else {
                        match std::fs::canonicalize(&requested) {
                            Ok(root)
                                if root.is_dir()
                                    && (!self.bootstrap.fixture_mode
                                        || is_isolated_fixture_workspace(&root)) =>
                            {
                                let previous_state = self.durable_state.clone();
                                let previous_workspace = self.bootstrap.workspace_root.clone();
                                let root_changed =
                                    self.bootstrap.workspace_root.as_deref() != Some(&root);
                                self.bootstrap.workspace_root = Some(root);
                                self.durable_state.authoring_workspace_root = self
                                    .bootstrap
                                    .workspace_root
                                    .as_ref()
                                    .map(|root| root.display().to_string());
                                if let Some(store) = &self.state_store
                                    && let Err(message) = store.save(&self.durable_state)
                                {
                                    self.durable_state = previous_state;
                                    self.bootstrap.workspace_root = previous_workspace;
                                    response_kind = ClientResponseKind::Rejected;
                                    error = Some(ServiceError {
                                        code: ServiceErrorCode::PersistenceFailure,
                                        message,
                                        retryable: true,
                                    });
                                    let snapshot = self.snapshot();
                                    return ClientResponse {
                                        protocol_version: vapor_client_protocol::PROTOCOL_VERSION,
                                        request_id: request.request_id,
                                        revision: snapshot.revision,
                                        kind: response_kind,
                                        operation_id: None,
                                        snapshot,
                                        error,
                                    };
                                }
                                self.selected_content_id = None;
                                if root_changed {
                                    self.catalog.reset();
                                    self.workspace_content = WorkspaceContentSnapshot::default();
                                    self.application_release_review = None;
                                    self.application_release_snapshot = None;
                                }
                                self.refresh_catalog();
                                self.refresh_workspace_content();
                                self.revision.0 = self.revision.0.saturating_add(1);
                            }
                            Ok(root) if self.bootstrap.fixture_mode && root.is_dir() => {
                                response_kind = ClientResponseKind::Rejected;
                                error = Some(ServiceError {
                                code: ServiceErrorCode::ActionUnavailable,
                                message: "Fixture mode can open only temporary test workspaces; it cannot inspect a player workspace.".to_owned(),
                                retryable: false,
                            });
                            }
                            Ok(root) => {
                                response_kind = ClientResponseKind::Rejected;
                                error = Some(ServiceError {
                                    code: ServiceErrorCode::InvalidRequest,
                                    message: format!(
                                        "{} is not a workspace folder",
                                        root.display()
                                    ),
                                    retryable: true,
                                });
                            }
                            Err(failure) => {
                                response_kind = ClientResponseKind::Rejected;
                                error = Some(ServiceError {
                                    code: ServiceErrorCode::InvalidRequest,
                                    message: format!("{}: {failure}", requested.display()),
                                    retryable: true,
                                });
                            }
                        }
                    }
                }
                ClientAction::Workspace(
                    action @ (WorkspaceAction::ListFiles { .. }
                    | WorkspaceAction::OpenFile { .. }
                    | WorkspaceAction::SaveFile { .. }
                    | WorkspaceAction::CreateFile { .. }
                    | WorkspaceAction::Validate
                    | WorkspaceAction::BuildDocumentation
                    | WorkspaceAction::PublishDocumentation
                    | WorkspaceAction::RunTool { .. }
                    | WorkspaceAction::RunConfiguration { .. }
                    | WorkspaceAction::DeployApplication { .. }
                    | WorkspaceAction::PackageApplication { .. }
                    | WorkspaceAction::Clean { .. }
                    | WorkspaceAction::Remember
                    | WorkspaceAction::AdoptProject { .. }
                    | WorkspaceAction::UnregisterProject { .. }),
                ) => match self.start_workspace_operation(action) {
                    Ok(()) => {
                        response_kind = ClientResponseKind::OperationAccepted;
                        self.revision.0 = self.revision.0.saturating_add(1);
                    }
                    Err(operation_error) => {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(operation_error);
                    }
                },
                ClientAction::Authoring(action) => {
                    let active_source_list =
                        matches!(&action, AuthoringAction::ListRegisteredSources)
                            && self
                                .operation_registry
                                .has_action("authoring.list-registered-sources");
                    if active_source_list {
                        // Several frontends can ask for the initial source list
                        // at once. Reuse its in-flight read instead of making
                        // the second startup fail on a stale revision.
                        response_kind = ClientResponseKind::OperationAccepted;
                    } else {
                        match self.start_authoring_operation(action) {
                            Ok(()) => {
                                response_kind = ClientResponseKind::OperationAccepted;
                                self.revision.0 = self.revision.0.saturating_add(1);
                            }
                            Err(operation_error) => {
                                response_kind = ClientResponseKind::Rejected;
                                error = Some(operation_error);
                            }
                        }
                    }
                }
                ClientAction::Library(LibraryAction::SetAutoInstall {
                    content_id,
                    enabled,
                }) => {
                    if !self.bootstrap.fixture_mode {
                        match self.start_library_operation(LibraryAction::SetAutoInstall {
                            content_id: content_id.clone(),
                            enabled,
                        }) {
                            Ok(()) => {
                                response_kind = ClientResponseKind::OperationAccepted;
                                self.revision.0 = self.revision.0.saturating_add(1);
                            }
                            Err(operation_error) => {
                                response_kind = ClientResponseKind::Rejected;
                                error = Some(operation_error);
                            }
                        }
                    } else {
                        let parsed = vapor_client_protocol::ContentReference::parse(
                            content_id.clone(),
                            None,
                        )
                        .and_then(|reference| {
                            reference.vapor_id().map_err(|error| error.to_string())
                        });
                        match parsed {
                            Err(message) => {
                                response_kind = ClientResponseKind::Rejected;
                                error = Some(ServiceError {
                                    code: ServiceErrorCode::InvalidRequest,
                                    message: format!(
                                        "invalid content ID `{content_id}`: {message}"
                                    ),
                                    retryable: false,
                                });
                            }
                            Ok(content_id) => {
                                if enabled
                                    && let Err(message) =
                                        self.validate_auto_install_target(&content_id.to_string())
                                {
                                    response_kind = ClientResponseKind::Rejected;
                                    error = Some(ServiceError {
                                        code: ServiceErrorCode::ActionUnavailable,
                                        message,
                                        retryable: false,
                                    });
                                    let snapshot = self.snapshot();
                                    return ClientResponse {
                                        protocol_version: vapor_client_protocol::PROTOCOL_VERSION,
                                        request_id: request.request_id,
                                        revision: snapshot.revision,
                                        kind: response_kind,
                                        operation_id: None,
                                        snapshot,
                                        error,
                                    };
                                }
                                let previous_state = self.durable_state.clone();
                                vapor_client_domain::set_library_auto_install(
                                    &mut self.durable_state.library_policies,
                                    content_id,
                                    enabled,
                                );
                                if let Some(store) = &self.state_store
                                    && let Err(message) = store.save(&self.durable_state)
                                {
                                    self.durable_state = previous_state;
                                    response_kind = ClientResponseKind::Rejected;
                                    error = Some(ServiceError {
                                        code: ServiceErrorCode::PersistenceFailure,
                                        message,
                                        retryable: true,
                                    });
                                } else if enabled {
                                    if let Err(message) = self.collect_unreferenced_auto_install() {
                                        self.durable_state = previous_state;
                                        if let Some(store) = &self.state_store {
                                            let _ = store.save(&self.durable_state);
                                        }
                                        response_kind = ClientResponseKind::Rejected;
                                        error = Some(ServiceError {
                                            code: ServiceErrorCode::ProviderFailure,
                                            message,
                                            retryable: true,
                                        });
                                    } else {
                                        self.revision.0 = self.revision.0.saturating_add(1);
                                    }
                                } else {
                                    self.revision.0 = self.revision.0.saturating_add(1);
                                }
                            }
                        }
                    }
                }
                ClientAction::Library(action) => match self.start_library_operation(action) {
                    Ok(()) => {
                        response_kind = ClientResponseKind::OperationAccepted;
                        self.revision.0 = self.revision.0.saturating_add(1);
                    }
                    Err(operation_error) => {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(operation_error);
                    }
                },
                ClientAction::Catalog(CatalogAction::Search { query }) => {
                    self.catalog.set_query(&query);
                    self.revision.0 = self.revision.0.saturating_add(1);
                }
                ClientAction::Catalog(CatalogAction::Inspect {
                    content_id,
                    version,
                }) => {
                    if let Some(entry) = self
                        .catalog
                        .entries()
                        .iter()
                        .find(|entry| entry.id == content_id && entry.version == version)
                    {
                        self.selected_content_id = Some(match entry.version.as_deref() {
                            Some(version) => format!("{}@{version}", entry.id),
                            None => entry.id.clone(),
                        });
                        self.revision.0 = self.revision.0.saturating_add(1);
                    } else {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(ServiceError {
                            code: ServiceErrorCode::ActionUnavailable,
                            message:
                                "That content identity is not in the current catalog snapshot."
                                    .to_owned(),
                            retryable: true,
                        });
                    }
                }
                ClientAction::Session(SessionAction::CheckpointLocalState {
                    editor_drafts,
                    operation_history,
                }) => {
                    let previous_state = self.durable_state.clone();
                    self.durable_state.editor_drafts = editor_drafts;
                    self.durable_state.operation_history = operation_history;
                    if let Some(store) = &self.state_store
                        && let Err(message) = store.save(&self.durable_state)
                    {
                        self.durable_state = previous_state;
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(ServiceError {
                            code: ServiceErrorCode::PersistenceFailure,
                            message,
                            retryable: true,
                        });
                    } else {
                        self.revision.0 = self.revision.0.saturating_add(1);
                    }
                }
                ClientAction::Session(SessionAction::CancelOperation { operation_id }) => {
                    let Some(active) = self
                        .operation_registry
                        .get(&operation_id)
                        .map(|operation| operation.summary.clone())
                    else {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(operation_cancellation_error(
                            "That operation is no longer active.",
                        ));
                        let snapshot = self.snapshot();
                        return ClientResponse {
                            protocol_version: vapor_client_protocol::PROTOCOL_VERSION,
                            request_id: request.request_id,
                            revision: snapshot.revision,
                            kind: response_kind,
                            operation_id: None,
                            snapshot,
                            error,
                        };
                    };
                    if !self.operation_registry.is_cancellable(&operation_id) {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(operation_cancellation_error(
                            "This operation cannot be stopped safely once it has started.",
                        ));
                    } else if let Some(cancellation) =
                        self.operation_registry.cancellation(&operation_id)
                    {
                        cancellation.cancel();
                        self.operation_registry
                            .set_status(&operation_id, OperationStatus::Cancelling);
                        self.revision.0 = self.revision.0.saturating_add(1);
                        self.operation_events
                            .push_back(ClientEvent::OperationChanged(OperationEvent {
                                revision: self.revision,
                                id: active.id,
                                composition_id: active.composition_id,
                                action: active.action,
                                phase: OperationPhase::Finalizing,
                                status: OperationStatus::Cancelling,
                                progress: active.progress,
                                message: Some("Cancellation requested.".to_owned()),
                            }));
                    } else {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(operation_cancellation_error(
                            "This operation has no active cancellation handle.",
                        ));
                    }
                }
                ClientAction::Developer(DeveloperAction::RefreshInstallationRole) => {
                    self.revision.0 = self.revision.0.saturating_add(1);
                }
                ClientAction::Developer(
                    action @ (DeveloperAction::PromoteInstallationRole { .. }
                    | DeveloperAction::DemoteInstallationRole { .. }),
                ) => {
                    let (target, promote) = match action {
                        DeveloperAction::PromoteInstallationRole { target } => (target, true),
                        DeveloperAction::DemoteInstallationRole { target } => (target, false),
                        _ => unreachable!("the matched action is a role transition"),
                    };
                    if self.bootstrap.fixture_mode {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(ServiceError {
                            code: ServiceErrorCode::ActionUnavailable,
                            message: "Installation roles are unavailable in fixture sessions."
                                .to_owned(),
                            retryable: false,
                        });
                    } else {
                        let operation = if promote {
                            "developer.promote-installation-role"
                        } else {
                            "developer.demote-installation-role"
                        };
                        let execute_target = target.clone();
                        let execute = if promote {
                            application::promote_installation_role
                                as fn(&str) -> Result<String, String>
                        } else {
                            application::demote_installation_role
                                as fn(&str) -> Result<String, String>
                        };
                        let spawn = self.spawn_background_operation(
                            BackgroundOperationSpec {
                                domain: "installation-role",
                                action: operation,
                                thread_prefix: "vapor-role-transition",
                                failure_label: "installation role change",
                                composition_id: None,
                                phase: OperationPhase::Installing,
                            },
                            move |_| execute(&execute_target),
                            |result| result.clone(),
                            |id, result| {
                                BackgroundOperationMessage::CompleteRoleTransition(Box::new(
                                    RoleTransitionCompletion { id, result },
                                ))
                            },
                        );
                        match spawn {
                            Ok(()) => {
                                response_kind = ClientResponseKind::OperationAccepted;
                                self.revision.0 = self.revision.0.saturating_add(1);
                            }
                            Err(failure) => {
                                response_kind = ClientResponseKind::Rejected;
                                error = Some(failure);
                            }
                        }
                    }
                }
                ClientAction::Developer(DeveloperAction::SubscribeTelemetry { .. }) => {}
                ClientAction::Developer(DeveloperAction::RefreshProfileTraces) => {
                    if self.bootstrap.fixture_mode {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(ServiceError {
                            code: ServiceErrorCode::ActionUnavailable,
                            message: "Fixture sessions cannot access the managed trace library."
                                .to_owned(),
                            retryable: false,
                        });
                    } else {
                        match application::list_managed_profile_traces() {
                            Ok(records) => {
                                self.profile_traces = records;
                                self.profile_trace_error = None;
                                self.revision.0 = self.revision.0.saturating_add(1);
                            }
                            Err(message) => {
                                self.profile_trace_error = Some(message.clone());
                                response_kind = ClientResponseKind::Rejected;
                                error = Some(ServiceError {
                                    code: ServiceErrorCode::ProviderFailure,
                                    message,
                                    retryable: true,
                                });
                            }
                        }
                    }
                }
                ClientAction::Developer(DeveloperAction::ImportProfileTrace { path }) => {
                    if self.bootstrap.fixture_mode {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(ServiceError {
                            code: ServiceErrorCode::ActionUnavailable,
                            message: "Fixture sessions cannot access the managed trace library."
                                .to_owned(),
                            retryable: false,
                        });
                    } else {
                        let imported =
                            application::import_managed_profile_trace(&PathBuf::from(path));
                        match imported {
                            Ok(_) => match application::list_managed_profile_traces() {
                                Ok(records) => {
                                    self.profile_traces = records;
                                    self.profile_trace_error = None;
                                    self.revision.0 = self.revision.0.saturating_add(1);
                                }
                                Err(message) => {
                                    self.profile_trace_error = Some(message.clone());
                                    response_kind = ClientResponseKind::Rejected;
                                    error = Some(ServiceError {
                                        code: ServiceErrorCode::ProviderFailure,
                                        message,
                                        retryable: true,
                                    });
                                }
                            },
                            Err(message) => {
                                self.profile_trace_error = Some(message.clone());
                                response_kind = ClientResponseKind::Rejected;
                                error = Some(ServiceError {
                                    code: ServiceErrorCode::ProviderFailure,
                                    message,
                                    retryable: false,
                                });
                            }
                        }
                    }
                }
                ClientAction::Developer(DeveloperAction::AnalyzeProfileTrace {
                    trace_id,
                    options,
                }) => {
                    if self.bootstrap.fixture_mode {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(ServiceError {
                            code: ServiceErrorCode::ActionUnavailable,
                            message: "Fixture sessions cannot analyze managed traces.".to_owned(),
                            retryable: false,
                        });
                    } else if let Some(record) = self
                        .profile_traces
                        .iter()
                        .find(|record| record.id == trace_id)
                        .cloned()
                    {
                        let spawn = self.spawn_background_operation(
                            BackgroundOperationSpec {
                                domain: "profiling",
                                action: "analyze-trace",
                                thread_prefix: "vapor-profile-analysis",
                                failure_label: "profile trace analysis",
                                composition_id: None,
                                phase: OperationPhase::Finalizing,
                            },
                            move |_| application::analyze_managed_profile_trace(&record, &options),
                            |result| {
                                result
                                    .as_ref()
                                    .map(|_| "Trace analysis complete".to_owned())
                                    .map_err(Clone::clone)
                            },
                            |id, result| {
                                BackgroundOperationMessage::CompleteProfileAnalysis(Box::new(
                                    ProfileAnalysisCompletion { id, result },
                                ))
                            },
                        );
                        match spawn {
                            Ok(()) => response_kind = ClientResponseKind::OperationAccepted,
                            Err(failure) => {
                                response_kind = ClientResponseKind::Rejected;
                                error = Some(failure);
                            }
                        }
                    } else {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(ServiceError {
                            code: ServiceErrorCode::InvalidRequest,
                            message: format!(
                                "Saved trace '{trace_id}' is not in the current library."
                            ),
                            retryable: true,
                        });
                    }
                }
                ClientAction::Developer(DeveloperAction::OpenProfiler) => {
                    if self.bootstrap.fixture_mode {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(ServiceError {
                            code: ServiceErrorCode::ActionUnavailable,
                            message: "Fixture sessions cannot open host developer tools."
                                .to_owned(),
                            retryable: false,
                        });
                    } else if let Err(message) = application::open_managed_profiler() {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(ServiceError {
                            code: ServiceErrorCode::ProviderFailure,
                            message,
                            retryable: true,
                        });
                    }
                }
                ClientAction::Composition(action) => match action {
                    action @ (CompositionAction::Create { .. }
                    | CompositionAction::CreateFromContent { .. }
                    | CompositionAction::Duplicate { .. }
                    | CompositionAction::Rename { .. }
                    | CompositionAction::Remove { .. }
                    | CompositionAction::Select { .. }
                    | CompositionAction::SetRoot { .. }
                    | CompositionAction::ResolveDefaultRoot { .. }
                    | CompositionAction::SetLaunchArguments { .. }
                    | CompositionAction::AdoptVerifiedGraph { .. }
                    | CompositionAction::InvalidateContent { .. }) => {
                        match self.apply_composition_action(action) {
                            Ok(()) => self.revision.0 = self.revision.0.saturating_add(1),
                            Err(action_error) => {
                                response_kind = ClientResponseKind::Rejected;
                                error = Some(action_error);
                            }
                        }
                    }
                    action => match self.start_composition_operation(action) {
                        Ok(()) => {
                            response_kind = ClientResponseKind::OperationAccepted;
                            self.revision.0 = self.revision.0.saturating_add(1);
                        }
                        Err(operation_error) => {
                            response_kind = ClientResponseKind::Rejected;
                            error = Some(operation_error);
                        }
                    },
                },
                ClientAction::Workshop(WorkshopAction::DiscardReview) => {
                    self.workshop_review = None;
                    self.workshop_batch_review = None;
                    self.revision.0 = self.revision.0.saturating_add(1);
                }
                ClientAction::Workshop(action) => match self.start_workshop_operation(action) {
                    Ok(()) => {
                        response_kind = ClientResponseKind::OperationAccepted;
                        self.revision.0 = self.revision.0.saturating_add(1);
                    }
                    Err(operation_error) => {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(operation_error);
                    }
                },
                ClientAction::Publishing(action) => match action {
                    PublishingAction::DiscardRootReview => {
                        self.root_publish_review = None;
                        self.application_release_review = None;
                        self.application_release_snapshot = None;
                        self.revision.0 = self.revision.0.saturating_add(1);
                    }
                    action @ (PublishingAction::StageApplication { .. }
                    | PublishingAction::PublishApplication { .. }) => {
                        match self.start_application_release_operation(action) {
                            Ok(()) => {
                                response_kind = ClientResponseKind::OperationAccepted;
                                self.revision.0 = self.revision.0.saturating_add(1);
                            }
                            Err(operation_error) => {
                                response_kind = ClientResponseKind::Rejected;
                                error = Some(operation_error);
                            }
                        }
                    }
                    action => match self.start_root_publish_operation(action) {
                        Ok(()) => {
                            response_kind = ClientResponseKind::OperationAccepted;
                            self.revision.0 = self.revision.0.saturating_add(1);
                        }
                        Err(operation_error) => {
                            response_kind = ClientResponseKind::Rejected;
                            error = Some(operation_error);
                        }
                    },
                },
                action => {
                    response_kind = ClientResponseKind::Rejected;
                    error = Some(ServiceError {
                        code: ServiceErrorCode::ActionUnavailable,
                        message: format!(
                            "Action '{}' is not connected to the service yet.",
                            action.id()
                        ),
                        retryable: false,
                    });
                }
            }
        }
        let snapshot = self.snapshot();
        let operation_id = if response_kind == ClientResponseKind::OperationAccepted {
            snapshot
                .active_operations
                .iter()
                .find(|operation| !active_operation_ids_before.contains(&operation.id))
                .or_else(|| {
                    is_registered_source_list.then(|| {
                        snapshot.active_operations.iter().find(|operation| {
                            operation.action == "authoring.list-registered-sources"
                        })
                    })?
                })
                .map(|operation| operation.id.clone())
        } else {
            None
        };
        if response_kind == ClientResponseKind::OperationAccepted && operation_id.is_none() {
            response_kind = ClientResponseKind::Rejected;
            error = Some(ServiceError {
                code: ServiceErrorCode::InternalFailure,
                message: "The service accepted an operation without retaining its identity."
                    .to_owned(),
                retryable: true,
            });
        }
        ClientResponse {
            protocol_version: vapor_client_protocol::PROTOCOL_VERSION,
            request_id: request.request_id,
            revision: snapshot.revision,
            kind: response_kind,
            operation_id,
            snapshot,
            error,
        }
    }

    /// Drain background lifecycle updates and turn completed composition work into durable composition state.
    pub fn poll_background_operations(&mut self) -> Vec<ClientEvent> {
        self.poll_background_operation_messages();
        self.operation_events.drain(..).collect()
    }

    fn spawn_background_operation<T>(
        &mut self,
        spec: BackgroundOperationSpec,
        execute: impl FnOnce(application::OperationContext) -> T + Send + 'static,
        outcome: impl FnOnce(&T) -> Result<String, String> + Send + 'static,
        completion: impl FnOnce(String, T) -> BackgroundOperationMessage + Send + 'static,
    ) -> Result<(), ServiceError>
    where
        T: Send + 'static,
    {
        let operation_id = application::OperationId::fresh();
        let id = operation_id.0.to_string();
        let cancellable = matches!(
            spec.action,
            "workspace.fmt"
                | "workspace.check"
                | "workspace.test"
                | "workspace.build"
                | "workspace.run-configuration"
                | "workspace.package-application"
                | "authoring.discover-content-projects"
        ) && !self.bootstrap.fixture_mode;
        let cancellation = application::OperationCancellation::default();
        let resources = self.operation_resources(&spec);
        let summary = OperationSummary {
            id: id.clone(),
            composition_id: spec.composition_id.clone(),
            action: spec.action.to_owned(),
            phase: spec.phase,
            status: OperationStatus::Running,
            progress: None,
        };
        self.operation_registry
            .start(summary, resources, Some(cancellation.clone()), cancellable)
            .map_err(|conflict| {
                let message = match conflict {
                    OperationReservationError::Conflict { existing_operation_id } => format!(
                        "This request conflicts with operation {existing_operation_id}; it can start when that operation finishes."
                    ),
                    OperationReservationError::DuplicateOperationId => {
                        "The service generated a duplicate operation identifier.".to_owned()
                    }
                };
                ServiceError {
                    code: ServiceErrorCode::OperationInProgress,
                    message,
                    retryable: true,
                }
            })?;

        let request = application::CoreOperationRequest {
            id: operation_id,
            domain: spec.domain.to_owned(),
            action: spec.action.to_owned(),
        };
        let lifecycle_sender = self.operation_sender.clone();
        let completion_sender = self.operation_sender.clone();
        let worker_operation_id = id.clone();
        let failure_label = spec.failure_label;
        thread::Builder::new()
            .name(format!("{}-{id}", spec.thread_prefix))
            .spawn(move || {
                let sink: application::OperationEventSink = std::sync::Arc::new(move |event| {
                    if matches!(
                        event.status,
                        application::OperationStatus::Running
                            | application::OperationStatus::Cancelled
                    ) {
                        let _ = lifecycle_sender.send(BackgroundOperationMessage::Lifecycle(event));
                    }
                });
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    application::OperationService::run_observed_cancellable(
                        request,
                        sink,
                        cancellation,
                        execute,
                        outcome,
                    )
                }));
                match result {
                    Ok(result) => {
                        let _ = completion_sender.send(completion(worker_operation_id, result));
                    }
                    Err(payload) => {
                        let _ = completion_sender.send(BackgroundOperationMessage::Panicked {
                            id: worker_operation_id,
                            message: background_operation_panic_message(payload),
                        });
                    }
                }
            })
            .map_err(|error| {
                self.remove_operation(&id);
                ServiceError {
                    code: ServiceErrorCode::InternalFailure,
                    message: format!("Could not start {failure_label}: {error}"),
                    retryable: true,
                }
            })?;
        Ok(())
    }

    fn poll_background_operation_messages(&mut self) {
        while let Ok(message) = self.operation_receiver.try_recv() {
            if let Some(completion_id) = message.completion_id()
                && self
                    .operation_registry
                    .get(completion_id)
                    .is_some_and(|operation| {
                        matches!(
                            operation.summary.status,
                            OperationStatus::Cancelling | OperationStatus::Cancelled
                        )
                    })
            {
                if let Some(active) = self
                    .operation_registry
                    .get(completion_id)
                    .map(|operation| operation.summary.clone())
                {
                    let status = OperationStatus::Cancelled;
                    self.finish_operation(completion_id, status.clone());
                    self.revision.0 = self.revision.0.saturating_add(1);
                    self.operation_events
                        .push_back(ClientEvent::OperationChanged(OperationEvent {
                            revision: self.revision,
                            id: completion_id.to_owned(),
                            composition_id: active.composition_id,
                            action: active.action,
                            phase: OperationPhase::Complete,
                            status,
                            progress: active.progress,
                            message: Some("Operation cancelled.".to_owned()),
                        }));
                }
                continue;
            }
            match message {
                BackgroundOperationMessage::Panicked { id, message } => {
                    let Some(active) = self
                        .operation_registry
                        .get(&id)
                        .map(|op| op.summary.clone())
                    else {
                        continue;
                    };
                    let status = OperationStatus::Failed {
                        message: message.clone(),
                    };
                    self.finish_operation(&id, status.clone());
                    self.revision.0 = self.revision.0.saturating_add(1);
                    self.operation_events
                        .push_back(ClientEvent::OperationChanged(OperationEvent {
                            revision: self.revision,
                            id,
                            composition_id: active.composition_id,
                            action: active.action,
                            phase: OperationPhase::Complete,
                            status,
                            progress: active.progress,
                            message: Some(message),
                        }));
                }
                BackgroundOperationMessage::Lifecycle(event) => {
                    let id = event.id.0.to_string();
                    let progress = event.progress.as_ref().and_then(|progress| {
                        progress
                            .total
                            .filter(|total| *total > 0)
                            .map(|total| (progress.completed as f32 / total as f32).clamp(0.0, 1.0))
                    });
                    let Some(active) = self.operation_registry.get_mut(&id) else {
                        continue;
                    };
                    active.summary.phase = protocol_phase(event.phase);
                    active.summary.progress = progress;
                    if let Some(output) = event.output {
                        queue_operation_output(
                            &mut self.operation_events,
                            vapor_client_protocol::OperationOutputEvent {
                                id,
                                stream: match output.stream {
                                    application::OperationOutputStream::Stdout => {
                                        vapor_client_protocol::OperationOutputStream::Stdout
                                    }
                                    application::OperationOutputStream::Stderr => {
                                        vapor_client_protocol::OperationOutputStream::Stderr
                                    }
                                },
                                text: output.text,
                            },
                        );
                        continue;
                    }
                    let (mut status, message) = protocol_status(event.status);
                    if active.summary.status == OperationStatus::Cancelling
                        && status == OperationStatus::Running
                    {
                        status = OperationStatus::Cancelling;
                    }
                    let message = message.or_else(|| {
                        event
                            .progress
                            .as_ref()
                            .and_then(|progress| progress.message.clone())
                    });
                    if matches!(
                        status,
                        OperationStatus::Succeeded
                            | OperationStatus::Failed { .. }
                            | OperationStatus::Cancelled
                    ) {
                        // The typed completion carries the authoritative outcome.
                        // Publishing terminal lifecycle events here would report
                        // success/failure twice for one accepted operation.
                        if status == OperationStatus::Cancelled {
                            active.summary.status = OperationStatus::Cancelled;
                        }
                        continue;
                    }
                    active.summary.status = status.clone();
                    let event_summary = active.summary.clone();
                    self.revision.0 = self.revision.0.saturating_add(1);
                    self.operation_events
                        .push_back(ClientEvent::OperationChanged(OperationEvent {
                            revision: self.revision,
                            id,
                            composition_id: event_summary.composition_id,
                            action: event_summary.action,
                            phase: event_summary.phase,
                            status,
                            progress: event_summary.progress,
                            message,
                        }));
                }
                BackgroundOperationMessage::Complete(completion) => {
                    self.apply_composition_completion(*completion);
                }
                BackgroundOperationMessage::CompleteWorkshop(completion) => {
                    self.apply_workshop_completion(*completion);
                }
                BackgroundOperationMessage::CompleteRootPublish(completion) => {
                    self.apply_root_publish_completion(*completion);
                }
                BackgroundOperationMessage::CompleteApplicationRelease(completion) => {
                    self.apply_application_release_completion(*completion);
                }
                BackgroundOperationMessage::CompleteWorkspace(completion) => {
                    self.apply_workspace_completion(*completion);
                }
                BackgroundOperationMessage::CompleteAuthoring(completion) => {
                    self.apply_authoring_completion(*completion);
                }
                BackgroundOperationMessage::CompleteLibrary(completion) => {
                    self.apply_library_completion(*completion);
                }
                BackgroundOperationMessage::CompleteCatalogRefresh(completion) => {
                    self.apply_catalog_refresh_completion(*completion);
                }
                BackgroundOperationMessage::CompleteWorkspaceDiscovery(completion) => {
                    self.apply_workspace_discovery_completion(*completion);
                }
                BackgroundOperationMessage::CompleteProfileAnalysis(completion) => {
                    self.apply_profile_analysis_completion(*completion);
                }
                BackgroundOperationMessage::CompleteRoleTransition(completion) => {
                    self.apply_role_transition_completion(*completion);
                }
            }
        }
    }

    fn apply_profile_analysis_completion(&mut self, completion: ProfileAnalysisCompletion) {
        let Some(active) = self
            .operation_registry
            .get(&completion.id)
            .map(|operation| operation.summary.clone())
        else {
            return;
        };
        let action = active.action.clone();
        let (status, message) = match &completion.result {
            Ok(_) => (
                OperationStatus::Succeeded,
                Some("Trace analysis complete.".to_owned()),
            ),
            Err(message) => (
                OperationStatus::Failed {
                    message: message.clone(),
                },
                Some(message.clone()),
            ),
        };
        self.finish_operation(&completion.id, status.clone());
        self.revision.0 = self.revision.0.saturating_add(1);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: completion.id,
                composition_id: None,
                action,
                phase: OperationPhase::Complete,
                status,
                progress: None,
                message,
            }));
        self.operation_events
            .push_back(ClientEvent::ProfileAnalysis(completion.result));
    }

    fn apply_role_transition_completion(&mut self, completion: RoleTransitionCompletion) {
        let Some(active) = self
            .operation_registry
            .get(&completion.id)
            .map(|operation| operation.summary.clone())
        else {
            return;
        };
        let action = active.action.clone();
        let (status, message) = match completion.result {
            Ok(message) => (OperationStatus::Succeeded, Some(message)),
            Err(message) => (
                OperationStatus::Failed {
                    message: message.clone(),
                },
                Some(message),
            ),
        };
        self.finish_operation(&completion.id, status.clone());
        self.revision.0 = self.revision.0.saturating_add(1);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: completion.id,
                composition_id: None,
                action,
                phase: OperationPhase::Complete,
                status,
                progress: None,
                message,
            }));
    }

    fn collect_unreferenced_auto_install(&mut self) -> Result<Vec<String>, String> {
        if self.bootstrap.fixture_mode {
            return Ok(Vec::new());
        }
        let has_unresolved_installed_composition = self
            .durable_state
            .compositions
            .compositions
            .iter()
            .any(|composition| {
                if composition.root_package.is_none() {
                    return false;
                }
                match &composition.installation {
                    InstallationState::Partial { .. } => true,
                    InstallationState::Installed { artifacts, .. } => {
                        artifacts.is_empty() && composition.resolved_graph.is_none()
                    }
                    InstallationState::OutOfDate {
                        actual_graph,
                        artifacts,
                        ..
                    } => artifacts.is_empty() && actual_graph.nodes.is_empty(),
                    InstallationState::NeedsRepair { artifacts, .. } => {
                        artifacts.is_empty() && composition.resolved_graph.is_none()
                    }
                    InstallationState::Unknown | InstallationState::NotInstalled => false,
                }
            });
        if has_unresolved_installed_composition {
            return Ok(Vec::new());
        }
        let policies = self.durable_state.library_policies.clone();
        let mut collected = Vec::new();
        let mut unsubscribe_ids = BTreeSet::new();
        for policy in policies
            .into_iter()
            .filter(|policy| policy.auto_install && policy.packagepack_owners.is_empty())
        {
            let referenced = vapor_client_domain::installed_composition_uses_content(
                &self.durable_state.compositions,
                &policy.content_id,
            );
            if referenced {
                continue;
            }
            let entry = self
                .catalog
                .entries()
                .iter()
                .find(|entry| entry.id == policy.content_id.as_str())
                .ok_or_else(|| format!(
                    "Auto-Install cannot release '{}': refresh the Library to recover its Workshop identity.",
                    policy.content_id
                ))?;
            let published_file_id = entry
                .published_file_id
                .as_deref()
                .ok_or_else(|| format!(
                    "Auto-Install cannot release '{}': its Workshop subscription identity is missing.",
                    policy.content_id
                ))?
                .parse::<u64>()
                .map_err(|_| format!(
                    "Auto-Install cannot release '{}': its Workshop item ID is invalid.",
                    policy.content_id
                ))?;
            application::collect_library_item(
                &self.bootstrap.application_executable,
                policy.content_id.as_str(),
            )?;
            unsubscribe_ids.insert(published_file_id);
            collected.push(policy.content_id.to_string());
        }
        if !unsubscribe_ids.is_empty() {
            let app_id = self.bootstrap.workshop_app_id.ok_or_else(|| {
                "Auto-Install found managed subscriptions, but this installation has no Workshop application configured.".to_owned()
            })?;
            let ids = unsubscribe_ids.into_iter().collect::<Vec<_>>();
            for batch in ids.chunks(application::MAX_WORKSHOP_SUBSCRIPTION_BATCH) {
                application::unsubscribe_workshop_items(app_id, batch)?;
            }
            let published_ids = ids.iter().map(u64::to_string).collect::<BTreeSet<_>>();
            self.workshop
                .subscriptions
                .retain(|item| !published_ids.contains(&item.published_file_id));
            self.fixture_workshop_subscriptions
                .retain(|id| !published_ids.contains(id));
            let previous = self.durable_state.library_policies.clone();
            let collected_ids = collected.iter().cloned().collect::<BTreeSet<_>>();
            self.durable_state
                .library_policies
                .retain(|policy| !collected_ids.contains(policy.content_id.as_str()));
            if let Some(store) = &self.state_store
                && let Err(error) = store.save(&self.durable_state)
            {
                self.durable_state.library_policies = previous;
                return Err(format!(
                    "Auto-Install released unused Workshop items, but could not save the updated ownership state: {error}"
                ));
            }
            self.workshop.subscriptions_revision =
                self.workshop.subscriptions_revision.saturating_add(1);
        }
        if !collected.is_empty() {
            self.refresh_catalog();
        }
        Ok(collected)
    }
}

fn seed_legacy_empty_selection(state: &mut ClientPersistentState, seed: Option<&str>) -> bool {
    let Some(composition) = state
        .compositions
        .compositions
        .iter_mut()
        .find(|composition| composition.id == "default" && composition.name == "Default")
    else {
        return false;
    };
    let mut changed = false;
    if composition.root_package.is_none() {
        let Some(seed) = seed.filter(|seed| !seed.trim().is_empty()) else {
            return false;
        };
        composition.root_package = Some(ContentReference {
            id: seed.to_owned(),
            version: None,
        });
        composition.resolved_graph = None;
        composition.installation = InstallationState::Unknown;
        changed = true;
    }
    if composition.name == "Default"
        && let Some(root) = composition.root_package.as_ref()
    {
        let label = root
            .id
            .rsplit('/')
            .next()
            .unwrap_or(&root.id)
            .strip_suffix("-packagepack")
            .unwrap_or_else(|| root.id.rsplit('/').next().unwrap_or(&root.id));
        let name = label
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" ");
        if !name.is_empty() {
            composition.name = name;
            changed = true;
        }
    }
    changed
}

fn is_registered_source_list_query(action: &ClientAction) -> bool {
    matches!(
        action,
        ClientAction::Authoring(AuthoringAction::ListRegisteredSources)
    )
}

fn action_can_rebase_on_current_state(action: &ClientAction) -> bool {
    matches!(
        action,
        ClientAction::Catalog(CatalogAction::Refresh | CatalogAction::DiscoverContentProjects)
            | ClientAction::Composition(_)
            | ClientAction::Workspace(_)
            | ClientAction::Workshop(_)
            | ClientAction::Authoring(_)
            | ClientAction::Library(_)
            | ClientAction::Publishing(_)
            | ClientAction::Developer(
                DeveloperAction::AnalyzeProfileTrace { .. }
                    | DeveloperAction::PromoteInstallationRole { .. }
                    | DeveloperAction::DemoteInstallationRole { .. }
            )
    )
}

fn ensure_workshop_app_id(bootstrap: &mut ClientHostConfig) {
    if !bootstrap.fixture_mode && bootstrap.workshop_app_id.is_none() {
        bootstrap.workshop_app_id = Some(vapor_sdk_core::root::types::ROOT_STEAM_APP_ID);
    }
}

fn is_installed_application_root(root: &Path) -> bool {
    root.join("App.vapor.toml").is_file() && !root.join("Workspace.vapor.toml").is_file()
}

fn default_superworkspace_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .map(|home| home.join("Vapor"))
}

fn configured_superworkspace_root() -> Result<PathBuf, String> {
    let installation =
        vapor_core::VaporInstallation::discover().map_err(|error| error.to_string())?;
    let configured = vapor_core::source_state(&installation)
        .map_err(|error| error.to_string())?
        .superworkspace;
    let root = configured
        .or_else(default_superworkspace_path)
        .ok_or_else(|| "Your home folder could not be resolved.".to_owned())?;
    let discovered = vapor_core::VaporSuperworkspace::discover_from(&root)
        .map_err(|_| "Set up the Vapor source folder before finding repositories.".to_owned())?;
    if discovered.root != root {
        return Err(format!(
            "The selected source folder is inside another Superworkspace at {}.",
            discovered.root.display()
        ));
    }
    Ok(root)
}

pub(super) fn invalid_client_action_request(message: &str) -> ServiceError {
    ServiceError {
        code: ServiceErrorCode::InvalidRequest,
        message: message.to_owned(),
        retryable: false,
    }
}

pub(super) fn validate_runtime_target_matrix(targets: &[String]) -> Result<(), ServiceError> {
    if targets.len() > 32 {
        return Err(invalid_client_action_request(
            "A content package may contain at most 32 runtime targets.",
        ));
    }
    let mut unique = BTreeSet::new();
    for target in targets {
        if target.is_empty()
            || target.len() > 128
            || target.trim() != target
            || !unique.insert(target)
        {
            return Err(invalid_client_action_request(
                "Runtime targets must be unique, non-empty target triples without surrounding whitespace and at most 128 bytes.",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use vapor_client_protocol::{ClientAction, ClientRequest, RequestId};

    #[test]
    fn action_profiles_reject_missing_local_capabilities() {
        let action = ClientAction::Workspace(WorkspaceAction::BuildDocumentation);
        let mut readiness = InstallationRoleSnapshot::default();
        let unavailable = action_capability_error(action.profile(), &readiness)
            .expect("managed Rust is required by this action");
        assert!(unavailable.message.contains("Rust/Cargo"));

        readiness.rust_ready = true;
        assert!(action_capability_error(action.profile(), &readiness).is_none());
    }

    #[test]
    fn operation_registry_allows_independent_work_and_rejects_same_workspace_scan() {
        let mut registry = OperationRegistry::default();
        let workspace = PathBuf::from("/fixture/workspace");
        let other_workspace = PathBuf::from("/fixture/other-workspace");

        registry
            .start(
                running_operation("catalog", "catalog.refresh"),
                [OperationResourceKey::CatalogRefresh],
                None,
                false,
            )
            .expect("catalog refresh should be reserved");
        registry
            .start(
                running_operation("workspace-scan", "workspace.scan"),
                [OperationResourceKey::WorkspaceScan(workspace.clone())],
                None,
                false,
            )
            .expect("workspace scan should not conflict with catalog refresh");
        registry
            .start(
                running_operation("workspace-build", "workspace.build"),
                [OperationResourceKey::Workspace(workspace.clone())],
                None,
                false,
            )
            .expect("read-only discovery and workspace build may proceed independently");
        registry
            .start(
                running_operation("other-workspace-scan", "workspace.scan"),
                [OperationResourceKey::WorkspaceScan(other_workspace)],
                None,
                false,
            )
            .expect("different workspace scans should not conflict");

        let conflict = registry.start(
            running_operation("duplicate-workspace-scan", "workspace.scan"),
            [OperationResourceKey::WorkspaceScan(workspace.clone())],
            None,
            false,
        );
        assert!(matches!(
            conflict,
            Err(OperationReservationError::Conflict { .. })
        ));
        let edit_conflict = registry.start(
            running_operation("workspace-edit", "workspace.save-file"),
            [OperationResourceKey::WorkspaceEdit(workspace.clone())],
            None,
            false,
        );
        assert!(matches!(
            edit_conflict,
            Err(OperationReservationError::Conflict { .. })
        ));
        assert_eq!(registry.active_summaries().len(), 4);
    }

    #[test]
    fn accepted_response_identifies_its_operation_when_an_unrelated_one_is_primary() {
        let mut session = session();
        session.bootstrap.fixture_mode = false;
        session
            .operation_registry
            .start(
                running_operation("library-refresh", "catalog.refresh"),
                [OperationResourceKey::CatalogRefresh],
                None,
                false,
            )
            .expect("register unrelated in-flight Library operation");

        let response = session.dispatch(request(
            None,
            ClientAction::Authoring(AuthoringAction::ListRegisteredSources),
        ));
        let authoring = response
            .snapshot
            .active_operations
            .iter()
            .find(|operation| operation.action == "authoring.list-registered-sources")
            .expect("source-list operation is active");

        assert_eq!(
            response.snapshot.active_operation.as_ref().unwrap().id,
            "library-refresh"
        );
        assert_eq!(
            response.operation_id.as_deref(),
            Some(authoring.id.as_str())
        );
        assert_eq!(
            serde_json::to_value(&response).expect("serialize response")["operation_id"],
            authoring.id
        );
    }

    #[test]
    fn manifest_seed_initializes_only_the_legacy_empty_selection() {
        let mut state = ClientPersistentState::default();
        assert!(seed_legacy_empty_selection(
            &mut state,
            Some("publisher/loo-cast/loo-cast-packagepack")
        ));
        let selected = &state.compositions.compositions[0];
        assert_eq!(selected.id, "default");
        assert_eq!(selected.name, "Loo Cast");
        assert_eq!(
            selected.root_package.as_ref().map(|root| root.id.as_str()),
            Some("publisher/loo-cast/loo-cast-packagepack")
        );
        assert!(!seed_legacy_empty_selection(
            &mut state,
            Some("publisher/other/other-packagepack")
        ));
        assert_eq!(state.compositions.compositions.len(), 1);
        assert_eq!(state.compositions.compositions[0].name, "Loo Cast");
    }

    #[test]
    fn manifest_seed_preserves_custom_compositions_and_roots() {
        let mut state = ClientPersistentState::default();
        state.compositions.compositions[0].name = "My Setup".to_owned();
        assert!(!seed_legacy_empty_selection(
            &mut state,
            Some("publisher/loo-cast/loo-cast-packagepack")
        ));
        assert_eq!(state.compositions.compositions[0].name, "My Setup");
        assert!(state.compositions.compositions[0].root_package.is_none());
    }

    #[test]
    fn panicking_background_operation_reports_failure_and_releases_service_idle_state() {
        let mut session = session();
        session
            .spawn_background_operation(
                BackgroundOperationSpec {
                    domain: "workspace",
                    action: "workspace.test",
                    thread_prefix: "test-operation",
                    failure_label: "workspace test",
                    composition_id: None,
                    phase: OperationPhase::Building,
                },
                |_| -> Result<(), String> { panic!("synthetic worker panic") },
                |_| Ok("completed".to_owned()),
                |id, _| BackgroundOperationMessage::Panicked {
                    id,
                    message: "synthetic worker panic".to_owned(),
                },
            )
            .expect("start background operation");

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut events = Vec::new();
        while session.has_active_operation() && Instant::now() < deadline {
            events.extend(session.poll_background_operations());
            thread::sleep(Duration::from_millis(5));
        }
        events.extend(session.poll_background_operations());

        assert!(!session.has_active_operation());
        assert!(events.iter().any(|event| {
            matches!(event, ClientEvent::OperationChanged(event)
                if matches!(event.status, OperationStatus::Failed { .. }))
        }));
    }

    #[test]
    fn buffered_operation_output_is_capped_without_dropping_lifecycle_events() {
        let mut events = VecDeque::from([ClientEvent::OperationChanged(OperationEvent {
            revision: ClientRevision(1),
            id: "operation-1".to_owned(),
            composition_id: None,
            action: "workspace.test".to_owned(),
            phase: OperationPhase::Running,
            status: OperationStatus::Running,
            progress: None,
            message: None,
        })]);

        for index in 0..=MAX_BUFFERED_OPERATION_OUTPUT_EVENTS {
            queue_operation_output(
                &mut events,
                OperationOutputEvent {
                    id: "operation-1".to_owned(),
                    stream: vapor_client_protocol::OperationOutputStream::Stdout,
                    text: format!("line-{index}"),
                },
            );
        }

        let outputs = events
            .iter()
            .filter_map(|event| match event {
                ClientEvent::OperationOutput(output) => Some(output),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(outputs.len(), MAX_BUFFERED_OPERATION_OUTPUT_EVENTS);
        assert_eq!(
            outputs.last().map(|output| output.text.as_str()),
            Some("line-512")
        );
        assert_eq!(
            outputs
                .iter()
                .filter(|output| output.text == OPERATION_OUTPUT_OMISSION_NOTICE)
                .count(),
            1
        );
        assert!(matches!(
            events.front(),
            Some(ClientEvent::OperationChanged(_))
        ));
    }

    #[test]
    fn pending_background_operation_messages_apply_backpressure_at_a_fixed_limit() {
        let session = session();
        let event = || application::CoreOperationEvent {
            id: application::OperationId(1),
            domain: "workspace".to_owned(),
            action: "test".to_owned(),
            phase: application::OperationPhase::Running,
            progress: None,
            output: Some(application::OperationOutput {
                stream: application::OperationOutputStream::Stdout,
                text: "compiler output".to_owned(),
            }),
            status: application::OperationStatus::Running,
        };

        for _ in 0..MAX_PENDING_BACKGROUND_OPERATION_MESSAGES {
            assert!(
                session
                    .operation_sender
                    .try_send(BackgroundOperationMessage::Lifecycle(event()))
                    .is_ok()
            );
        }
        assert!(matches!(
            session
                .operation_sender
                .try_send(BackgroundOperationMessage::Lifecycle(event())),
            Err(mpsc::TrySendError::Full(_))
        ));
        assert_eq!(
            session.operation_receiver.try_iter().count(),
            MAX_PENDING_BACKGROUND_OPERATION_MESSAGES
        );
    }

    #[test]
    fn installed_app_root_is_not_restored_as_an_authoring_workspace() {
        let app_root = fixture_temp_workspace("installed-app-root");
        let bin = app_root.join("bin");
        std::fs::create_dir_all(&bin).expect("create installed app bin");
        std::fs::write(app_root.join("App.vapor.toml"), "[app]\nid = \"test\"\n")
            .expect("write installed app manifest");

        let state_directory = fixture_temp_workspace("installed-app-state");
        let store = ClientStateStore::new(state_directory.clone());
        let saved_state = ClientPersistentState {
            authoring_workspace_root: Some(app_root.display().to_string()),
            ..ClientPersistentState::default()
        };
        store
            .save(&saved_state)
            .expect("save stale authoring selection");

        let host_context = ClientHostContext {
            application_executable: bin.join("vapor-installer"),
            app_root: Some(app_root.clone()),
            workspace_root: None,
            state_directory: state_directory.clone(),
            platform: vapor_client::HostPlatform::Linux,
            steam_client_available: true,
        };
        let session = ClientSession::with_persistent_state(
            ClientHostConfig {
                application_executable: bin.join("vapor-installer"),
                workshop_app_id: Some(2122620),
                workspace_root: None,
                active_composition_id: None,
                fixture_mode: true,
                legacy_storage: LegacyStorageSeed::default(),
                state_directory: Some(state_directory.clone()),
            },
            host_context,
            store,
            None,
        )
        .expect("start service from installed app");

        assert_eq!(session.bootstrap.workspace_root, None);
        assert_eq!(session.durable_state.authoring_workspace_root, None);
        assert_eq!(
            ClientStateStore::new(state_directory.clone())
                .load_or_import(None)
                .expect("load corrected state")
                .state
                .authoring_workspace_root,
            None
        );

        std::fs::remove_dir_all(app_root).expect("remove installed app fixture");
        std::fs::remove_dir_all(state_directory).expect("remove state fixture");
    }

    fn session() -> ClientSession {
        ClientSession::new(ClientHostConfig {
            application_executable: PathBuf::from("/app/bin/vapor-gui"),
            workshop_app_id: None,
            workspace_root: None,
            active_composition_id: Some("default".to_owned()),
            fixture_mode: true,
            legacy_storage: vapor_client_protocol::LegacyStorageSeed::default(),
            state_directory: None,
        })
    }

    fn fixture_workspace_session() -> ClientSession {
        ClientSession::new(ClientHostConfig {
            application_executable: PathBuf::from("/app/bin/vapor-gui"),
            workshop_app_id: Some(2122620),
            workspace_root: Some(PathBuf::from("/fixture/workspace")),
            active_composition_id: Some("default".to_owned()),
            fixture_mode: true,
            legacy_storage: vapor_client_protocol::LegacyStorageSeed::default(),
            state_directory: None,
        })
    }

    fn request(revision: Option<u64>, action: ClientAction) -> ClientRequest {
        ClientRequest {
            protocol_version: vapor_client_protocol::PROTOCOL_VERSION,
            request_id: RequestId("test-request".to_owned()),
            expected_revision: revision.map(ClientRevision),
            action,
        }
    }

    fn running_operation(id: &str, action: &str) -> OperationSummary {
        OperationSummary {
            id: id.to_owned(),
            composition_id: None,
            action: action.to_owned(),
            phase: OperationPhase::Building,
            status: OperationStatus::Running,
            progress: None,
        }
    }

    fn register_test_operation(
        session: &mut ClientSession,
        summary: OperationSummary,
        cancellation: Option<application::OperationCancellation>,
        cancellable: bool,
    ) {
        let resource = OperationResourceKey::Exclusive(summary.id.clone());
        session
            .operation_registry
            .start(summary, [resource], cancellation, cancellable)
            .expect("register test operation");
    }

    #[test]
    fn concurrent_registered_source_queries_join_the_active_lookup() {
        let mut session = session();
        session.revision = ClientRevision(5);
        register_test_operation(
            &mut session,
            running_operation("source-list-1", "authoring.list-registered-sources"),
            None,
            false,
        );

        let response = session.dispatch(request(
            Some(0),
            ClientAction::Authoring(AuthoringAction::ListRegisteredSources),
        ));

        assert_eq!(response.kind, ClientResponseKind::OperationAccepted);
        assert_eq!(response.revision, ClientRevision(5));
        assert_eq!(response.operation_id.as_deref(), Some("source-list-1"));
        assert_eq!(
            response.snapshot.active_operation.unwrap().id,
            "source-list-1"
        );
        assert!(response.error.is_none());
    }

    #[test]
    fn cancellation_targets_the_active_operation_even_after_its_revision_changes() {
        let mut session = session();
        let cancellation = application::OperationCancellation::default();
        register_test_operation(
            &mut session,
            running_operation("42", "workspace.test"),
            Some(cancellation.clone()),
            true,
        );

        let response = session.dispatch(request(
            Some(u64::MAX),
            ClientAction::Session(SessionAction::CancelOperation {
                operation_id: "42".to_owned(),
            }),
        ));

        assert_eq!(response.kind, ClientResponseKind::Applied);
        assert!(cancellation.is_cancelled());
        assert_eq!(
            session.operation_registry.primary().unwrap().status,
            OperationStatus::Cancelling
        );
        assert!(matches!(
            session.operation_events.back(),
            Some(ClientEvent::OperationChanged(OperationEvent {
                status: OperationStatus::Cancelling,
                ..
            }))
        ));

        session
            .operation_sender
            .send(BackgroundOperationMessage::Panicked {
                id: "42".to_owned(),
                message: "worker stopped".to_owned(),
            })
            .expect("send terminal worker result");
        session.poll_background_operation_messages();
        assert!(session.operation_registry.is_empty());
        assert_eq!(
            session
                .operation_registry
                .history()
                .back()
                .map(|summary| &summary.status),
            Some(&OperationStatus::Cancelled)
        );
        assert_eq!(
            session
                .operation_events
                .iter()
                .filter(|event| matches!(
                    event,
                    ClientEvent::OperationChanged(OperationEvent {
                        id,
                        phase: OperationPhase::Complete,
                        ..
                    }) if id == "42"
                ))
                .count(),
            1
        );
    }

    #[test]
    fn cancellation_rejects_non_cancellable_operations_instead_of_cancelling_them() {
        let mut session = session();
        let cancellation = application::OperationCancellation::default();
        register_test_operation(
            &mut session,
            running_operation("42", "publishing.publish-root"),
            Some(cancellation.clone()),
            false,
        );

        let response = session.dispatch(request(
            None,
            ClientAction::Session(SessionAction::CancelOperation {
                operation_id: "42".to_owned(),
            }),
        ));

        assert_eq!(response.kind, ClientResponseKind::Rejected);
        assert_eq!(
            response.error.unwrap().code,
            ServiceErrorCode::ActionUnavailable
        );
        assert!(!cancellation.is_cancelled());
        assert_eq!(
            session.operation_registry.primary().unwrap().status,
            OperationStatus::Running
        );
    }

    #[test]
    fn cancellation_records_one_terminal_outcome_and_discards_the_late_result() {
        let mut session = session();
        let mut active = running_operation("42", "developer.analyze-profile-trace");
        active.status = OperationStatus::Cancelled;
        register_test_operation(&mut session, active, None, false);
        session
            .operation_sender
            .send(BackgroundOperationMessage::CompleteProfileAnalysis(
                Box::new(ProfileAnalysisCompletion {
                    id: "42".to_owned(),
                    result: Err("late worker failure".to_owned()),
                }),
            ))
            .unwrap();

        session.poll_background_operation_messages();

        assert!(!session.has_active_operation());
        assert_eq!(
            session
                .operation_registry
                .history()
                .back()
                .map(|summary| &summary.status),
            Some(&OperationStatus::Cancelled)
        );
        assert_eq!(
            session
                .operation_events
                .iter()
                .filter(|event| matches!(
                    event,
                    ClientEvent::OperationChanged(OperationEvent {
                        id,
                        phase: OperationPhase::Complete,
                        status: OperationStatus::Cancelled,
                        ..
                    }) if id == "42"
                ))
                .count(),
            1
        );
        assert!(session.profile_traces.is_empty());
    }

    #[test]
    fn application_publish_rejects_unmatched_confirmation_before_starting_upload() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock follows epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "vapor-client-release-confirmation-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create isolated workspace");
        let mut session = ClientSession::new(ClientHostConfig {
            application_executable: PathBuf::from("/app/bin/vapor-gui"),
            workshop_app_id: None,
            workspace_root: Some(root.clone()),
            active_composition_id: None,
            fixture_mode: false,
            legacy_storage: vapor_client_protocol::LegacyStorageSeed::default(),
            state_directory: Some(root.join("service-state")),
        });
        let review = application::ApplicationReleaseReview {
            app_id: 2122620,
            depot_ids: vec![2122621],
            runtime_targets: vec!["x86_64-unknown-linux-gnu".into()],
            branch: "vapor-dev".into(),
            account: "builder".into(),
            description: "release".into(),
            release_build_validated: true,
            package_root: root.join("stage"),
            build_script: root.join("build.vdf"),
            files: 1,
            fingerprint: "fnv1a64:1234:1:1".into(),
        };
        session.application_release_review = Some(review.clone());
        session.application_release_snapshot = Some(ApplicationReleaseSnapshot {
            review_id: "release-review".into(),
            planned: true,
            app_id: review.app_id,
            depot_ids: review.depot_ids.clone(),
            runtime_targets: review.runtime_targets.clone(),
            branch: review.branch.clone(),
            account: review.account.clone(),
            description: review.description.clone(),
            release_build_validated: review.release_build_validated,
            package_root: review.package_root.display().to_string(),
            build_script: review.build_script.display().to_string(),
            files: review.files,
            fingerprint: review.fingerprint.clone(),
            succeeded: false,
            status: None,
        });

        let response = session.dispatch(request(
            Some(0),
            ClientAction::Publishing(PublishingAction::PublishApplication {
                review_id: "release-review".into(),
                confirmation: "wrong confirmation".into(),
            }),
        ));

        assert_eq!(response.kind, ClientResponseKind::Rejected);
        assert!(!session.has_active_operation());
        assert!(session.application_release_review.is_some());

        let mut unbuilt_review = review;
        unbuilt_review.release_build_validated = false;
        session.application_release_review = Some(unbuilt_review.clone());
        session.application_release_snapshot = Some(ApplicationReleaseSnapshot {
            review_id: "release-review".into(),
            planned: true,
            app_id: unbuilt_review.app_id,
            depot_ids: unbuilt_review.depot_ids.clone(),
            runtime_targets: unbuilt_review.runtime_targets.clone(),
            branch: unbuilt_review.branch.clone(),
            account: unbuilt_review.account.clone(),
            description: unbuilt_review.description.clone(),
            release_build_validated: false,
            package_root: unbuilt_review.package_root.display().to_string(),
            build_script: unbuilt_review.build_script.display().to_string(),
            files: unbuilt_review.files,
            fingerprint: unbuilt_review.fingerprint.clone(),
            succeeded: false,
            status: None,
        });
        let response = session.dispatch(request(
            Some(0),
            ClientAction::Publishing(PublishingAction::PublishApplication {
                review_id: "release-review".into(),
                confirmation: "PUBLISH APPLICATION 2122620 TO vapor-dev".into(),
            }),
        ));
        assert_eq!(response.kind, ClientResponseKind::Rejected);
        assert!(
            response
                .error
                .is_some_and(|error| error.message.contains("service-validated release build"))
        );
        assert!(!session.has_active_operation());
        std::fs::remove_dir_all(root).expect("remove isolated workspace");
    }

    #[test]
    fn enabling_referenced_content_remains_available_to_repair_a_composition() {
        assert!(!library_action_can_invalidate_composition(
            &LibraryAction::Enable {
                selector: "fixture.engine".into(),
            }
        ));
        assert!(library_action_can_invalidate_composition(
            &LibraryAction::Disable {
                selector: "fixture.engine".into(),
            }
        ));
    }

    #[test]
    fn automatic_update_policy_requires_cached_workshop_content_in_a_composition() {
        let mut session = session();
        let content_id = "publisher/project/movement";
        let content = ContentReference::parse(content_id.to_owned(), Some("1.0.0".to_owned()))
            .expect("valid content reference");
        let root = ContentReference::parse("author/game/pack".to_owned(), Some("1.0.0".to_owned()))
            .expect("valid composition root");
        session.durable_state.compositions.compositions[0].root_package = Some(root.clone());
        session.durable_state.compositions.compositions[0].installation =
            InstallationState::Installed {
                artifacts: Vec::new(),
                graph_fingerprint: None,
            };
        session.durable_state.compositions.compositions[0].resolved_graph =
            Some(vapor_client_protocol::ResolvedCompositionGraph {
                root: root.clone(),
                effective_engine: ContentReference::parse(
                    "author/engine/core".to_owned(),
                    Some("1.0.0".to_owned()),
                )
                .unwrap(),
                effective_game: Some(
                    ContentReference::parse(
                        "author/game/main".to_owned(),
                        Some("1.0.0".to_owned()),
                    )
                    .unwrap(),
                ),
                nodes: vec![vapor_client_protocol::ResolvedNodeIdentity {
                    content,
                    kind: "game-mod".to_owned(),
                    fingerprint: None,
                    source: None,
                    dependencies: Vec::new(),
                }],
            });
        let mut entry = CatalogEntry {
            id: content_id.to_owned(),
            name: Some("Movement".to_owned()),
            version: Some("1.0.0".to_owned()),
            kind: "game-mod".to_owned(),
            published_file_id: Some("3762162552".to_owned()),
            dependencies: Vec::new(),
            dependency_details: Vec::new(),
            conflicts: Vec::new(),
            locations: vec![CatalogLocation {
                kind: CatalogLocationKind::Cache,
                root: "/cache/movement".to_owned(),
                enabled: true,
                fingerprint: None,
                install_source: None,
            }],
            role: CatalogItemRole::RuntimeContent,
            actions: Vec::new(),
        };
        let root_entry = CatalogEntry {
            id: root.id.clone(),
            name: Some("Pack".to_owned()),
            version: Some("1.0.0".to_owned()),
            kind: "packagepack".to_owned(),
            published_file_id: Some("3762162551".to_owned()),
            dependencies: Vec::new(),
            dependency_details: Vec::new(),
            conflicts: Vec::new(),
            locations: Vec::new(),
            role: CatalogItemRole::Other,
            actions: Vec::new(),
        };
        session
            .catalog
            .replace(vec![entry.clone(), root_entry.clone()]);

        assert!(session.validate_auto_install_target(content_id).is_ok());

        session.durable_state.compositions.compositions[0].resolved_graph = None;
        assert!(
            session.validate_auto_install_target(content_id).is_err_and(
                |message| message.contains("owned by a subscribed or installed Packagepack")
            )
        );

        session.durable_state.compositions.compositions[0].resolved_graph =
            Some(vapor_client_protocol::ResolvedCompositionGraph {
                root,
                effective_engine: ContentReference::parse(
                    "author/engine/core".to_owned(),
                    Some("1.0.0".to_owned()),
                )
                .unwrap(),
                effective_game: None,
                nodes: vec![],
            });
        entry.published_file_id = None;
        session.catalog.replace(vec![entry, root_entry]);
        assert!(
            session
                .validate_auto_install_target(content_id)
                .is_err_and(|message| message.contains("Workshop"))
        );
    }

    fn wait_for_operation(session: &mut ClientSession) -> Vec<ClientEvent> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut events = Vec::new();
        while Instant::now() < deadline {
            events.extend(session.poll_background_operations());
            if !session.has_active_operation() {
                return events;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("fixture composition operation did not complete");
    }

    #[test]
    fn fixture_catalog_refresh_is_deterministic_and_advances_revision() {
        let mut session = session();
        let response = session.dispatch(request(
            Some(0),
            ClientAction::Catalog(CatalogAction::Refresh),
        ));
        assert_eq!(response.kind, ClientResponseKind::Applied);
        assert_eq!(response.revision, ClientRevision(1));
        assert_eq!(response.snapshot.catalog[0].id, "fixture.engine");
        assert_eq!(response.snapshot.host.mode, "fixture");
        assert!(!response.snapshot.workspace_content.source_projects_loaded);
        assert!(
            !response
                .snapshot
                .workspace_content
                .unregistered_projects_loaded
        );
    }

    #[test]
    fn auto_install_download_context_carries_all_dependent_constraints_and_compositions() {
        use vapor_client_protocol::{
            CatalogEntry, CatalogItemRole, CatalogLocation, CatalogLocationKind,
            ResolvedCompositionGraph, ResolvedDependencyIdentity, ResolvedNodeIdentity,
        };
        let mut session = session();
        let content_id = "publisher/project/movement";
        let content = ContentReference::parse(content_id.to_owned(), Some("1.0.0".to_owned()))
            .expect("valid content reference");
        let root = ContentReference::parse("author/game/pack".to_owned(), Some("1.0.0".to_owned()))
            .expect("valid Packagepack reference");
        session
            .durable_state
            .library_policies
            .push(vapor_client_protocol::LibraryItemPolicy {
                content_id: content.vapor_id().expect("valid Vapor ID"),
                auto_install: true,
                packagepack_owners: Vec::new(),
                user_selected: true,
            });
        session.durable_state.compositions.compositions[0].root_package = Some(root.clone());
        session.durable_state.compositions.compositions[0].installation =
            InstallationState::Installed {
                artifacts: Vec::new(),
                graph_fingerprint: None,
            };
        session.durable_state.compositions.compositions[0].resolved_graph =
            Some(ResolvedCompositionGraph {
                root: root.clone(),
                effective_engine: ContentReference::parse(
                    "author/engine/core".to_owned(),
                    Some("1.0.0".to_owned()),
                )
                .unwrap(),
                effective_game: Some(
                    ContentReference::parse(
                        "author/game/main".to_owned(),
                        Some("1.0.0".to_owned()),
                    )
                    .unwrap(),
                ),
                nodes: vec![
                    ResolvedNodeIdentity {
                        content: root,
                        kind: "packagepack".to_owned(),
                        fingerprint: None,
                        source: None,
                        dependencies: vec![ResolvedDependencyIdentity {
                            binding: "movement".to_owned(),
                            content: content.clone(),
                            version_requirement: Some("^1.0".to_owned()),
                        }],
                    },
                    ResolvedNodeIdentity {
                        content: content.clone(),
                        kind: "game-mod".to_owned(),
                        fingerprint: None,
                        source: None,
                        dependencies: Vec::new(),
                    },
                ],
            });
        session.catalog.replace(vec![CatalogEntry {
            id: content_id.to_owned(),
            name: Some("Movement".to_owned()),
            version: Some("1.0.0".to_owned()),
            kind: "game-mod".to_owned(),
            published_file_id: Some("3762162552".to_owned()),
            dependencies: Vec::new(),
            dependency_details: Vec::new(),
            conflicts: Vec::new(),
            locations: vec![CatalogLocation {
                kind: CatalogLocationKind::Cache,
                root: "/cache/movement".to_owned(),
                enabled: true,
                fingerprint: None,
                install_source: None,
            }],
            role: CatalogItemRole::Other,
            actions: Vec::new(),
        }]);

        let kind = WorkshopOperationKind::Acquire {
            ids: vec!["3762162552".to_owned()],
        };
        let context = session.auto_install_context(&kind);
        assert_eq!(context.constraints[content_id], vec!["^1.0".to_owned()]);
        assert_eq!(context.compositions[content_id].len(), 1);
        assert_eq!(
            context.compositions[content_id][0].composition.id,
            "default"
        );

        session
            .durable_state
            .compositions
            .compositions
            .push(Composition {
                id: "unresolved".to_owned(),
                name: "Unresolved composition".to_owned(),
                root_package: Some(
                    ContentReference::parse(
                        "author/game/other-pack".to_owned(),
                        Some("1.0.0".to_owned()),
                    )
                    .unwrap(),
                ),
                resolved_graph: None,
                workspace: None,
                launch: vapor_client_application::LaunchSettings::default(),
                installation: InstallationState::Unknown,
            });
        let context = session.auto_install_context(&kind);
        assert!(context.constraints.is_empty());
        assert!(context.compositions.is_empty());
        assert!(
            context
                .warning
                .as_deref()
                .is_some_and(|warning| warning.contains("has not been resolved"))
        );
    }

    #[test]
    fn fixture_workshop_search_is_owned_by_the_service_and_returns_results() {
        let mut session = session();
        let response = session.dispatch(request(
            Some(0),
            ClientAction::Workshop(vapor_client_protocol::WorkshopAction::Search {
                query: "moon".into(),
                page: 1,
                content_kind: Some(vapor_client_protocol::ContentKind::Game),
                required_tags: vec!["game".into()],
            }),
        ));

        assert_eq!(response.kind, ClientResponseKind::OperationAccepted);
        let events = wait_for_operation(&mut session);
        assert!(events.iter().any(|event| matches!(
            event,
            ClientEvent::OperationChanged(event)
                if event.action == "workshop.search"
                    && event.status == OperationStatus::Succeeded
        )));
        let snapshot = serde_json::to_value(session.snapshot()).expect("encode snapshot");
        assert_eq!(snapshot["workshop"]["search"]["query"], "moon");
        assert_eq!(snapshot["workshop"]["search"]["page"], 1);
        assert_eq!(snapshot["workshop"]["search"]["required_tags"][0], "game");
        assert_eq!(snapshot["workshop"]["search"]["content_kind"], "game");
        assert_eq!(
            snapshot["workshop"]["search"]["items"][0]["published_file_id"],
            "1002"
        );
        assert_eq!(
            snapshot["workshop"]["search"]["items"][0]["tags"][0],
            "game"
        );
    }

    #[test]
    fn fixture_workshop_acquisition_is_owned_by_the_service_and_updates_its_catalog() {
        let mut session = session();
        let response = session.dispatch(request(
            Some(0),
            ClientAction::Workshop(WorkshopAction::Acquire {
                item_ids: vec!["1002".into(), "1002".into()],
            }),
        ));

        assert_eq!(response.kind, ClientResponseKind::OperationAccepted);
        let events = wait_for_operation(&mut session);
        assert!(events.iter().any(|event| matches!(
            event,
            ClientEvent::OperationChanged(event)
                if event.action == "workshop.acquire"
                    && event.status == OperationStatus::Succeeded
        )));
        let snapshot = session.snapshot();
        let acquisition = snapshot
            .workshop
            .acquisition
            .expect("service publishes the acquired items");
        assert_eq!(acquisition.requested, 1);
        assert_eq!(acquisition.items.len(), 1);
        assert_eq!(acquisition.items[0].id, "fixture.content.1002");
        assert!(snapshot.catalog.iter().any(|item| {
            item.id == "fixture.content.1002"
                && item.locations.iter().any(|location| {
                    location.kind == CatalogLocationKind::Cache
                        && location.root == "fixture/cache/fixture.content.1002"
                })
        }));
    }

    #[test]
    fn fixture_workshop_review_and_publish_are_service_owned_and_confirmation_bound() {
        let mut session = fixture_workspace_session();
        session.refresh_catalog();
        assert!(
            session
                .snapshot()
                .catalog
                .iter()
                .any(|entry| { entry.id == "fixture.game" && !entry.conflicts.is_empty() })
        );
        let staged = session.dispatch(request(
            Some(0),
            ClientAction::Workshop(WorkshopAction::Stage {
                selector: "fixture.game@1.0.0".into(),
                target_triples: Vec::new(),
            }),
        ));
        assert_eq!(staged.kind, ClientResponseKind::OperationAccepted);
        let stage_events = wait_for_operation(&mut session);
        assert!(stage_events.iter().any(|event| matches!(
            event,
            ClientEvent::OperationChanged(event)
                if event.action == "workshop.stage"
                    && event.status == OperationStatus::Succeeded
        )));
        let review = session
            .snapshot()
            .workshop
            .review
            .expect("stage result is retained by the service");
        assert_eq!(review.selector, "fixture.game@1.0.0");
        assert_eq!(review.preview.file_paths, ["Vapor.toml"]);

        let rejected = session.dispatch(request(
            None,
            ClientAction::Workshop(WorkshopAction::Publish {
                review_id: review.review_id.clone(),
                account: "publisher".into(),
                confirmation: "PUBLISH fixture.game".into(),
                change_note: String::new(),
            }),
        ));
        assert_eq!(rejected.kind, ClientResponseKind::Rejected);
        assert!(session.snapshot().workshop.review.is_some());

        let published = session.dispatch(request(
            None,
            ClientAction::Workshop(WorkshopAction::Publish {
                review_id: review.review_id,
                account: "publisher".into(),
                confirmation: "PUBLISH fixture.game TO publisher".into(),
                change_note: "Fixture only".into(),
            }),
        ));
        assert_eq!(published.kind, ClientResponseKind::OperationAccepted);
        let publish_events = wait_for_operation(&mut session);
        assert!(publish_events.iter().any(|event| matches!(
            event,
            ClientEvent::OperationChanged(event)
                if event.action == "workshop.publish"
                    && event.status == OperationStatus::Succeeded
        )));
        assert!(session.snapshot().workshop.review.is_none());
    }

    #[test]
    fn fixture_workshop_batch_review_and_publish_are_bound_to_one_batch_confirmation() {
        let mut session = fixture_workspace_session();
        let staged = session.dispatch(request(
            Some(0),
            ClientAction::Workshop(WorkshopAction::StageBatch {
                selectors: vec!["fixture.game@1.0.0".into()],
                target_triples: Vec::new(),
            }),
        ));
        assert_eq!(staged.kind, ClientResponseKind::OperationAccepted);
        let stage_events = wait_for_operation(&mut session);
        assert!(stage_events.iter().any(|event| matches!(
            event,
            ClientEvent::OperationChanged(event)
                if event.action == "workshop.stage-batch"
                    && event.status == OperationStatus::Succeeded
        )));
        let review = session
            .snapshot()
            .workshop
            .batch_review
            .expect("the service retains the batch review");
        assert_eq!(review.items.len(), 1);
        assert_eq!(review.items[0].selector, "fixture.game@1.0.0");

        let duplicate = session.dispatch(request(
            None,
            ClientAction::Workshop(WorkshopAction::StageBatch {
                selectors: vec!["fixture.game@1.0.0".into(), "fixture.game@1.0.0".into()],
                target_triples: Vec::new(),
            }),
        ));
        assert_eq!(duplicate.kind, ClientResponseKind::Rejected);
        assert!(session.snapshot().workshop.batch_review.is_some());

        let rejected = session.dispatch(request(
            None,
            ClientAction::Workshop(WorkshopAction::PublishBatch {
                review_id: review.review_id.clone(),
                account: "publisher".into(),
                confirmation: "PUBLISH fixture.game TO publisher".into(),
                change_note: String::new(),
            }),
        ));
        assert_eq!(rejected.kind, ClientResponseKind::Rejected);
        assert!(session.snapshot().workshop.batch_review.is_some());

        let published = session.dispatch(request(
            None,
            ClientAction::Workshop(WorkshopAction::PublishBatch {
                review_id: review.review_id,
                account: "publisher".into(),
                confirmation: "PUBLISH 1 ITEM TO publisher".into(),
                change_note: "Fixture batch".into(),
            }),
        ));
        assert_eq!(published.kind, ClientResponseKind::OperationAccepted);
        let publish_events = wait_for_operation(&mut session);
        assert!(publish_events.iter().any(|event| matches!(
            event,
            ClientEvent::OperationChanged(event)
                if event.action == "workshop.publish-batch"
                    && event.status == OperationStatus::Succeeded
        )));
        assert!(session.snapshot().workshop.batch_review.is_none());
    }

    #[test]
    fn workshop_stage_requires_a_current_workspace_identity() {
        let mut session = session();
        let response = session.dispatch(request(
            None,
            ClientAction::Workshop(WorkshopAction::Stage {
                selector: "fixture.game@1.0.0".into(),
                target_triples: Vec::new(),
            }),
        ));
        assert_eq!(response.kind, ClientResponseKind::Rejected);
        assert_eq!(
            response.error.expect("workspace rejection").code,
            ServiceErrorCode::ActionUnavailable
        );
    }

    #[test]
    fn fixture_workshop_subscription_changes_return_the_service_owned_list() {
        let mut session = session();
        let loaded = session.dispatch(request(
            None,
            ClientAction::Workshop(WorkshopAction::LoadSubscriptions),
        ));
        assert_eq!(loaded.kind, ClientResponseKind::OperationAccepted);
        wait_for_operation(&mut session);
        assert!(session.snapshot().workshop.subscriptions_loaded);
        assert_eq!(
            session.snapshot().workshop.subscriptions[0].published_file_id,
            "1001"
        );

        let subscribed = session.dispatch(request(
            None,
            ClientAction::Workshop(WorkshopAction::Subscribe {
                item_ids: vec!["1002".into(), "1002".into()],
            }),
        ));
        assert_eq!(subscribed.kind, ClientResponseKind::OperationAccepted);
        wait_for_operation(&mut session);
        let ids = session
            .snapshot()
            .workshop
            .subscriptions
            .iter()
            .map(|item| item.published_file_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["1001", "1002"]);
        let after_subscribe = session.snapshot();
        assert!(after_subscribe.catalog.iter().any(|item| {
            item.id == "fixture.content.1002"
                && item
                    .locations
                    .iter()
                    .any(|location| location.kind == CatalogLocationKind::Cache && location.enabled)
        }));

        let unsubscribed = session.dispatch(request(
            None,
            ClientAction::Workshop(WorkshopAction::Unsubscribe {
                item_ids: vec!["1002".into()],
            }),
        ));
        assert_eq!(unsubscribed.kind, ClientResponseKind::OperationAccepted);
        wait_for_operation(&mut session);
        let after_unsubscribe = session.snapshot();
        let ids = after_unsubscribe
            .workshop
            .subscriptions
            .iter()
            .map(|item| item.published_file_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["1001"]);
        assert!(
            after_unsubscribe.catalog.iter().any(|item| {
                item.id == "fixture.content.1002"
                    && item.locations.iter().any(|location| {
                        location.kind == CatalogLocationKind::Cache && location.enabled
                    })
            }),
            "unsubscribing must not erase the independent Library copy"
        );

        let invalid = session.dispatch(request(
            None,
            ClientAction::Workshop(WorkshopAction::Subscribe {
                item_ids: vec!["not-an-id".into()],
            }),
        ));
        assert_eq!(invalid.kind, ClientResponseKind::Rejected);
        assert_eq!(
            invalid.error.expect("invalid ID rejection").code,
            ServiceErrorCode::InvalidRequest
        );
    }

    #[test]
    fn fixture_preview_is_composition_scoped_and_returns_a_typed_plan() {
        let mut session = session();
        let response = session.dispatch(request(
            Some(0),
            ClientAction::Composition(vapor_client_protocol::CompositionAction::Preview {
                composition_id: "default".into(),
                selector: Some("fixture.pack@1.0.0".into()),
            }),
        ));
        assert_eq!(response.kind, ClientResponseKind::OperationAccepted);
        assert_eq!(
            response
                .snapshot
                .active_operation
                .as_ref()
                .unwrap()
                .composition_id
                .as_deref(),
            Some("default")
        );

        let events = wait_for_operation(&mut session);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    ClientEvent::OperationChanged(OperationEvent {
                        status: OperationStatus::Succeeded,
                        ..
                    })
                ))
                .count(),
            1,
            "composition persistence must contribute exactly one terminal success event"
        );
        let preview = session.snapshot().composition_preview.unwrap();
        assert_eq!(preview.composition_id, "default");
        assert_eq!(preview.selector, "fixture.pack@1.0.0");
        assert_eq!(preview.nodes[0].kind, "packagepack");
        assert!(preview.warnings[0].contains("Fixture mode"));
    }

    #[test]
    fn fixture_install_updates_service_composition_without_launching_a_process() {
        let mut session = session();
        let root = ContentReference {
            id: "fixture.pack".into(),
            version: Some("1.0.0".into()),
        };
        let assigned = session.dispatch(request(
            Some(0),
            ClientAction::Composition(CompositionAction::SetRoot {
                composition_id: "default".into(),
                content_id: Some(root.id.clone()),
                content_version: root.version.clone(),
                workspace_root: Some("/fixture/workspace".into()),
            }),
        ));
        assert_eq!(assigned.kind, ClientResponseKind::Applied);

        let accepted = session.dispatch(request(
            Some(1),
            ClientAction::Composition(vapor_client_protocol::CompositionAction::Install {
                composition_id: "default".into(),
            }),
        ));
        assert_eq!(accepted.kind, ClientResponseKind::OperationAccepted);
        let events = wait_for_operation(&mut session);
        assert!(events.iter().any(|event| matches!(
            event,
            ClientEvent::OperationChanged(OperationEvent {
                status: OperationStatus::Succeeded,
                ..
            })
        )));
        let composition = &session.snapshot().durable_state.compositions.compositions[0];
        assert_eq!(composition.resolved_graph.as_ref().unwrap().root, root);
        assert!(matches!(
            &composition.installation,
            InstallationState::Installed { .. }
        ));
    }

    #[test]
    fn library_uninstall_is_rejected_when_a_composition_owns_the_content() {
        let mut session = session();
        let assigned = session.dispatch(request(
            Some(0),
            ClientAction::Composition(CompositionAction::SetRoot {
                composition_id: "default".into(),
                content_id: Some("fixture.pack".into()),
                content_version: Some("1.0.0".into()),
                workspace_root: None,
            }),
        ));
        assert_eq!(assigned.kind, ClientResponseKind::Applied);

        let rejected = session.dispatch(request(
            Some(1),
            ClientAction::Library(LibraryAction::Uninstall {
                selector: "fixture.pack".into(),
            }),
        ));

        assert_eq!(rejected.kind, ClientResponseKind::Rejected);
        assert!(
            rejected
                .error
                .as_ref()
                .is_some_and(|error| error.message.contains("composition 'default'"))
        );
        assert!(!session.has_active_operation());
    }

    #[test]
    fn obsolete_install_completion_does_not_replace_a_new_composition() {
        let mut session = session();
        let first_root = ContentReference {
            id: "fixture.first".into(),
            version: Some("1.0.0".into()),
        };
        let assigned_first = session.dispatch(request(
            Some(0),
            ClientAction::Composition(CompositionAction::SetRoot {
                composition_id: "default".into(),
                content_id: Some(first_root.id.clone()),
                content_version: first_root.version.clone(),
                workspace_root: Some("/fixture/workspace".into()),
            }),
        ));
        assert_eq!(assigned_first.kind, ClientResponseKind::Applied);

        let first_composition = session.durable_state.compositions.compositions[0].clone();
        let inputs = CompositionOperationInputs::capture(
            &first_composition,
            CompositionOperationKind::Install,
        );
        let result = fixture_composition_result(
            &first_composition,
            "fixture.first@1.0.0".into(),
            CompositionOperationKind::Install,
        );
        register_test_operation(
            &mut session,
            OperationSummary {
                id: "operation-install-first".into(),
                composition_id: Some("default".into()),
                action: "composition.install".into(),
                phase: OperationPhase::Installing,
                status: OperationStatus::Running,
                progress: None,
            },
            None,
            false,
        );

        let second_root = ContentReference {
            id: "fixture.second".into(),
            version: Some("2.0.0".into()),
        };
        let assigned_second = session.dispatch(request(
            Some(1),
            ClientAction::Composition(CompositionAction::SetRoot {
                composition_id: "default".into(),
                content_id: Some(second_root.id.clone()),
                content_version: second_root.version.clone(),
                workspace_root: Some("/fixture/workspace".into()),
            }),
        ));
        assert_eq!(assigned_second.kind, ClientResponseKind::Applied);

        session.apply_composition_completion(CompositionOperationCompletion {
            id: "operation-install-first".into(),
            composition_id: "default".into(),
            action: CompositionOperationKind::Install,
            inputs,
            result,
        });

        let composition = &session.durable_state.compositions.compositions[0];
        assert_eq!(composition.root_package.as_ref(), Some(&second_root));
        assert!(composition.resolved_graph.is_none());
        assert_eq!(composition.installation, InstallationState::Unknown);
        assert!(!session.has_active_operation());
        assert!(session.operation_events.iter().any(|event| matches!(
            event,
            ClientEvent::OperationChanged(OperationEvent {
                id,
                status: OperationStatus::Succeeded,
                message: Some(message),
                ..
            }) if id == "operation-install-first"
                && message.contains("previous composition selection")
                && message.contains("Review installed content")
        )));
    }

    #[test]
    fn verification_failure_has_one_terminal_event_after_composition_reconciliation() {
        let mut session = session();
        let composition = session.durable_state.compositions.compositions[0].clone();
        register_test_operation(
            &mut session,
            OperationSummary {
                id: "operation-verify".to_owned(),
                composition_id: Some("default".to_owned()),
                action: "composition.verify".to_owned(),
                phase: OperationPhase::Verifying,
                status: OperationStatus::Running,
                progress: None,
            },
            None,
            false,
        );
        session.apply_composition_completion(CompositionOperationCompletion {
            id: "operation-verify".to_owned(),
            composition_id: "default".to_owned(),
            action: CompositionOperationKind::Verify,
            inputs: CompositionOperationInputs::capture(
                &composition,
                CompositionOperationKind::Verify,
            ),
            result: CompositionOperationResult::Verify(Ok(
                vapor_client_application::InstallationCheck {
                    ok: false,
                    detail: vec!["packagepack payload is missing".to_owned()],
                    artifacts: Vec::new(),
                    graph: None,
                    update_required: None,
                },
            )),
        });

        assert!(!session.has_active_operation());
        assert!(matches!(
            &session.durable_state.compositions.compositions[0].installation,
            InstallationState::NeedsRepair { .. }
        ));
        let terminal_events = session
            .operation_events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    ClientEvent::OperationChanged(OperationEvent {
                        id,
                        status: OperationStatus::Failed { .. },
                        ..
                    }) if id == "operation-verify"
                )
            })
            .count();
        assert_eq!(terminal_events, 1);
    }

    #[test]
    fn verification_marks_a_host_incompatible_packagepack_for_update() {
        let mut session = session();
        session.durable_state.compositions.compositions[0].root_package = Some(
            ContentReference::parse(
                "ghf-studios/loo-cast/loo-cast-packagepack".to_owned(),
                Some("0.5.0".to_owned()),
            )
            .expect("valid Packagepack identity"),
        );
        let root = session.durable_state.compositions.compositions[0]
            .root_package
            .clone()
            .expect("root package");
        let graph = vapor_client_protocol::ResolvedCompositionGraph {
            root: root.clone(),
            effective_engine: ContentReference::parse(
                "ghf-studios/loo-cast/spacetime-engine".to_owned(),
                Some("0.5.0".to_owned()),
            )
            .expect("valid Engine identity"),
            effective_game: Some(
                ContentReference::parse(
                    "ghf-studios/loo-cast/loo-cast-game".to_owned(),
                    Some("0.5.0".to_owned()),
                )
                .expect("valid Game identity"),
            ),
            nodes: Vec::new(),
        };
        let composition = session.durable_state.compositions.compositions[0].clone();
        register_test_operation(
            &mut session,
            running_operation("operation-verify-target", "composition.verify"),
            None,
            false,
        );
        session.apply_composition_completion(CompositionOperationCompletion {
            id: "operation-verify-target".to_owned(),
            composition_id: "default".to_owned(),
            action: CompositionOperationKind::Verify,
            inputs: CompositionOperationInputs::capture(
                &composition,
                CompositionOperationKind::Verify,
            ),
            result: CompositionOperationResult::Verify(Ok(
                vapor_client_application::InstallationCheck {
                    ok: true,
                    detail: vec!["3 content items verified".to_owned()],
                    artifacts: Vec::new(),
                    graph: Some(graph.clone()),
                    update_required: Some(
                        "This setup is not ready to play on this device: no Windows build".into(),
                    ),
                },
            )),
        });

        assert!(matches!(
            &session.durable_state.compositions.compositions[0].installation,
            InstallationState::OutOfDate {
                detail,
                actual_graph,
                verified: true,
                ..
            } if detail.contains("not ready to play") && actual_graph == &graph
        ));
    }

    #[test]
    fn workspace_project_discovery_is_a_service_action() {
        let mut session = ClientSession::new(ClientHostConfig {
            application_executable: PathBuf::from("/app/bin/vapor-gui"),
            workshop_app_id: None,
            workspace_root: Some(PathBuf::from("/fixture/workspace")),
            active_composition_id: None,
            fixture_mode: true,
            legacy_storage: vapor_client_protocol::LegacyStorageSeed::default(),
            state_directory: None,
        });
        let response = session.dispatch(request(
            Some(0),
            ClientAction::Catalog(CatalogAction::DiscoverContentProjects),
        ));

        assert_eq!(response.kind, ClientResponseKind::Applied);
        assert_eq!(response.revision, ClientRevision(1));
        assert_eq!(
            response.snapshot.workspace_content.source_projects[0].id,
            "fixture.game"
        );
        assert_eq!(
            response.snapshot.workspace_content.unregistered_projects[0].id,
            "fixture.addon"
        );
        assert!(response.snapshot.catalog.iter().all(|entry| {
            entry
                .locations
                .iter()
                .all(|location| location.kind != CatalogLocationKind::Workspace)
        }));
    }

    #[test]
    fn workspace_discovery_is_unavailable_until_a_workspace_is_open() {
        let mut session = session();
        let response = session.dispatch(request(
            Some(0),
            ClientAction::Catalog(CatalogAction::DiscoverContentProjects),
        ));

        assert_eq!(response.kind, ClientResponseKind::Rejected);
        assert_eq!(response.revision, ClientRevision(0));
        assert_eq!(
            response.error.unwrap().code,
            ServiceErrorCode::ActionUnavailable
        );
        let action = response
            .snapshot
            .available_actions
            .into_iter()
            .find(|action| action.id == "catalog.discover-content-projects")
            .expect("availability projection contains workspace discovery");
        assert!(!action.enabled);
        assert_eq!(action.reason.as_deref(), Some("Open a workspace first."));
    }

    #[test]
    fn workspace_open_returns_source_and_unregistered_projects_in_its_snapshot() {
        let mut session = ClientSession::new(ClientHostConfig {
            application_executable: PathBuf::from("/app/bin/vapor-gui"),
            workshop_app_id: None,
            workspace_root: None,
            active_composition_id: None,
            fixture_mode: true,
            legacy_storage: vapor_client_protocol::LegacyStorageSeed::default(),
            state_directory: None,
        });
        let root = fixture_temp_workspace("fixture-workspace-open");
        let response = session.dispatch(request(
            Some(0),
            ClientAction::Workspace(vapor_client_protocol::WorkspaceAction::Open {
                root: root.display().to_string(),
            }),
        ));

        assert_eq!(response.kind, ClientResponseKind::Applied);
        let content = response.snapshot.workspace_content;
        assert!(content.source_projects_loaded);
        assert_eq!(content.source_projects[0].id, "fixture.game");
        assert!(content.unregistered_projects_loaded);
        assert_eq!(content.unregistered_projects[0].id, "fixture.addon");
        assert_eq!(content.unregistered_diagnostics, ["fixture diagnostic"]);
        std::fs::remove_dir_all(root).expect("remove fixture workspace");
    }

    #[test]
    fn fixture_mode_refuses_to_open_a_workspace_outside_its_temporary_root() {
        let mut session = ClientSession::new(ClientHostConfig {
            application_executable: PathBuf::from("/app/bin/vapor-gui"),
            workshop_app_id: None,
            workspace_root: None,
            active_composition_id: None,
            fixture_mode: true,
            legacy_storage: vapor_client_protocol::LegacyStorageSeed::default(),
            state_directory: None,
        });
        let outside_fixture = std::env::current_dir().expect("current directory");

        let response = session.dispatch(request(
            Some(0),
            ClientAction::Workspace(vapor_client_protocol::WorkspaceAction::Open {
                root: outside_fixture.display().to_string(),
            }),
        ));

        assert_eq!(response.kind, ClientResponseKind::Rejected);
        assert_eq!(response.revision, ClientRevision(0));
        assert_eq!(
            response.error.expect("fixture boundary error").code,
            ServiceErrorCode::ActionUnavailable
        );
        assert!(response.snapshot.host.workspace.is_none());
    }

    #[test]
    fn fixture_mode_refuses_registry_and_authoring_operations() {
        let root = fixture_temp_workspace("fixture-guarded-workspace");
        let mut session = ClientSession::new(ClientHostConfig {
            application_executable: PathBuf::from("/app/bin/vapor-gui"),
            workshop_app_id: None,
            workspace_root: Some(root.clone()),
            active_composition_id: None,
            fixture_mode: true,
            legacy_storage: vapor_client_protocol::LegacyStorageSeed::default(),
            state_directory: None,
        });

        let remember = session.dispatch(request(
            None,
            ClientAction::Workspace(WorkspaceAction::Remember),
        ));
        assert_eq!(remember.kind, ClientResponseKind::Rejected);
        assert_eq!(
            remember.error.expect("registry boundary error").code,
            ServiceErrorCode::ActionUnavailable
        );

        let registry = session.dispatch(request(
            None,
            ClientAction::Authoring(AuthoringAction::ListRegisteredSources),
        ));
        assert_eq!(registry.kind, ClientResponseKind::Rejected);
        assert_eq!(
            registry.error.expect("authoring boundary error").code,
            ServiceErrorCode::ActionUnavailable
        );
        assert!(root.exists());
        std::fs::remove_dir_all(root).expect("remove fixture workspace");
    }

    #[test]
    fn telemetry_subscription_is_a_nonmutating_service_control_action() {
        let mut session = session();
        let response = session.dispatch(request(
            None,
            ClientAction::Developer(DeveloperAction::SubscribeTelemetry { enabled: true }),
        ));

        assert_eq!(response.kind, ClientResponseKind::Applied);
        assert_eq!(response.revision, ClientRevision(0));
    }

    #[test]
    fn fixture_sessions_reject_installation_role_mutations() {
        for action in [
            DeveloperAction::PromoteInstallationRole {
                target: "composer".to_owned(),
            },
            DeveloperAction::DemoteInstallationRole {
                target: "player".to_owned(),
            },
        ] {
            let mut session = session();
            let response = session.dispatch(request(None, ClientAction::Developer(action)));

            assert_eq!(response.kind, ClientResponseKind::Rejected);
            assert_eq!(response.revision, ClientRevision(0));
            assert_eq!(
                response.error.expect("fixture boundary error").code,
                ServiceErrorCode::ActionUnavailable
            );
        }
    }

    #[test]
    fn service_supplies_the_canonical_workshop_app_id_for_production_hosts() {
        let mut production = session().bootstrap;
        production.fixture_mode = false;
        production.workshop_app_id = None;
        ensure_workshop_app_id(&mut production);
        assert_eq!(
            production.workshop_app_id,
            Some(vapor_sdk_core::root::types::ROOT_STEAM_APP_ID)
        );

        let mut fixture = session().bootstrap;
        fixture.workshop_app_id = None;
        ensure_workshop_app_id(&mut fixture);
        assert_eq!(fixture.workshop_app_id, None);
    }

    #[test]
    fn fixture_sessions_reject_managed_profile_library_access() {
        for action in [
            DeveloperAction::RefreshProfileTraces,
            DeveloperAction::ImportProfileTrace {
                path: "/captures/example.tracy".to_owned(),
            },
            DeveloperAction::AnalyzeProfileTrace {
                trace_id: "trace-1".to_owned(),
                options: vapor_client_protocol::ProfileAnalysisOptions {
                    top: 10,
                    sort: vapor_client_protocol::ProfileTraceSort::P95,
                    name_filter: None,
                    self_time: false,
                },
            },
        ] {
            let mut session = session();
            let response = session.dispatch(request(None, ClientAction::Developer(action)));

            assert_eq!(response.kind, ClientResponseKind::Rejected);
            assert_eq!(
                response.error.expect("fixture boundary error").code,
                ServiceErrorCode::ActionUnavailable
            );
            assert!(response.snapshot.profile_traces.is_empty());
        }
    }

    #[test]
    fn profile_analysis_completion_clears_the_operation_and_returns_the_report_event() {
        let mut session = session();
        register_test_operation(
            &mut session,
            OperationSummary {
                id: "profile-analysis-1".to_owned(),
                composition_id: None,
                action: "analyze-trace".to_owned(),
                phase: OperationPhase::Finalizing,
                status: OperationStatus::Running,
                progress: None,
            },
            None,
            false,
        );
        let report = ProfileReportSnapshot {
            trace: PathBuf::from("/captures/run.tracy"),
            self_time: false,
            occurrence_count: 1,
            zone_count: 1,
            zones: Vec::new(),
        };

        session.apply_profile_analysis_completion(ProfileAnalysisCompletion {
            id: "profile-analysis-1".to_owned(),
            result: Ok(report.clone()),
        });

        let events = session.poll_background_operations();
        assert!(session.snapshot().active_operation.is_none());
        assert!(events.iter().any(|event| matches!(
            event,
            ClientEvent::OperationChanged(OperationEvent {
                status: OperationStatus::Succeeded,
                ..
            })
        )));
        assert!(events.iter().any(|event| {
            matches!(event, ClientEvent::ProfileAnalysis(Ok(result)) if result == &report)
        }));
    }

    #[test]
    fn workspace_file_lifecycle_runs_through_the_service_and_guards_external_edits() {
        let root = std::env::temp_dir().join(format!(
            "vapor-client-service-workspace-files-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock follows epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create fixture workspace");
        std::fs::write(root.join("Vapor.toml"), "before").expect("write fixture manifest");
        let root = std::fs::canonicalize(root).expect("canonicalize fixture workspace");
        let mut session = ClientSession::new(ClientHostConfig {
            application_executable: PathBuf::from("/app/bin/vapor-gui"),
            workshop_app_id: None,
            workspace_root: Some(root.clone()),
            active_composition_id: Some("default".to_owned()),
            fixture_mode: true,
            legacy_storage: vapor_client_protocol::LegacyStorageSeed::default(),
            state_directory: None,
        });

        let response = session.dispatch(request(
            None,
            ClientAction::Workspace(WorkspaceAction::ListFiles { path: ".".into() }),
        ));
        assert_eq!(response.kind, ClientResponseKind::OperationAccepted);
        let events = wait_for_operation(&mut session);
        assert!(events.iter().any(|event| matches!(
            event,
            ClientEvent::WorkspaceFileResult(result)
                if matches!(result.as_ref(), WorkspaceFileResult::Listed { files, .. }
                    if files == &["Vapor.toml"])
        )));

        let response = session.dispatch(request(
            None,
            ClientAction::Workspace(WorkspaceAction::OpenFile {
                path: "Vapor.toml".into(),
            }),
        ));
        assert_eq!(response.kind, ClientResponseKind::OperationAccepted);
        let events = wait_for_operation(&mut session);
        let fingerprint = events
            .iter()
            .find_map(|event| match event {
                ClientEvent::WorkspaceFileResult(result) => match result.as_ref() {
                    WorkspaceFileResult::Opened {
                        contents,
                        fingerprint,
                        ..
                    } if contents == "before" => Some(fingerprint.clone()),
                    _ => None,
                },
                _ => None,
            })
            .expect("open returns text and its save fingerprint");

        std::fs::write(root.join("Vapor.toml"), "external").expect("simulate external edit");
        let response = session.dispatch(request(
            None,
            ClientAction::Workspace(WorkspaceAction::SaveFile {
                path: "Vapor.toml".into(),
                expected_fingerprint: fingerprint,
                contents: "mine".into(),
            }),
        ));
        assert_eq!(response.kind, ClientResponseKind::OperationAccepted);
        let events = wait_for_operation(&mut session);
        assert!(events.iter().any(|event| matches!(
            event,
            ClientEvent::OperationChanged(event)
                if event.action == "workspace.save-file"
                    && matches!(&event.status, OperationStatus::Failed { message }
                        if message.contains("changed on disk"))
        )));
        assert_eq!(
            std::fs::read_to_string(root.join("Vapor.toml")).expect("read after stale save"),
            "external"
        );

        let response = session.dispatch(request(
            None,
            ClientAction::Workspace(WorkspaceAction::CreateFile {
                path: "nested/new.toml".into(),
                contents: String::new(),
            }),
        ));
        assert_eq!(response.kind, ClientResponseKind::OperationAccepted);
        let events = wait_for_operation(&mut session);
        assert!(events.iter().any(|event| matches!(
            event,
            ClientEvent::WorkspaceFileResult(result)
                if matches!(result.as_ref(), WorkspaceFileResult::Created { path, .. }
                    if path == "nested/new.toml")
        )));
        assert!(root.join("nested/new.toml").is_file());
        std::fs::remove_dir_all(root).expect("remove fixture workspace");
    }

    #[test]
    fn opening_authoring_workspace_preserves_playable_composition_and_launch_settings() {
        let mut session = ClientSession::new(ClientHostConfig {
            application_executable: PathBuf::from("/app/bin/vapor-gui"),
            workshop_app_id: None,
            workspace_root: None,
            active_composition_id: None,
            fixture_mode: true,
            legacy_storage: vapor_client_protocol::LegacyStorageSeed::default(),
            state_directory: None,
        });
        let assigned = session.dispatch(request(
            Some(0),
            ClientAction::Composition(CompositionAction::SetRoot {
                composition_id: "default".to_owned(),
                content_id: Some("old.game".to_owned()),
                content_version: Some("1.0".to_owned()),
                workspace_root: Some("/fixture/composition-source".to_owned()),
            }),
        ));
        assert_eq!(assigned.kind, ClientResponseKind::Applied);

        let launch = session.dispatch(request(
            Some(1),
            ClientAction::Composition(CompositionAction::SetLaunchArguments {
                composition_id: "default".to_owned(),
                arguments: vec!["--flight-mode".to_owned(), "safe".to_owned()],
            }),
        ));
        assert_eq!(launch.kind, ClientResponseKind::Applied);

        let installed = session.dispatch(request(
            Some(2),
            ClientAction::Composition(vapor_client_protocol::CompositionAction::Install {
                composition_id: "default".to_owned(),
            }),
        ));
        assert_eq!(installed.kind, ClientResponseKind::OperationAccepted);
        wait_for_operation(&mut session);
        let composition_before = session.durable_state.compositions.compositions[0].clone();
        let composition_revision_before = session.durable_state.composition_revision;

        let root = fixture_temp_workspace("fixture-switch-workspace");
        let opened = session.dispatch(request(
            Some(session.revision.0),
            ClientAction::Workspace(vapor_client_protocol::WorkspaceAction::Open {
                root: root.display().to_string(),
            }),
        ));
        assert_eq!(opened.kind, ClientResponseKind::Applied);
        let composition = &opened.snapshot.durable_state.compositions.compositions[0];
        assert_eq!(composition, &composition_before);
        assert_eq!(
            opened.snapshot.durable_state.composition_revision,
            composition_revision_before
        );
        assert_eq!(
            opened
                .snapshot
                .durable_state
                .authoring_workspace_root
                .as_deref(),
            Some(root.to_string_lossy().as_ref())
        );
        std::fs::remove_dir_all(root).expect("remove fixture workspace");
    }

    #[test]
    fn selecting_a_composition_does_not_change_the_authoring_workspace() {
        let authoring_root = fixture_temp_workspace("fixture-independent-authoring-workspace");
        let linked_root = fixture_temp_workspace("fixture-composition-source-workspace");
        let mut session = ClientSession::new(ClientHostConfig {
            application_executable: PathBuf::from("/app/bin/vapor-gui"),
            workshop_app_id: None,
            workspace_root: Some(authoring_root.clone()),
            active_composition_id: None,
            fixture_mode: true,
            legacy_storage: vapor_client_protocol::LegacyStorageSeed::default(),
            state_directory: None,
        });
        session.durable_state.compositions.compositions[0].workspace = Some(WorkspaceAssociation {
            root: linked_root.clone(),
        });

        let selected = session.dispatch(request(
            Some(0),
            ClientAction::Composition(CompositionAction::Select {
                composition_id: "default".to_owned(),
            }),
        ));

        assert_eq!(selected.kind, ClientResponseKind::Applied);
        assert_eq!(
            selected
                .snapshot
                .durable_state
                .authoring_workspace_root
                .as_deref(),
            Some(authoring_root.to_string_lossy().as_ref())
        );
        assert_eq!(
            selected.snapshot.durable_state.compositions.compositions[0]
                .workspace
                .as_ref()
                .map(|workspace| &workspace.root),
            Some(&linked_root)
        );
        std::fs::remove_dir_all(authoring_root).expect("remove authoring workspace");
        std::fs::remove_dir_all(linked_root).expect("remove source workspace");
    }

    #[test]
    fn creating_a_composition_does_not_implicitly_bind_the_authoring_workspace() {
        let authoring_root = fixture_temp_workspace("fixture-new-composition-workspace");
        let mut session = ClientSession::new(ClientHostConfig {
            application_executable: PathBuf::from("/app/bin/vapor-gui"),
            workshop_app_id: None,
            workspace_root: None,
            active_composition_id: None,
            fixture_mode: true,
            legacy_storage: vapor_client_protocol::LegacyStorageSeed::default(),
            state_directory: None,
        });
        let opened = session.dispatch(request(
            Some(0),
            ClientAction::Workspace(WorkspaceAction::Open {
                root: authoring_root.display().to_string(),
            }),
        ));
        assert_eq!(opened.kind, ClientResponseKind::Applied);

        let created = session.dispatch(request(
            Some(1),
            ClientAction::Composition(CompositionAction::Create {
                name: "New composition".to_owned(),
            }),
        ));

        assert_eq!(created.kind, ClientResponseKind::Applied);
        let composition = created
            .snapshot
            .durable_state
            .compositions
            .compositions
            .iter()
            .find(|composition| {
                composition.id == created.snapshot.durable_state.active_composition_id
            })
            .expect("new composition is active");
        assert!(composition.workspace.is_none());
        assert_eq!(
            created
                .snapshot
                .durable_state
                .authoring_workspace_root
                .as_deref(),
            Some(authoring_root.to_string_lossy().as_ref())
        );
        std::fs::remove_dir_all(authoring_root).expect("remove authoring workspace");
    }

    #[test]
    fn create_from_content_requires_available_packagepack_and_selects_it_atomically() {
        let mut session = session();
        session.catalog.replace(vec![CatalogEntry {
            id: "examples/wheel".into(),
            name: Some("Wheel Salvage Demo".into()),
            version: Some("0.1.1".into()),
            kind: "packagepack".into(),
            published_file_id: Some("3823734148".into()),
            dependencies: vec![],
            dependency_details: vec![],
            conflicts: vec![],
            locations: vec![CatalogLocation {
                kind: CatalogLocationKind::Cache,
                root: "fixture/cache/examples-wheel".into(),
                enabled: true,
                fingerprint: Some("fixture:wheel".into()),
                install_source: None,
            }],
            role: CatalogItemRole::PlayableComposition,
            actions: vec![],
        }]);

        let created = session.dispatch(request(
            Some(0),
            ClientAction::Composition(CompositionAction::CreateFromContent {
                name: "Wheel Salvage Demo".into(),
                content_id: "examples/wheel".into(),
                content_version: "0.1.1".into(),
            }),
        ));

        assert_eq!(created.kind, ClientResponseKind::Applied);
        let active = created
            .snapshot
            .durable_state
            .compositions
            .compositions
            .iter()
            .find(|composition| {
                composition.id == created.snapshot.durable_state.active_composition_id
            })
            .expect("new composition is active");
        assert_eq!(active.name, "Wheel Salvage Demo");
        assert_eq!(
            active.root_package,
            Some(ContentReference {
                id: "examples/wheel".into(),
                version: Some("0.1.1".into()),
            })
        );
        assert!(active.workspace.is_none());
        assert_eq!(active.installation, InstallationState::Unknown);
        assert_eq!(created.revision, ClientRevision(1));
    }

    #[test]
    fn create_from_content_rejects_unavailable_or_non_packagepack_content() {
        let mut session = session();
        session.catalog.replace(vec![CatalogEntry {
            id: "examples/wheel".into(),
            name: Some("Wheel Salvage Demo".into()),
            version: Some("0.1.1".into()),
            kind: "game".into(),
            published_file_id: Some("3823734148".into()),
            dependencies: vec![],
            dependency_details: vec![],
            conflicts: vec![],
            locations: vec![],
            role: CatalogItemRole::RuntimeContent,
            actions: vec![],
        }]);

        let rejected = session.dispatch(request(
            Some(0),
            ClientAction::Composition(CompositionAction::CreateFromContent {
                name: "Wheel Salvage Demo".into(),
                content_id: "examples/wheel".into(),
                content_version: "0.1.1".into(),
            }),
        ));

        assert_eq!(rejected.kind, ClientResponseKind::Rejected);
        assert_eq!(rejected.revision, ClientRevision(0));
        assert_eq!(
            rejected
                .snapshot
                .durable_state
                .compositions
                .compositions
                .len(),
            1
        );
    }

    fn fixture_temp_workspace(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "vapor-studio-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock follows epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create isolated fixture workspace");
        std::fs::canonicalize(root).expect("canonicalize isolated fixture workspace")
    }

    #[test]
    fn catalog_search_is_a_projection_without_destroying_shared_facts() {
        let mut session = session();
        let _ = session.dispatch(request(
            Some(0),
            ClientAction::Catalog(CatalogAction::Refresh),
        ));
        let filtered = session.dispatch(request(
            Some(1),
            ClientAction::Catalog(CatalogAction::Search {
                query: "missing".into(),
            }),
        ));
        assert_eq!(filtered.snapshot.catalog.len(), 1);
        assert!(filtered.snapshot.catalog_search.matches.is_empty());
        assert_eq!(filtered.snapshot.catalog_search.query, "missing");
        assert!(filtered.snapshot.catalog_status.is_current());
        let restored = session.dispatch(request(
            Some(2),
            ClientAction::Catalog(CatalogAction::Search {
                query: String::new(),
            }),
        ));
        assert_eq!(restored.snapshot.catalog.len(), 1);
    }

    #[test]
    fn stale_action_is_rejected_without_mutating_state() {
        let mut session = session();
        let _ = session.dispatch(request(
            Some(0),
            ClientAction::Catalog(CatalogAction::Refresh),
        ));
        let response = session.dispatch(request(
            Some(0),
            ClientAction::Catalog(CatalogAction::Inspect {
                content_id: "fixture.engine".to_owned(),
                version: Some("1.0.0".to_owned()),
            }),
        ));
        assert_eq!(response.kind, ClientResponseKind::Rejected);
        assert_eq!(
            response.error.unwrap().code,
            ServiceErrorCode::StaleRevision
        );
        assert_eq!(response.revision, ClientRevision(1));
        assert_eq!(response.snapshot.selected_content_id, None);
    }

    #[test]
    fn composition_selection_requires_an_existing_composition() {
        let mut session = ClientSession::new(ClientHostConfig {
            application_executable: PathBuf::from("/app/bin/vapor-gui"),
            workshop_app_id: None,
            workspace_root: None,
            active_composition_id: None,
            fixture_mode: false,
            legacy_storage: vapor_client_protocol::LegacyStorageSeed::default(),
            state_directory: None,
        });
        let created = session.dispatch(request(
            Some(0),
            ClientAction::Composition(CompositionAction::Create {
                name: "Second".to_owned(),
            }),
        ));
        assert_eq!(created.kind, ClientResponseKind::Applied);
        assert_eq!(created.revision, ClientRevision(1));
        assert_eq!(
            created.snapshot.active_composition_id.as_deref(),
            Some("composition-1")
        );
        assert_eq!(
            created
                .snapshot
                .durable_state
                .compositions
                .compositions
                .len(),
            2
        );
        assert_eq!(created.snapshot.durable_state.composition_revision, 1);
        let rejected = session.dispatch(request(
            Some(1),
            ClientAction::Composition(CompositionAction::Select {
                composition_id: "unknown".to_owned(),
            }),
        ));
        assert_eq!(rejected.kind, ClientResponseKind::Rejected);
        assert_eq!(rejected.revision, ClientRevision(1));
    }

    #[test]
    fn fixture_default_composition_root_is_resolved_and_persisted_by_service() {
        let mut session = session();
        let initially_disabled = session
            .snapshot()
            .available_actions
            .into_iter()
            .find(|action| action.id == "composition.launch")
            .expect("launch readiness is projected");
        assert!(!initially_disabled.enabled);
        assert_eq!(
            initially_disabled.reason.as_deref(),
            Some("Choose a playable composition first.")
        );
        for action_id in [
            "composition.install",
            "composition.verify",
            "composition.repair",
            "composition.update",
        ] {
            assert!(
                !session
                    .snapshot()
                    .available_actions
                    .iter()
                    .find(|action| action.id == action_id)
                    .expect("composition action readiness is projected")
                    .enabled,
                "{action_id} should require a selected composition root"
            );
        }
        let response = session.dispatch(request(
            Some(0),
            ClientAction::Composition(CompositionAction::ResolveDefaultRoot {
                composition_id: "default".to_owned(),
            }),
        ));

        assert_eq!(response.kind, ClientResponseKind::Applied);
        let composition = &response.snapshot.durable_state.compositions.compositions[0];
        assert_eq!(
            composition.root_package,
            Some(ContentReference {
                id: "fixture.pack".to_owned(),
                version: None,
            })
        );
        assert_eq!(composition.installation, InstallationState::Unknown);
        let launch = response
            .snapshot
            .available_actions
            .iter()
            .find(|action| action.id == "composition.launch")
            .expect("launch readiness is projected after choosing a composition");
        assert!(launch.enabled);
        assert_eq!(launch.reason, None);
        assert!(
            response
                .snapshot
                .available_actions
                .iter()
                .find(|action| action.id == "composition.install")
                .expect("install readiness is projected")
                .enabled
        );

        let install = session.dispatch(request(
            Some(response.revision.0),
            ClientAction::Composition(vapor_client_protocol::CompositionAction::Install {
                composition_id: "default".to_owned(),
            }),
        ));
        assert_eq!(install.kind, ClientResponseKind::OperationAccepted);
        let launch_during_install = install
            .snapshot
            .available_actions
            .iter()
            .find(|action| action.id == "composition.launch")
            .expect("launch readiness remains projected during install");
        assert!(!launch_during_install.enabled);
        assert!(launch_during_install.queueable);
        assert_eq!(
            launch_during_install.reason.as_deref(),
            Some("This composition already has an operation running.")
        );
        for action_id in [
            "composition.install",
            "composition.verify",
            "composition.repair",
            "composition.update",
            "composition.launch",
        ] {
            assert!(
                !install
                    .snapshot
                    .available_actions
                    .iter()
                    .find(|action| action.id == action_id)
                    .expect("composition action readiness remains projected during install")
                    .enabled,
                "{action_id} should be blocked by the active composition operation"
            );
        }
        let _ = wait_for_operation(&mut session);
        assert!(
            session
                .snapshot()
                .available_actions
                .iter()
                .find(|action| action.id == "composition.launch")
                .expect("launch readiness is restored after install")
                .enabled
        );

        let composition = &mut session.durable_state.compositions.compositions[0];
        let actual_graph = composition
            .resolved_graph
            .clone()
            .expect("fixture install resolves a graph");
        let artifacts = match &composition.installation {
            InstallationState::Installed { artifacts, .. } => artifacts.clone(),
            state => panic!("expected fixture installation, got {state:?}"),
        };
        composition.installation = InstallationState::OutOfDate {
            detail: "fixture graph drift".to_owned(),
            actual_graph,
            artifacts,
            verified: true,
        };
        let stale = session.snapshot();
        let launch = stale
            .available_actions
            .iter()
            .find(|action| action.id == "composition.launch")
            .expect("launch readiness is projected for stale installs");
        assert!(!launch.enabled);
        assert_eq!(
            launch.reason.as_deref(),
            Some("Review or update this composition before launching.")
        );
        let rejected = session.dispatch(request(
            Some(stale.revision.0),
            ClientAction::Composition(vapor_client_protocol::CompositionAction::Launch {
                composition_id: "default".to_owned(),
            }),
        ));
        assert_eq!(rejected.kind, ClientResponseKind::Rejected);
        assert_eq!(
            rejected.error.expect("outdated launch rejected").code,
            ServiceErrorCode::ActionUnavailable
        );
    }

    #[test]
    fn composition_launch_arguments_are_validated_and_composition_revision_is_projected() {
        let mut session = session();
        let update = session.dispatch(request(
            Some(0),
            ClientAction::Composition(CompositionAction::SetLaunchArguments {
                composition_id: "default".to_owned(),
                arguments: vec!["--windowed".to_owned()],
            }),
        ));
        assert_eq!(update.kind, ClientResponseKind::Applied);
        assert_eq!(update.snapshot.durable_state.composition_revision, 1);
        assert_eq!(
            update.snapshot.durable_state.compositions.compositions[0]
                .launch
                .arguments,
            ["--windowed"]
        );

        let rejected = session.dispatch(request(
            Some(1),
            ClientAction::Composition(CompositionAction::SetLaunchArguments {
                composition_id: "default".to_owned(),
                arguments: vec!["x".repeat(4097)],
            }),
        ));
        assert_eq!(rejected.kind, ClientResponseKind::Rejected);
        assert_eq!(rejected.snapshot.durable_state.composition_revision, 1);
        assert_eq!(
            rejected.snapshot.durable_state.compositions.compositions[0]
                .launch
                .arguments,
            ["--windowed"]
        );
    }

    #[test]
    fn content_invalidation_clears_stale_graph_and_installation_without_removing_root() {
        let mut session = session();
        let root = ContentReference {
            id: "fixture.game".to_owned(),
            version: Some("1.0.0".to_owned()),
        };
        let assigned = session.dispatch(request(
            Some(0),
            ClientAction::Composition(CompositionAction::SetRoot {
                composition_id: "default".to_owned(),
                content_id: Some(root.id.clone()),
                content_version: root.version.clone(),
                workspace_root: None,
            }),
        ));
        assert_eq!(assigned.kind, ClientResponseKind::Applied);

        let invalidated = session.dispatch(request(
            Some(1),
            ClientAction::Composition(CompositionAction::InvalidateContent {
                content_id: root.id.clone(),
                content_version: root.version.clone(),
            }),
        ));
        assert_eq!(invalidated.kind, ClientResponseKind::Applied);
        let composition = &invalidated.snapshot.durable_state.compositions.compositions[0];
        assert_eq!(composition.root_package, Some(root));
        assert_eq!(composition.resolved_graph, None);
        assert_eq!(composition.installation, InstallationState::Unknown);
    }
}
