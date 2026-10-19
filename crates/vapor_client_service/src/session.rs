use crate::{
    catalog::CatalogState,
    host::ClientHostContext,
    persistence::{ClientPersistentState, ClientStateStore},
};
mod authoring;
mod catalog;
mod composition;
mod library;
mod publishing;
mod snapshot;
mod workshop;
mod workspace;
#[cfg(test)]
use composition::fixture_composition_result;
use composition::{protocol_phase, protocol_status};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
};
use vapor_client::ClientHostConfig;
use vapor_client_application as application;
use vapor_client_protocol::{
    ApplicationReleaseSnapshot, AuthoringAction, AuthoringCompositionLink, AuthoringResult,
    AuthoringResultKind, AvailableSuperworkspaceRepositorySnapshot, CatalogAction, ClientAction,
    ClientEvent, ClientRequest, ClientResponse, ClientResponseKind, ClientRevision, Composition,
    CompositionAction, ContentReference, DeveloperAction, InstallationState, LegacyStorageSeed,
    LibraryAction, OperationEvent, OperationOutputEvent, OperationPhase, OperationStatus,
    OperationSummary, ProfileReportSnapshot, ProfileTraceSnapshot, PublishingAction,
    RootPublishingSnapshot, ServiceError, ServiceErrorCode, SessionAction,
    SourceMetadataRepairAction, WorkshopAcquisitionBatchSnapshot, WorkshopAcquisitionSnapshot,
    WorkshopAction, WorkshopItemSnapshot, WorkshopMetadataDraft, WorkshopPackagePreviewSnapshot,
    WorkshopReviewSnapshot, WorkshopSearchSnapshot, WorkshopSnapshot, WorkshopSubscriptionSnapshot,
    WorkspaceAction, WorkspaceAssociation, WorkspaceContentSnapshot, WorkspaceFileResult,
    WorkspaceProjectSelection, WorkspaceToolOperation,
};
#[cfg(test)]
use vapor_client_protocol::{CatalogEntry, CatalogItemRole, CatalogLocation, CatalogLocationKind};
use workspace::is_isolated_fixture_workspace;

const MAX_WORKSPACE_PROJECTS: usize = 10_000;
const MAX_WORKSPACE_DIAGNOSTICS: usize = 1_000;
const MAX_WORKSHOP_REVIEW_FILE_PATHS: usize = 50;
const MAX_BUFFERED_OPERATION_OUTPUT_EVENTS: usize = 512;
const OPERATION_OUTPUT_OMISSION_NOTICE: &str =
    "Older output omitted because the service buffer reached its limit.";

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
    active_operation: Option<OperationSummary>,
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
    operation_sender: Sender<BackgroundOperationMessage>,
}

enum BackgroundOperationMessage {
    Lifecycle(application::CoreOperationEvent),
    Complete(Box<CompositionOperationCompletion>),
    CompleteWorkshop(Box<WorkshopOperationCompletion>),
    CompleteRootPublish(Box<RootPublishOperationCompletion>),
    CompleteApplicationRelease(Box<ApplicationReleaseOperationCompletion>),
    CompleteWorkspace(Box<WorkspaceOperationCompletion>),
    CompleteAuthoring(Box<AuthoringOperationCompletion>),
    CompleteLibrary(Box<LibraryOperationCompletion>),
    CompleteProfileAnalysis(Box<ProfileAnalysisCompletion>),
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

struct ProfileAnalysisCompletion {
    id: String,
    result: Result<ProfileReportSnapshot, String>,
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

struct AutoInstallCompositionUpdate {
    composition_id: String,
    inputs: CompositionOperationInputs,
    result: Result<vapor_client_application::InstalledComposition, String>,
}

#[derive(Debug, Clone, Default)]
struct AutoInstallContext {
    constraints: BTreeMap<String, Vec<String>>,
    compositions: BTreeMap<String, Vec<AutoInstallCompositionTarget>>,
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
    RunTool(
        WorkspaceToolOperation,
        WorkspaceProjectSelection,
        Vec<String>,
    ),
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
        acquisition: Result<
            (
                Vec<vapor_client_application::WorkshopAcquisition>,
                usize,
                Vec<AutoInstallCompositionUpdate>,
            ),
            String,
        >,
    },
    Acquisitions {
        items: Vec<vapor_client_application::WorkshopAcquisition>,
        requested: usize,
        updates: Vec<AutoInstallCompositionUpdate>,
    },
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
        if let Some(workspace_root) = bootstrap.workspace_root.as_ref() {
            let workspace = Some(workspace_root.display().to_string());
            if loaded.state.authoring_workspace_root != workspace {
                loaded.state.authoring_workspace_root = workspace;
                store.save(&loaded.state)?;
            }
        }
        if bootstrap.workspace_root.is_none() {
            bootstrap.workspace_root = loaded
                .state
                .authoring_workspace_root
                .as_deref()
                .map(PathBuf::from);
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
        let (operation_sender, operation_receiver) = mpsc::channel();
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
            active_operation: None,
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
        self.active_operation.is_some()
    }

    pub(super) fn reject_if_operation_in_progress(&self) -> Result<(), ServiceError> {
        if self.active_operation.is_some() {
            return Err(ServiceError {
                code: ServiceErrorCode::OperationInProgress,
                message: "Wait for the current operation to finish.".to_owned(),
                retryable: true,
            });
        }
        Ok(())
    }

    pub fn dispatch(&mut self, request: ClientRequest) -> ClientResponse {
        self.poll_background_operation_messages();
        let mut response_kind = ClientResponseKind::Applied;
        let mut error = None;
        if request
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
        } else {
            match request.action {
                ClientAction::Catalog(CatalogAction::Refresh) => {
                    self.refresh_catalog();
                    self.revision.0 = self.revision.0.saturating_add(1);
                    if let Some(message) = self.catalog.status().error.clone() {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(ServiceError {
                            code: ServiceErrorCode::ProviderFailure,
                            message,
                            retryable: true,
                        });
                    }
                }
                ClientAction::Catalog(CatalogAction::DiscoverContentProjects)
                    if self.bootstrap.workspace_root.is_some() =>
                {
                    self.refresh_workspace_content();
                    self.revision.0 = self.revision.0.saturating_add(1);
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
                    | WorkspaceAction::RunTool { .. }
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
                ClientAction::Authoring(action) => match self.start_authoring_operation(action) {
                    Ok(()) => {
                        response_kind = ClientResponseKind::OperationAccepted;
                        self.revision.0 = self.revision.0.saturating_add(1);
                    }
                    Err(operation_error) => {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(operation_error);
                    }
                },
                ClientAction::Library(LibraryAction::SetAutoInstall {
                    content_id,
                    enabled,
                }) => {
                    if self.active_operation.is_some() {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(ServiceError {
                            code: ServiceErrorCode::OperationInProgress,
                            message: "Wait for the current operation to finish before changing Library update policy.".to_owned(),
                            retryable: true,
                        });
                        let snapshot = self.snapshot();
                        return ClientResponse {
                            protocol_version: vapor_client_protocol::PROTOCOL_VERSION,
                            request_id: request.request_id,
                            revision: snapshot.revision,
                            kind: response_kind,
                            snapshot,
                            error,
                        };
                    }
                    let parsed =
                        vapor_client_protocol::ContentReference::parse(content_id.clone(), None)
                            .and_then(|reference| {
                                reference.vapor_id().map_err(|error| error.to_string())
                            });
                    match parsed {
                        Err(message) => {
                            response_kind = ClientResponseKind::Rejected;
                            error = Some(ServiceError {
                                code: ServiceErrorCode::InvalidRequest,
                                message: format!("invalid content ID `{content_id}`: {message}"),
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
                    } else if let Err(failure) = self.reject_if_operation_in_progress() {
                        response_kind = ClientResponseKind::Rejected;
                        error = Some(failure);
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
        ClientResponse {
            protocol_version: vapor_client_protocol::PROTOCOL_VERSION,
            request_id: request.request_id,
            revision: snapshot.revision,
            kind: response_kind,
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
        self.active_operation = Some(OperationSummary {
            id: id.clone(),
            composition_id: spec.composition_id,
            action: spec.action.to_owned(),
            phase: spec.phase,
            status: OperationStatus::Running,
            progress: None,
        });

        let request = application::CoreOperationRequest {
            id: operation_id,
            domain: spec.domain.to_owned(),
            action: spec.action.to_owned(),
        };
        let lifecycle_sender = self.operation_sender.clone();
        let completion_sender = self.operation_sender.clone();
        let failure_label = spec.failure_label;
        thread::Builder::new()
            .name(format!("{}-{id}", spec.thread_prefix))
            .spawn(move || {
                let sink: application::OperationEventSink = std::sync::Arc::new(move |event| {
                    // The service emits terminal events only after it has applied
                    // and persisted the operation's user-visible state.
                    if matches!(event.status, application::OperationStatus::Running) {
                        let _ = lifecycle_sender.send(BackgroundOperationMessage::Lifecycle(event));
                    }
                });
                let result =
                    application::OperationService::run_observed(request, sink, execute, outcome);
                let _ = completion_sender.send(completion(id, result));
            })
            .map_err(|error| {
                self.active_operation = None;
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
            match message {
                BackgroundOperationMessage::Lifecycle(event) => {
                    let id = event.id.0.to_string();
                    let Some(active) = self.active_operation.as_mut().filter(|op| op.id == id)
                    else {
                        continue;
                    };
                    active.phase = protocol_phase(event.phase);
                    active.progress = event.progress.as_ref().and_then(|progress| {
                        progress
                            .total
                            .filter(|total| *total > 0)
                            .map(|total| (progress.completed as f32 / total as f32).clamp(0.0, 1.0))
                    });
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
                    let (status, message) = protocol_status(event.status);
                    let message = message.or_else(|| {
                        event
                            .progress
                            .as_ref()
                            .and_then(|progress| progress.message.clone())
                    });
                    active.status = status.clone();
                    self.revision.0 = self.revision.0.saturating_add(1);
                    self.operation_events
                        .push_back(ClientEvent::OperationChanged(OperationEvent {
                            revision: self.revision,
                            id,
                            composition_id: active.composition_id.clone(),
                            action: active.action.clone(),
                            phase: active.phase,
                            status,
                            progress: active.progress,
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
                BackgroundOperationMessage::CompleteProfileAnalysis(completion) => {
                    self.apply_profile_analysis_completion(*completion);
                }
            }
        }
    }

    fn apply_profile_analysis_completion(&mut self, completion: ProfileAnalysisCompletion) {
        let Some(active) = self
            .active_operation
            .as_ref()
            .filter(|operation| operation.id == completion.id)
        else {
            return;
        };
        let action = active.action.clone();
        self.active_operation = None;
        self.revision.0 = self.revision.0.saturating_add(1);
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

    fn collect_unreferenced_auto_install(&mut self) -> Result<Vec<String>, String> {
        if self.bootstrap.fixture_mode {
            return Ok(Vec::new());
        }
        if self
            .durable_state
            .compositions
            .compositions
            .iter()
            .any(|composition| {
                composition.root_package.is_some() && composition.resolved_graph.is_none()
            })
        {
            return Ok(Vec::new());
        }
        let policies = self.durable_state.library_policies.clone();
        let mut collected = Vec::new();
        for policy in policies.into_iter().filter(|policy| policy.auto_install) {
            let referenced = self
                .durable_state
                .compositions
                .compositions
                .iter()
                .filter_map(|composition| composition.resolved_graph.as_ref())
                .any(|graph| {
                    graph
                        .nodes
                        .iter()
                        .any(|node| node.content.id == policy.content_id.as_str())
                });
            if referenced {
                continue;
            }
            if application::collect_library_item(
                &self.bootstrap.application_executable,
                policy.content_id.as_str(),
            )? {
                collected.push(policy.content_id.to_string());
            }
        }
        if !collected.is_empty() {
            self.refresh_catalog();
        }
        Ok(collected)
    }
}

fn ensure_workshop_app_id(bootstrap: &mut ClientHostConfig) {
    if !bootstrap.fixture_mode && bootstrap.workshop_app_id.is_none() {
        bootstrap.workshop_app_id = Some(vapor_sdk_core::root::types::ROOT_STEAM_APP_ID);
    }
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
        assert!(session.active_operation.is_none());
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
        assert!(session.active_operation.is_none());
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
        session.catalog.replace(vec![entry.clone()]);

        assert!(session.validate_auto_install_target(content_id).is_ok());

        session.durable_state.compositions.compositions[0].resolved_graph = None;
        assert!(
            session
                .validate_auto_install_target(content_id)
                .is_err_and(|message| message.contains("Add this item to a composition"))
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
        session.catalog.replace(vec![entry]);
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
            if session.active_operation.is_none() {
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
        assert!(response.snapshot.workspace_content.source_projects_loaded);
        assert!(
            response
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
        let context = session
            .auto_install_context(&kind)
            .expect("safe update context can be constructed");
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
        assert!(
            session
                .auto_install_context(&kind)
                .unwrap_err()
                .contains("has not been resolved")
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
        assert_eq!(
            snapshot["workshop"]["search"]["items"][0]["published_file_id"],
            "1002"
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
        assert!(session.active_operation.is_none());
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
        session.active_operation = Some(OperationSummary {
            id: "operation-install-first".into(),
            composition_id: Some("default".into()),
            action: "composition.install".into(),
            phase: OperationPhase::Installing,
            status: OperationStatus::Running,
            progress: None,
        });

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
        assert!(session.active_operation.is_none());
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
        let composition = &session.durable_state.compositions.compositions[0];
        session.active_operation = Some(OperationSummary {
            id: "operation-verify".to_owned(),
            composition_id: Some("default".to_owned()),
            action: "composition.verify".to_owned(),
            phase: OperationPhase::Verifying,
            status: OperationStatus::Running,
            progress: None,
        });
        session.apply_composition_completion(CompositionOperationCompletion {
            id: "operation-verify".to_owned(),
            composition_id: "default".to_owned(),
            action: CompositionOperationKind::Verify,
            inputs: CompositionOperationInputs::capture(
                composition,
                CompositionOperationKind::Verify,
            ),
            result: CompositionOperationResult::Verify(Ok(
                vapor_client_application::InstallationCheck {
                    ok: false,
                    detail: vec!["packagepack payload is missing".to_owned()],
                    artifacts: Vec::new(),
                    graph: None,
                },
            )),
        });

        assert!(session.active_operation.is_none());
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
        session.active_operation = Some(OperationSummary {
            id: "profile-analysis-1".to_owned(),
            composition_id: None,
            action: "analyze-trace".to_owned(),
            phase: OperationPhase::Finalizing,
            status: OperationStatus::Running,
            progress: None,
        });
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
        assert_eq!(
            launch_during_install.reason.as_deref(),
            Some("Wait for the current operation to finish.")
        );
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
