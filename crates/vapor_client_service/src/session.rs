use crate::{
    catalog::CatalogState,
    host::ClientHostContext,
    persistence::{ClientPersistentState, ClientStateStore},
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
};
use vapor_client::ClientHostConfig;
use vapor_client_application as application;
use vapor_client_application::{
    CatalogContent, SourceContent, discover_content_catalog, discover_source_content,
    scan_unregistered_content_projects,
};
use vapor_client_protocol::{
    AuthoringAction, AuthoringCompositionLink, AuthoringResult, AuthoringResultKind,
    AvailableAction, AvailableSuperworkspaceRepositorySnapshot, CatalogAction, CatalogConflict,
    CatalogEntry, CatalogItemAction, CatalogItemRole, CatalogLocation, CatalogLocationKind,
    ClientAction, ClientEvent, ClientRequest, ClientResponse, ClientResponseKind, ClientRevision,
    ClientSnapshot, Composition, CompositionAction, ContentReference, DurableStateSnapshot,
    HostSummary, InstallationState, LegacyStorageSeed, LibraryAction, OperationEvent,
    OperationPhase, OperationStatus, OperationSummary, PublishingAction, RootPublishingSnapshot,
    ServiceError, ServiceErrorCode, SessionAction, SuperworkspaceRepositorySnapshot,
    SuperworkspaceSnapshot, WorkshopAcquisitionBatchSnapshot, WorkshopAcquisitionSnapshot,
    WorkshopAction, WorkshopItemSnapshot, WorkshopMetadataDraft, WorkshopPackagePreviewSnapshot,
    WorkshopReviewSnapshot, WorkshopSearchSnapshot, WorkshopSnapshot, WorkshopSubscriptionSnapshot,
    WorkspaceAction, WorkspaceAssociation, WorkspaceContentSnapshot, WorkspaceFileResult,
    WorkspaceProjectCandidate, WorkspaceSourceProject, WorkspaceWorkshopPolicy,
};

const MAX_WORKSPACE_PROJECTS: usize = 10_000;
const MAX_WORKSPACE_DIAGNOSTICS: usize = 1_000;
const MAX_WORKSHOP_REVIEW_FILE_PATHS: usize = 50;

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
    root_publish_review: Option<RootPublishReviewState>,
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
    CompleteWorkspace(Box<WorkspaceOperationCompletion>),
    CompleteAuthoring(Box<AuthoringOperationCompletion>),
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
    workspace_root: PathBuf,
    preview: vapor_client_application::WorkshopPackagePreview,
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
        workspace_root: PathBuf,
    },
    Publish {
        review: Box<WorkshopReviewState>,
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
    Publication(vapor_client_application::WorkshopPublication),
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
            root_publish_review: None,
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

    pub fn snapshot(&self) -> ClientSnapshot {
        let mut available_actions = vec![AvailableAction {
            id: "catalog.refresh".to_owned(),
            enabled: true,
            reason: None,
        }];
        available_actions.extend([
            AvailableAction {
                id: "workspace.initialize-superworkspace".to_owned(),
                enabled: !self.bootstrap.fixture_mode,
                reason: self
                    .bootstrap
                    .fixture_mode
                    .then(|| "Unavailable in fixture mode.".to_owned()),
            },
            AvailableAction {
                id: "workspace.set-superworkspace-location".to_owned(),
                enabled: !self.bootstrap.fixture_mode,
                reason: self
                    .bootstrap
                    .fixture_mode
                    .then(|| "Unavailable in fixture mode.".to_owned()),
            },
            AvailableAction {
                id: "workspace.discover-superworkspace-repositories".to_owned(),
                enabled: !self.bootstrap.fixture_mode && self.active_operation.is_none(),
                reason: if self.bootstrap.fixture_mode {
                    Some("Unavailable in fixture mode.".to_owned())
                } else if self.active_operation.is_some() {
                    Some("Wait for the current operation to finish.".to_owned())
                } else {
                    None
                },
            },
            AvailableAction {
                id: "workspace.acquire-superworkspace-repositories".to_owned(),
                enabled: !self.bootstrap.fixture_mode && self.active_operation.is_none(),
                reason: if self.bootstrap.fixture_mode {
                    Some("Unavailable in fixture mode.".to_owned())
                } else if self.active_operation.is_some() {
                    Some("Wait for the current operation to finish.".to_owned())
                } else {
                    None
                },
            },
            AvailableAction {
                id: "catalog.discover-content-projects".to_owned(),
                enabled: self.bootstrap.workspace_root.is_some(),
                reason: self
                    .bootstrap
                    .workspace_root
                    .is_none()
                    .then(|| "Open a workspace first.".to_owned()),
            },
            AvailableAction {
                id: "composition.select".to_owned(),
                enabled: true,
                reason: None,
            },
            AvailableAction {
                id: "catalog.search".to_owned(),
                enabled: true,
                reason: None,
            },
            AvailableAction {
                id: "catalog.inspect".to_owned(),
                enabled: !self.catalog.entries().is_empty(),
                reason: self
                    .catalog
                    .entries()
                    .is_empty()
                    .then(|| "Refresh the catalog first.".to_owned()),
            },
            AvailableAction {
                id: "workspace.open".to_owned(),
                enabled: true,
                reason: None,
            },
        ]);
        let active_composition =
            self.durable_state
                .compositions
                .compositions
                .iter()
                .find(|composition| {
                    self.durable_state.active_composition_id.as_str() == composition.id.as_str()
                });
        let composition_has_root =
            active_composition.is_some_and(|composition| composition.root_package.is_some());
        let composition_is_out_of_date = active_composition.is_some_and(|composition| {
            matches!(
                &composition.installation,
                InstallationState::OutOfDate { .. }
            )
        });
        let has_workspace_source = active_composition.is_some_and(|composition| {
            composition.root_package.is_some() && composition.workspace.is_some()
        });
        let operation_is_idle = self.active_operation.is_none();
        let launch_reason = if !operation_is_idle {
            Some("Wait for the current operation to finish.".to_owned())
        } else if !composition_has_root {
            Some("Choose a playable composition first.".to_owned())
        } else if composition_is_out_of_date {
            Some("Review or update this composition before launching.".to_owned())
        } else {
            None
        };
        let build_reason = if !operation_is_idle {
            Some("Wait for the current operation to finish.".to_owned())
        } else if !has_workspace_source {
            Some("Open a project-backed composition before building and launching.".to_owned())
        } else {
            None
        };
        available_actions.extend([
            AvailableAction {
                id: "composition.launch".to_owned(),
                enabled: launch_reason.is_none(),
                reason: launch_reason,
            },
            AvailableAction {
                id: "composition.build-and-launch".to_owned(),
                enabled: build_reason.is_none(),
                reason: build_reason,
            },
        ]);
        let mut workshop = self.workshop.clone();
        workshop.review = self.workshop_review.as_ref().map(workshop_review_snapshot);
        ClientSnapshot {
            revision: self.revision,
            host: HostSummary {
                app_root: self
                    .host_context
                    .as_ref()
                    .and_then(|context| context.app_root.as_ref())
                    .map(|root| root.display().to_string())
                    .or_else(|| {
                        self.bootstrap
                            .application_executable
                            .parent()
                            .and_then(|path| path.parent())
                            .and_then(|path| path.parent())
                            .map(|root| root.display().to_string())
                    }),
                workspace: self
                    .bootstrap
                    .workspace_root
                    .as_ref()
                    .map(|path| path.display().to_string()),
                mode: if self.bootstrap.fixture_mode {
                    "fixture".to_owned()
                } else {
                    "production".to_owned()
                },
                platform: self.host_context.as_ref().map_or_else(
                    || std::env::consts::OS.to_owned(),
                    |context| format!("{:?}", context.platform).to_lowercase(),
                ),
                steam_client_available: self
                    .host_context
                    .as_ref()
                    .is_some_and(|context| context.steam_client_available),
            },
            active_composition_id: self.active_composition_id.clone(),
            durable_state: DurableStateSnapshot {
                schema_version: self.durable_state.schema_version,
                composition_revision: self.durable_state.composition_revision,
                compositions: self.durable_state.compositions.clone(),
                library_policies: self.durable_state.library_policies.clone(),
                active_composition_id: self.durable_state.active_composition_id.clone(),
                authoring_workspace_root: self.durable_state.authoring_workspace_root.clone(),
                editor_drafts: self.durable_state.editor_drafts.clone(),
                operation_history: self.durable_state.operation_history.clone(),
                recovery_backup: self.durable_state.recovery_backup.clone(),
            },
            active_operation: self.active_operation.clone(),
            composition_preview: self.composition_preview.clone(),
            catalog: self
                .catalog
                .entries()
                .iter()
                .map(|entry| catalog_projection(entry, &self.durable_state, self.catalog.entries()))
                .collect(),
            catalog_status: self.catalog.status().clone(),
            catalog_search: self.catalog.search(),
            workspace_content: self.workspace_content.clone(),
            superworkspace: self.superworkspace_snapshot(),
            workshop,
            root_publishing: self
                .root_publish_review
                .as_ref()
                .map(|review| review.snapshot.clone()),
            selected_content_id: self.selected_content_id.clone(),
            available_actions,
        }
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
                        self.revision.0 = self.revision.0.saturating_add(1);
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

    fn validate_workshop_stage(&mut self, selector: &str) -> Result<PathBuf, ServiceError> {
        let Some(workspace_root) = self.bootstrap.workspace_root.clone() else {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "Open the authored content workspace before staging a Workshop item."
                    .to_owned(),
                retryable: false,
            });
        };
        self.refresh_catalog();
        if let Some(error) = self.catalog.status().error.as_ref() {
            return Err(ServiceError {
                code: ServiceErrorCode::ProviderFailure,
                message: format!(
                    "Vapor Client could not refresh on-device content before staging: {error}"
                ),
                retryable: true,
            });
        }
        if self.catalog.status().omitted > 0 {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "The on-device catalog is incomplete; content conflicts cannot be confirmed before staging.".to_owned(),
                retryable: false,
            });
        }
        if !self.workspace_content.source_projects_loaded
            || self.workspace_content.source_projects_error.is_some()
        {
            return Err(ServiceError {
                code: ServiceErrorCode::ProviderFailure,
                message: self
                    .workspace_content
                    .source_projects_error
                    .clone()
                    .unwrap_or_else(|| {
                        "Vapor Client could not refresh the authored project list.".into()
                    }),
                retryable: true,
            });
        }
        let (id, version) = selector
            .split_once('@')
            .map(|(id, version)| (id, Some(version)))
            .unwrap_or((selector, None));
        if id.is_empty()
            || version.is_some_and(str::is_empty)
            || !self
                .workspace_content
                .source_projects
                .iter()
                .any(|project| project.id == id && project.version.as_deref() == version)
        {
            return Err(invalid_workshop_request(
                "Select an exact content identity from the active workspace before staging it.",
            ));
        }
        if self.catalog.entries().iter().any(|entry| {
            entry.id == id && entry.version.as_deref() == version && !entry.conflicts.is_empty()
        }) {
            return Err(invalid_workshop_request(
                "On-device copies of this content version disagree. Resolve the conflict before staging it.",
            ));
        }
        Ok(workspace_root)
    }

    fn start_composition_operation(
        &mut self,
        action: vapor_client_protocol::CompositionAction,
    ) -> Result<(), ServiceError> {
        if self.active_operation.is_some() {
            return Err(ServiceError {
                code: ServiceErrorCode::OperationInProgress,
                message: "Wait for the current operation to finish.".to_owned(),
                retryable: true,
            });
        }
        use vapor_client_protocol::CompositionAction as Action;
        let (composition_id, kind, selector_override) = match action {
            Action::Preview {
                composition_id,
                selector,
            } => (composition_id, CompositionOperationKind::Preview, selector),
            Action::Install { composition_id } => {
                (composition_id, CompositionOperationKind::Install, None)
            }
            Action::Verify { composition_id } => {
                (composition_id, CompositionOperationKind::Verify, None)
            }
            Action::Repair { composition_id } => {
                (composition_id, CompositionOperationKind::Repair, None)
            }
            Action::Update { composition_id } => {
                (composition_id, CompositionOperationKind::Update, None)
            }
            Action::Launch { composition_id } => {
                (composition_id, CompositionOperationKind::Launch, None)
            }
            Action::RunSource { composition_id } => {
                (composition_id, CompositionOperationKind::RunSource, None)
            }
            _ => {
                return Err(ServiceError {
                    code: ServiceErrorCode::InvalidRequest,
                    message: "This action changes composition settings and cannot start a runtime operation.".to_owned(),
                    retryable: false,
                });
            }
        };
        let composition = self
            .durable_state
            .compositions
            .compositions
            .iter()
            .find(|composition| composition.id == composition_id)
            .cloned()
            .ok_or_else(|| ServiceError {
                code: ServiceErrorCode::InvalidRequest,
                message: "The selected composition no longer exists.".to_owned(),
                retryable: true,
            })?;
        if composition.root_package.is_none() && selector_override.is_none() {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "Choose a packagepack for this composition first.".to_owned(),
                retryable: false,
            });
        }
        if matches!(kind, CompositionOperationKind::Launch)
            && matches!(
                &composition.installation,
                InstallationState::OutOfDate { .. }
            )
        {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "Review or update this composition before launching.".to_owned(),
                retryable: false,
            });
        }
        if selector_override.is_some() && !matches!(kind, CompositionOperationKind::Preview) {
            return Err(ServiceError {
                code: ServiceErrorCode::InvalidRequest,
                message: "Only preview accepts an explicit content selector.".to_owned(),
                retryable: false,
            });
        }
        if matches!(kind, CompositionOperationKind::RunSource) && composition.workspace.is_none() {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "Open the composition's workspace before running its source composition."
                    .to_owned(),
                retryable: false,
            });
        }
        self.composition_preview = None;
        let operation_id = application::OperationId::fresh();
        let id = operation_id.0.to_string();
        let action_id = match kind {
            CompositionOperationKind::Preview => "composition.preview",
            CompositionOperationKind::Install => "composition.install",
            CompositionOperationKind::Verify => "composition.verify",
            CompositionOperationKind::Repair => "composition.repair",
            CompositionOperationKind::Update => "composition.update",
            CompositionOperationKind::Launch => "composition.launch",
            CompositionOperationKind::RunSource => "composition.run-source",
        };
        self.active_operation = Some(OperationSummary {
            id: id.clone(),
            composition_id: Some(composition_id.clone()),
            action: action_id.to_owned(),
            phase: initial_protocol_phase(kind),
            status: OperationStatus::Running,
            progress: None,
        });

        let bootstrap = self.bootstrap.clone();
        let operation_selector = selector_override.clone();
        let operation_inputs = CompositionOperationInputs::capture(&composition, kind);
        let lifecycle_sender = self.operation_sender.clone();
        let completion_sender = self.operation_sender.clone();
        thread::Builder::new()
            .name(format!("studio-composition-{id}"))
            .spawn(move || {
                let request = application::CoreOperationRequest {
                    id: operation_id,
                    domain: "studio-composition".to_owned(),
                    action: action_id.to_owned(),
                };
                let sink: application::OperationEventSink = std::sync::Arc::new(move |event| {
                    // The operation runner's terminal event describes the core
                    // call only. The Vapor Client service still has to apply and
                    // persist the resulting composition state before it can report
                    // that the user-visible operation succeeded.
                    if matches!(event.status, application::OperationStatus::Running) {
                        let _ = lifecycle_sender.send(BackgroundOperationMessage::Lifecycle(event));
                    }
                });
                let result = application::OperationService::run_observed(
                    request,
                    sink,
                    |context| {
                        context.set_phase(core_operation_phase(kind));
                        execute_composition_operation(
                            &bootstrap,
                            &composition,
                            kind,
                            operation_selector.as_deref(),
                        )
                    },
                    composition_result_outcome,
                );
                let _ = completion_sender.send(BackgroundOperationMessage::Complete(Box::new(
                    CompositionOperationCompletion {
                        id,
                        composition_id,
                        action: kind,
                        inputs: operation_inputs,
                        result,
                    },
                )));
            })
            .map_err(|error| {
                self.active_operation = None;
                ServiceError {
                    code: ServiceErrorCode::InternalFailure,
                    message: format!("Could not start composition operation: {error}"),
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
                    let (status, message) = protocol_status(event.status);
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
                BackgroundOperationMessage::CompleteWorkspace(completion) => {
                    self.apply_workspace_completion(*completion);
                }
                BackgroundOperationMessage::CompleteAuthoring(completion) => {
                    self.apply_authoring_completion(*completion);
                }
            }
        }
    }

    fn start_workspace_operation(&mut self, action: WorkspaceAction) -> Result<(), ServiceError> {
        if self.active_operation.is_some() {
            return Err(ServiceError {
                code: ServiceErrorCode::OperationInProgress,
                message: "Wait for the current operation to finish.".to_owned(),
                retryable: true,
            });
        }
        let is_superworkspace_operation = matches!(
            &action,
            WorkspaceAction::DiscoverSuperworkspaceRepositories
                | WorkspaceAction::AcquireSuperworkspaceRepositories { .. }
        );
        if is_superworkspace_operation && self.bootstrap.fixture_mode {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message:
                    "Fixture sessions cannot access the real source registry or Superworkspace."
                        .to_owned(),
                retryable: false,
            });
        }
        let root = if is_superworkspace_operation {
            configured_superworkspace_root().map_err(|message| ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message,
                retryable: true,
            })?
        } else {
            self.bootstrap
                .workspace_root
                .clone()
                .ok_or_else(|| ServiceError {
                    code: ServiceErrorCode::ActionUnavailable,
                    message: "Open a workspace before using its file editor.".to_owned(),
                    retryable: false,
                })?
        };
        if self.bootstrap.fixture_mode && !is_isolated_fixture_workspace(&root) {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "Fixture mode can edit only temporary test workspaces; it cannot access a player workspace.".to_owned(),
                retryable: false,
            });
        }
        let kind = match action {
            WorkspaceAction::DiscoverSuperworkspaceRepositories => {
                WorkspaceOperationKind::DiscoverSuperworkspaceRepositories
            }
            WorkspaceAction::AcquireSuperworkspaceRepositories { repository_names } => {
                if repository_names.is_empty()
                    || repository_names.len() > 64
                    || repository_names
                        .iter()
                        .any(|name| name.trim().is_empty() || name.len() > 128)
                {
                    return Err(invalid_workspace_request(
                        "Choose between one and 64 valid source repositories.",
                    ));
                }
                WorkspaceOperationKind::AcquireSuperworkspaceRepositories(repository_names)
            }
            WorkspaceAction::ListFiles { path } => {
                if !path.is_empty() && path != "." {
                    return Err(invalid_workspace_request(
                        "Workspace file discovery currently starts at the workspace root.",
                    ));
                }
                WorkspaceOperationKind::ListFiles
            }
            WorkspaceAction::OpenFile { path } => {
                WorkspaceOperationKind::OpenFile(validate_workspace_relative_path(&path)?)
            }
            WorkspaceAction::SaveFile {
                path,
                expected_fingerprint,
                contents,
            } => {
                if contents.len() > 4 * 1024 * 1024 {
                    return Err(invalid_workspace_request(
                        "Workspace editor saves are limited to 4 MiB per file.",
                    ));
                }
                WorkspaceOperationKind::SaveFile(Box::new(WorkspaceFileSave {
                    path: validate_workspace_relative_path(&path)?,
                    expected_fingerprint,
                    contents,
                }))
            }
            WorkspaceAction::CreateFile { path, contents } => {
                if !contents.is_empty() {
                    return Err(invalid_workspace_request(
                        "New workspace files are created empty; edit and save them after creation.",
                    ));
                }
                WorkspaceOperationKind::CreateFile(validate_workspace_relative_path(&path)?)
            }
            WorkspaceAction::Validate => WorkspaceOperationKind::Validate,
            WorkspaceAction::Remember => WorkspaceOperationKind::Remember,
            WorkspaceAction::AdoptProject { path } => {
                let candidate = PathBuf::from(path);
                let candidate = if candidate.is_absolute() {
                    candidate
                } else {
                    root.join(candidate)
                };
                let canonical = candidate.canonicalize().map_err(|error| {
                    invalid_workspace_request(&format!("{}: {error}", candidate.display()))
                })?;
                let canonical_root = root.canonicalize().map_err(|error| {
                    invalid_workspace_request(&format!("{}: {error}", root.display()))
                })?;
                if !canonical.starts_with(&canonical_root) || !canonical.is_dir() {
                    return Err(invalid_workspace_request(
                        "Choose a content project folder inside the active workspace.",
                    ));
                }
                WorkspaceOperationKind::AdoptProject(canonical)
            }
            WorkspaceAction::UnregisterProject { project_id } => {
                if project_id.trim().is_empty() || project_id.len() > 256 {
                    return Err(invalid_workspace_request(
                        "Choose a valid content project ID.",
                    ));
                }
                WorkspaceOperationKind::UnregisterProject(project_id)
            }
            _ => {
                return Err(ServiceError {
                    code: ServiceErrorCode::ActionUnavailable,
                    message: "This workspace action is not a file editor operation.".to_owned(),
                    retryable: false,
                });
            }
        };
        if self.bootstrap.fixture_mode
            && !matches!(
                &kind,
                WorkspaceOperationKind::ListFiles
                    | WorkspaceOperationKind::OpenFile(_)
                    | WorkspaceOperationKind::SaveFile(_)
                    | WorkspaceOperationKind::CreateFile(_)
            )
        {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "This operation needs the real Vapor installation and is unavailable in fixture mode.".to_owned(),
                retryable: false,
            });
        }
        let operation_id = application::OperationId::fresh();
        let id = operation_id.0.to_string();
        let action_id = workspace_operation_action_id(&kind);
        self.active_operation = Some(OperationSummary {
            id: id.clone(),
            composition_id: None,
            action: action_id.to_owned(),
            phase: workspace_operation_phase(&kind),
            status: OperationStatus::Running,
            progress: None,
        });

        let action_for_worker = kind.clone();
        let root_for_worker = root.clone();
        let executable_for_worker = self.bootstrap.application_executable.clone();
        let fixture_mode = self.bootstrap.fixture_mode;
        let lifecycle_sender = self.operation_sender.clone();
        let completion_sender = self.operation_sender.clone();
        thread::Builder::new()
            .name(format!("studio-workspace-{id}"))
            .spawn(move || {
                let request = application::CoreOperationRequest {
                    id: operation_id,
                    domain: "studio-workspace".to_owned(),
                    action: action_id.to_owned(),
                };
                let sink: application::OperationEventSink = std::sync::Arc::new(move |event| {
                    if matches!(event.status, application::OperationStatus::Running) {
                        let _ = lifecycle_sender.send(BackgroundOperationMessage::Lifecycle(event));
                    }
                });
                let result = application::OperationService::run_observed(
                    request,
                    sink,
                    |_| {
                        execute_workspace_operation(
                            &executable_for_worker,
                            &root_for_worker,
                            action_for_worker.clone(),
                            fixture_mode,
                        )
                    },
                    workspace_result_outcome,
                );
                let _ = completion_sender.send(BackgroundOperationMessage::CompleteWorkspace(
                    Box::new(WorkspaceOperationCompletion {
                        id,
                        action: kind,
                        root,
                        result,
                    }),
                ));
            })
            .map_err(|error| {
                self.active_operation = None;
                ServiceError {
                    code: ServiceErrorCode::InternalFailure,
                    message: format!("Could not start workspace file operation: {error}"),
                    retryable: true,
                }
            })?;
        Ok(())
    }

    fn apply_workspace_completion(&mut self, completion: WorkspaceOperationCompletion) {
        let source_context_is_current = if matches!(
            &completion.action,
            WorkspaceOperationKind::DiscoverSuperworkspaceRepositories
                | WorkspaceOperationKind::AcquireSuperworkspaceRepositories(_)
        ) {
            configured_superworkspace_root().ok().as_deref() == Some(completion.root.as_path())
        } else {
            self.bootstrap.workspace_root.as_deref() == Some(&completion.root)
        };
        let mut result = completion.result;
        if !source_context_is_current {
            result =
                Err("The selected source folder changed while the operation was running.".into());
        }
        let mut workspace_file_result = None;
        let mut operation_succeeded = false;
        let message = match result {
            Ok(WorkspaceOperationResult::SourceRepositories(repositories)) => {
                operation_succeeded = true;
                self.superworkspace_sources = repositories;
                self.superworkspace_sources_loaded = true;
                self.superworkspace_sources_error = None;
                format!(
                    "Found {} source repositories in the Vapor registry.",
                    self.superworkspace_sources.len()
                )
            }
            Ok(WorkspaceOperationResult::AcquiredSourceRepositories(repository_ids)) => {
                operation_succeeded = true;
                format!(
                    "Added {} source repositories to the Vapor source folder.",
                    repository_ids.len()
                )
            }
            Ok(WorkspaceOperationResult::Listed(files)) => {
                operation_succeeded = true;
                let files = files
                    .into_iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                let message = format!("Found {} workspace files.", files.len());
                workspace_file_result = Some(WorkspaceFileResult::Listed {
                    operation_id: completion.id.clone(),
                    files,
                });
                message
            }
            Ok(WorkspaceOperationResult::Opened(opened)) => {
                operation_succeeded = true;
                let path = opened.path.to_string_lossy().into_owned();
                workspace_file_result = Some(WorkspaceFileResult::Opened {
                    operation_id: completion.id.clone(),
                    path: path.clone(),
                    contents: opened.contents,
                    fingerprint: opened.fingerprint,
                });
                format!("Opened {path}.")
            }
            Ok(WorkspaceOperationResult::Saved { path, fingerprint }) => {
                operation_succeeded = true;
                let path = path.to_string_lossy().into_owned();
                self.workshop_review = None;
                if is_content_manifest_path(Path::new(&path)) {
                    self.refresh_catalog();
                }
                workspace_file_result = Some(WorkspaceFileResult::Saved {
                    operation_id: completion.id.clone(),
                    path: path.clone(),
                    fingerprint,
                });
                format!("Saved {path}.")
            }
            Ok(WorkspaceOperationResult::Created(path)) => {
                operation_succeeded = true;
                let path = path.to_string_lossy().into_owned();
                self.workshop_review = None;
                workspace_file_result = Some(WorkspaceFileResult::Created {
                    operation_id: completion.id.clone(),
                    path: path.clone(),
                });
                format!("Created {path}.")
            }
            Ok(WorkspaceOperationResult::Completed) => {
                operation_succeeded = true;
                if matches!(
                    &completion.action,
                    WorkspaceOperationKind::AdoptProject(_)
                        | WorkspaceOperationKind::UnregisterProject(_)
                ) {
                    self.workshop_review = None;
                    self.refresh_catalog();
                }
                workspace_file_result = Some(WorkspaceFileResult::Completed {
                    operation_id: completion.id.clone(),
                });
                match &completion.action {
                    WorkspaceOperationKind::Validate => {
                        "Workspace validation completed.".to_owned()
                    }
                    WorkspaceOperationKind::AdoptProject(_) => {
                        "Content project added to the workspace.".to_owned()
                    }
                    WorkspaceOperationKind::UnregisterProject(_) => {
                        "Content project removed from discovery.".to_owned()
                    }
                    _ => "Workspace operation completed.".to_owned(),
                }
            }
            Ok(WorkspaceOperationResult::Unregistered(identity)) => {
                operation_succeeded = true;
                self.workshop_review = None;
                self.refresh_catalog();
                workspace_file_result = Some(WorkspaceFileResult::UnregisteredContent {
                    operation_id: completion.id.clone(),
                    content_id: identity.id,
                    version: identity.version,
                });
                "Content project removed from discovery.".to_owned()
            }
            Ok(WorkspaceOperationResult::Remembered(roots)) => {
                operation_succeeded = true;
                let roots = roots
                    .into_iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                workspace_file_result = Some(WorkspaceFileResult::RememberedSources {
                    operation_id: completion.id.clone(),
                    roots: roots.clone(),
                });
                format!(
                    "Registered {} workspace{}.",
                    roots.len(),
                    if roots.len() == 1 { "" } else { "s" }
                )
            }
            Err(error) => {
                if matches!(
                    &completion.action,
                    WorkspaceOperationKind::DiscoverSuperworkspaceRepositories
                        | WorkspaceOperationKind::AcquireSuperworkspaceRepositories(_)
                ) {
                    self.superworkspace_sources_error = Some(error.clone());
                }
                error
            }
        };
        self.active_operation = None;
        self.revision.0 = self.revision.0.saturating_add(1);
        let succeeded = operation_succeeded;
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: completion.id.clone(),
                composition_id: None,
                action: workspace_operation_action_id(&completion.action).to_owned(),
                phase: OperationPhase::Complete,
                status: if succeeded {
                    OperationStatus::Succeeded
                } else {
                    OperationStatus::Failed {
                        message: message.clone(),
                    }
                },
                progress: Some(1.0),
                message: Some(message),
            }));
        if let Some(result) = workspace_file_result {
            self.operation_events
                .push_back(ClientEvent::WorkspaceFileResult(Box::new(result)));
        }
    }

    fn start_authoring_operation(&mut self, action: AuthoringAction) -> Result<(), ServiceError> {
        if self.bootstrap.fixture_mode {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "Project authoring is unavailable in fixture mode; no real workspace, toolchain, or source registry will be accessed.".to_owned(),
                retryable: false,
            });
        }
        if self.active_operation.is_some() {
            return Err(ServiceError {
                code: ServiceErrorCode::OperationInProgress,
                message: "Wait for the current operation to finish.".to_owned(),
                retryable: true,
            });
        }
        let workspace = self.bootstrap.workspace_root.clone();
        let workspace_bound = !matches!(
            &action,
            AuthoringAction::CreateWorkspace { .. } | AuthoringAction::ListRegisteredSources
        );
        let executable = self.bootstrap.application_executable.clone();
        let operation_id = application::OperationId::fresh();
        let id = operation_id.0.to_string();
        let action_id = authoring_action_id(&action);
        self.active_operation = Some(OperationSummary {
            id: id.clone(),
            composition_id: None,
            action: action_id.to_owned(),
            phase: OperationPhase::Resolving,
            status: OperationStatus::Running,
            progress: None,
        });
        let lifecycle_sender = self.operation_sender.clone();
        let completion_sender = self.operation_sender.clone();
        thread::Builder::new()
            .name(format!("studio-authoring-{id}"))
            .spawn(move || {
                let request = application::CoreOperationRequest {
                    id: operation_id,
                    domain: "studio-authoring".to_owned(),
                    action: action_id.to_owned(),
                };
                let sink: application::OperationEventSink = std::sync::Arc::new(move |event| {
                    if matches!(event.status, application::OperationStatus::Running) {
                        let _ = lifecycle_sender.send(BackgroundOperationMessage::Lifecycle(event));
                    }
                });
                let result = application::OperationService::run_observed(
                    request,
                    sink,
                    |_| execute_authoring_operation(&executable, workspace.as_deref(), action),
                    authoring_result_outcome,
                );
                let _ = completion_sender.send(BackgroundOperationMessage::CompleteAuthoring(
                    Box::new(AuthoringOperationCompletion {
                        id,
                        action: action_id.to_owned(),
                        workspace,
                        workspace_bound,
                        result,
                    }),
                ));
            })
            .map_err(|error| {
                self.active_operation = None;
                ServiceError {
                    code: ServiceErrorCode::InternalFailure,
                    message: format!("Could not start authoring operation: {error}"),
                    retryable: true,
                }
            })?;
        Ok(())
    }

    fn apply_authoring_completion(&mut self, completion: AuthoringOperationCompletion) {
        let mut result = completion.result;
        if completion.workspace_bound
            && self.bootstrap.workspace_root.as_deref() != completion.workspace.as_deref()
        {
            result = Err("The active workspace changed while authoring was in progress.".into());
        }
        let message = result
            .as_ref()
            .map(authoring_result_message)
            .unwrap_or_else(|error| error.clone());
        let succeeded = result.is_ok();
        self.active_operation = None;
        self.revision.0 = self.revision.0.saturating_add(1);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: completion.id.clone(),
                composition_id: None,
                action: completion.action,
                phase: OperationPhase::Complete,
                status: if succeeded {
                    OperationStatus::Succeeded
                } else {
                    OperationStatus::Failed {
                        message: message.clone(),
                    }
                },
                progress: Some(1.0),
                message: Some(message),
            }));
        if let Ok(result) = result {
            match &result {
                AuthoringResultKind::RegisteredSources { .. }
                | AuthoringResultKind::ContentValidated { .. } => {}
                AuthoringResultKind::Composition { .. }
                | AuthoringResultKind::WorkshopMetadata { .. }
                | AuthoringResultKind::RuntimeOutputs { .. } => self.refresh_catalog(),
                AuthoringResultKind::ContentCreated { .. } => self.refresh_catalog(),
                AuthoringResultKind::WorkspaceCreated { .. } => {}
            }
            self.operation_events
                .push_back(ClientEvent::AuthoringResult(Box::new(AuthoringResult {
                    operation_id: completion.id,
                    result,
                })));
        }
    }

    fn start_root_publish_operation(
        &mut self,
        action: PublishingAction,
    ) -> Result<(), ServiceError> {
        if self.bootstrap.fixture_mode {
            return Err(invalid_workshop_request(
                "Steam root publishing is unavailable in Vapor Client fixture mode.",
            ));
        }
        if self.active_operation.is_some() {
            return Err(ServiceError {
                code: ServiceErrorCode::OperationInProgress,
                message: "Wait for the current operation to finish.".to_owned(),
                retryable: true,
            });
        }
        let (mut request, review_token, staged, operation_id) = match action {
            PublishingAction::StageRoot {
                account,
                description,
                set_live,
            } => {
                let account = account.trim();
                let description = description.trim();
                if account.is_empty() || account.len() > 128 {
                    return Err(invalid_workshop_request(
                        "Enter a Steamworks account name of 1 to 128 bytes.",
                    ));
                }
                if description.is_empty() || description.len() > 256 {
                    return Err(invalid_workshop_request(
                        "Enter a release description of 1 to 256 bytes.",
                    ));
                }
                let set_live = set_live
                    .map(|branch| branch.trim().to_owned())
                    .filter(|branch| !branch.is_empty());
                if set_live.as_deref().is_some_and(|branch| {
                    branch.len() > 64
                        || !branch
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
                }) {
                    return Err(invalid_workshop_request(
                        "Beta branch names may contain only letters, numbers, hyphens, underscores, and periods.",
                    ));
                }
                let operation_id = application::OperationId::fresh();
                let id = operation_id.0.to_string();
                self.root_publish_review = None;
                (
                    vapor_sdk_core::root::types::RootPublishRequest {
                        plan: true,
                        app_id: vapor_sdk_core::root::types::ROOT_STEAM_APP_ID,
                        depot_id: vapor_sdk_core::root::types::ROOT_STEAM_DEPOT_ID,
                        description: description.to_owned(),
                        account: account.to_owned(),
                        steamcmd: PathBuf::from("steamcmd"),
                        set_live,
                    },
                    id.clone(),
                    true,
                    id,
                )
            }
            PublishingAction::PublishRoot {
                review_id,
                confirmation,
            } => {
                let review = self
                    .root_publish_review
                    .as_ref()
                    .filter(|review| review.snapshot.review_id == review_id && review.snapshot.planned)
                    .ok_or_else(|| invalid_workshop_request(
                        "This Steam root review is no longer current. Stage a fresh release plan.",
                    ))?;
                let phrase = match review.snapshot.set_live.as_deref() {
                    Some(branch) => format!(
                        "UPLOAD ROOT {} AND SET LIVE {branch}",
                        vapor_sdk_core::root::types::ROOT_STEAM_APP_ID
                    ),
                    None => format!(
                        "UPLOAD ROOT {}",
                        vapor_sdk_core::root::types::ROOT_STEAM_APP_ID
                    ),
                };
                if confirmation.trim() != phrase {
                    return Err(invalid_workshop_request(&format!(
                        "Type {phrase} exactly to authorize this SteamPipe upload."
                    )));
                }
                let mut request = review.request.clone();
                request.plan = false;
                let operation_id = application::OperationId::fresh();
                (request, review_id, false, operation_id.0.to_string())
            }
            PublishingAction::DiscardRootReview => unreachable!("handled by dispatch"),
        };
        request.plan = staged;
        let sender = self.operation_sender.clone();
        self.active_operation = Some(OperationSummary {
            id: operation_id.clone(),
            composition_id: None,
            action: if staged {
                "publishing.stage-root"
            } else {
                "publishing.publish-root"
            }
            .to_owned(),
            phase: OperationPhase::Publishing,
            status: OperationStatus::Running,
            progress: None,
        });
        let action_id = review_token;
        thread::Builder::new()
            .name(format!("studio-root-publish-{operation_id}"))
            .spawn(move || {
                let result = vapor_sdk_core::root::publish::root_publish(&request)
                    .map_err(|error| error.to_string());
                let _ = sender.send(BackgroundOperationMessage::CompleteRootPublish(Box::new(
                    RootPublishOperationCompletion {
                        id: operation_id,
                        review_id: action_id,
                        staged,
                        request,
                        result,
                    },
                )));
            })
            .map_err(|error| {
                self.active_operation = None;
                ServiceError {
                    code: ServiceErrorCode::InternalFailure,
                    message: format!("Could not start Steam root operation: {error}"),
                    retryable: true,
                }
            })?;
        Ok(())
    }

    fn apply_root_publish_completion(&mut self, completion: RootPublishOperationCompletion) {
        let mut success = true;
        let message = match completion.result {
            Ok(report) => {
                let upload_succeeded = report
                    .status
                    .as_ref()
                    .is_some_and(|status| status.success());
                let status = report.status.map(|status| status.to_string());
                let snapshot = RootPublishingSnapshot {
                    review_id: if completion.staged {
                        completion.id.clone()
                    } else {
                        completion.review_id.clone()
                    },
                    planned: report.planned,
                    app_id: report.package.app_id,
                    depot_id: report.package.depot_id,
                    account: report.account,
                    description: report.package.description,
                    set_live: report.set_live,
                    steamcmd: report.steamcmd.display().to_string(),
                    package_root: report.package.package_root.display().to_string(),
                    copied_files: report.package.copied_files,
                    included_roots: report
                        .package
                        .included_roots
                        .into_iter()
                        .map(|path| path.display().to_string())
                        .collect(),
                    status: status.clone(),
                    succeeded: upload_succeeded,
                };
                let summary = if completion.staged {
                    format!(
                        "Steam root upload plan ready · {} files.",
                        snapshot.copied_files
                    )
                } else if upload_succeeded {
                    "SteamCMD completed the root upload.".to_owned()
                } else {
                    success = false;
                    format!(
                        "SteamCMD root upload finished with status {}.",
                        status.as_deref().unwrap_or("unknown")
                    )
                };
                self.root_publish_review = Some(RootPublishReviewState {
                    request: completion.request,
                    snapshot,
                });
                summary
            }
            Err(error) => {
                success = false;
                error
            }
        };
        let Some(active) = self.active_operation.take() else {
            return;
        };
        self.revision.0 = self.revision.0.saturating_add(1);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: active.id,
                composition_id: None,
                action: active.action,
                phase: OperationPhase::Complete,
                status: if success {
                    OperationStatus::Succeeded
                } else {
                    OperationStatus::Failed {
                        message: message.clone(),
                    }
                },
                progress: None,
                message: Some(message),
            }));
    }

    fn start_workshop_operation(&mut self, action: WorkshopAction) -> Result<(), ServiceError> {
        if self.active_operation.is_some() {
            return Err(ServiceError {
                code: ServiceErrorCode::OperationInProgress,
                message: "Wait for the current operation to finish.".to_owned(),
                retryable: true,
            });
        }
        let kind = match action {
            WorkshopAction::Search { query, page } => {
                if query.len() > 256 {
                    return Err(invalid_workshop_request(
                        "Workshop search terms must be 256 bytes or fewer.",
                    ));
                }
                if !(1..=1_000).contains(&page) {
                    return Err(invalid_workshop_request(
                        "Workshop search page must be between 1 and 1000.",
                    ));
                }
                WorkshopOperationKind::Search {
                    query: query.trim().to_owned(),
                    page,
                }
            }
            WorkshopAction::LoadSubscriptions => WorkshopOperationKind::LoadSubscriptions,
            WorkshopAction::Subscribe { item_ids } => WorkshopOperationKind::ChangeSubscriptions {
                ids: parse_workshop_ids(item_ids)?,
                subscribe: true,
            },
            WorkshopAction::Unsubscribe { item_ids } => {
                WorkshopOperationKind::ChangeSubscriptions {
                    ids: parse_workshop_ids(item_ids)?,
                    subscribe: false,
                }
            }
            WorkshopAction::Acquire { item_ids } => WorkshopOperationKind::Acquire {
                ids: parse_workshop_ids(item_ids)?
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
            },
            WorkshopAction::Stage { selector } => {
                let workspace_root = self.validate_workshop_stage(&selector)?;
                self.workshop_review = None;
                WorkshopOperationKind::Stage {
                    selector,
                    workspace_root,
                }
            }
            WorkshopAction::Publish {
                review_id,
                account,
                confirmation,
                change_note,
            } => {
                let review = self
                    .workshop_review
                    .as_ref()
                    .filter(|review| review.review_id == review_id)
                    .cloned()
                    .ok_or_else(|| {
                        invalid_workshop_request(
                            "This Workshop review is no longer current. Stage the content again.",
                        )
                    })?;
                let workspace_root = self.validate_workshop_stage(&review.selector)?;
                if workspace_root != review.workspace_root {
                    return Err(invalid_workshop_request(
                        "The active workspace changed after this review was staged.",
                    ));
                }
                let account = account.trim().to_owned();
                if account.is_empty() || account.len() > 128 {
                    return Err(invalid_workshop_request(
                        "Enter a publisher account name of 1 to 128 bytes.",
                    ));
                }
                let expected_confirmation = format!("PUBLISH {} TO {account}", review.preview.id);
                if confirmation.trim() != expected_confirmation {
                    return Err(invalid_workshop_request(&format!(
                        "Type {expected_confirmation} exactly to authorize this Workshop upload."
                    )));
                }
                let change_note = change_note.trim();
                if change_note.len() > 500 {
                    return Err(invalid_workshop_request(
                        "Workshop change notes must be 500 bytes or fewer.",
                    ));
                }
                WorkshopOperationKind::Publish {
                    review: Box::new(review),
                    account,
                    change_note: (!change_note.is_empty()).then(|| change_note.to_owned()),
                }
            }
            WorkshopAction::DiscardReview => {
                return Err(invalid_workshop_request(
                    "Workshop review discard must use the synchronous service path.",
                ));
            }
        };
        let auto_install = self
            .auto_install_context(&kind)
            .map_err(|message| ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message,
                retryable: true,
            })?;
        let operation_id = application::OperationId::fresh();
        let id = operation_id.0.to_string();
        let action_id = workshop_operation_action_id(&kind);
        self.active_operation = Some(OperationSummary {
            id: id.clone(),
            composition_id: None,
            action: action_id.to_owned(),
            phase: OperationPhase::Resolving,
            status: OperationStatus::Running,
            progress: None,
        });

        let fixture_mode = self.bootstrap.fixture_mode;
        let application_executable = self.bootstrap.application_executable.clone();
        let workshop_app_id = self.bootstrap.workshop_app_id;
        let fixture_subscriptions = self.fixture_workshop_subscriptions.clone();
        let auto_install = auto_install.clone();
        let lifecycle_sender = self.operation_sender.clone();
        let completion_sender = self.operation_sender.clone();
        thread::Builder::new()
            .name(format!("studio-workshop-{id}"))
            .spawn(move || {
                let request = application::CoreOperationRequest {
                    id: operation_id,
                    domain: "studio-workshop".to_owned(),
                    action: action_id.to_owned(),
                };
                let sink: application::OperationEventSink = std::sync::Arc::new(move |event| {
                    if matches!(event.status, application::OperationStatus::Running) {
                        let _ = lifecycle_sender.send(BackgroundOperationMessage::Lifecycle(event));
                    }
                });
                let action_for_worker = kind.clone();
                let result = application::OperationService::run_observed(
                    request,
                    sink,
                    |_| {
                        execute_workshop_operation(
                            &application_executable,
                            fixture_mode,
                            workshop_app_id,
                            fixture_subscriptions,
                            auto_install,
                            action_for_worker,
                        )
                    },
                    workshop_result_outcome,
                );
                let _ = completion_sender.send(BackgroundOperationMessage::CompleteWorkshop(
                    Box::new(WorkshopOperationCompletion {
                        id,
                        action: kind,
                        result,
                    }),
                ));
            })
            .map_err(|error| {
                self.active_operation = None;
                ServiceError {
                    code: ServiceErrorCode::InternalFailure,
                    message: format!("Could not start Workshop operation: {error}"),
                    retryable: true,
                }
            })?;
        Ok(())
    }

    fn auto_install_context(
        &self,
        kind: &WorkshopOperationKind,
    ) -> Result<AutoInstallContext, String> {
        let requested_ids = match kind {
            WorkshopOperationKind::Acquire { ids } => ids.iter().cloned().collect::<BTreeSet<_>>(),
            WorkshopOperationKind::ChangeSubscriptions {
                ids,
                subscribe: true,
            } => ids.iter().map(u64::to_string).collect::<BTreeSet<_>>(),
            _ => return Ok(AutoInstallContext::default()),
        };
        let mut context = AutoInstallContext::default();
        for item in self.catalog.entries().iter().filter(|item| {
            item.published_file_id
                .as_ref()
                .is_some_and(|id| requested_ids.contains(id))
        }) {
            let reference = ContentReference::parse(item.id.clone(), item.version.clone())
                .map_err(|error| format!("invalid cached content identity: {error}"))?;
            let content_id = reference
                .vapor_id()
                .map_err(|error| format!("invalid cached content ID: {error}"))?;
            if !vapor_client_domain::library_auto_install_enabled(
                &self.durable_state.library_policies,
                &content_id,
            ) {
                continue;
            }
            let constraints = vapor_client_domain::library_constraints(
                &self.durable_state.compositions,
                &content_id,
            )
            .map_err(|missing| {
                format!(
                    "Auto-Install paused: composition '{}' has an unresolved version constraint for {}. Resolve that composition before updating this item.",
                    missing.composition_id, content_id
                )
            })?;
            context.constraints.insert(
                content_id.to_string(),
                constraints.iter().map(ToString::to_string).collect(),
            );
            for composition in &self.durable_state.compositions.compositions {
                if composition.root_package.is_some() && composition.resolved_graph.is_none() {
                    return Err(format!(
                        "Auto-Install paused: composition '{}' has not been resolved, so Vapor cannot verify that this update is safe.",
                        composition.name
                    ));
                }
                let depends_on_item = composition.resolved_graph.as_ref().is_some_and(|graph| {
                    graph
                        .nodes
                        .iter()
                        .any(|node| node.content.id == content_id.as_str())
                });
                if depends_on_item
                    && composition.root_package.is_some()
                    && matches!(
                        composition.installation,
                        InstallationState::Installed { .. } | InstallationState::OutOfDate { .. }
                    )
                {
                    context
                        .compositions
                        .entry(content_id.to_string())
                        .or_default()
                        .push(AutoInstallCompositionTarget {
                            composition: composition.clone(),
                            inputs: CompositionOperationInputs::capture(
                                composition,
                                CompositionOperationKind::Update,
                            ),
                        });
                }
            }
        }
        Ok(context)
    }

    fn apply_auto_install_updates(
        &mut self,
        updates: Vec<AutoInstallCompositionUpdate>,
    ) -> Result<Option<String>, String> {
        if updates.is_empty() {
            return Ok(None);
        }
        let previous_compositions = self.durable_state.compositions.compositions.clone();
        let previous_revision = self.durable_state.composition_revision;
        let mut updated = 0usize;
        let mut failures = Vec::new();
        for update in updates {
            let current = self
                .durable_state
                .compositions
                .compositions
                .iter()
                .find(|composition| composition.id == update.composition_id)
                .map(|composition| {
                    CompositionOperationInputs::capture(
                        composition,
                        CompositionOperationKind::Update,
                    )
                });
            if current.as_ref() != Some(&update.inputs) {
                failures.push(format!(
                    "composition '{}' changed during the Workshop download and was not changed by Auto-Install",
                    update.composition_id
                ));
                continue;
            }
            let failed = apply_operation_result_to_composition(
                &mut self.composition_preview,
                &mut self.durable_state.compositions.compositions,
                &update.composition_id,
                CompositionOperationKind::Update,
                CompositionOperationResult::Update(update.result),
            );
            if let Some(failure) = failed {
                failures.push(format!("{}: {failure}", update.composition_id));
            } else {
                updated += 1;
            }
        }
        let changed = self.durable_state.compositions.compositions != previous_compositions;
        if changed {
            self.durable_state.composition_revision = previous_revision.saturating_add(1);
            if let Some(store) = &self.state_store
                && let Err(error) = store.save(&self.durable_state)
            {
                self.durable_state.compositions.compositions = previous_compositions;
                self.durable_state.composition_revision = previous_revision;
                self.refresh_catalog();
                return Err(format!(
                    "Workshop content was downloaded, but Auto-Install could not save composition state: {error}"
                ));
            }
        }
        match self.collect_unreferenced_auto_install() {
            Ok(_) => {}
            Err(error) => failures.push(format!("Library cleanup: {error}")),
        }
        self.refresh_catalog();
        if failures.is_empty() {
            return Ok((updated > 0).then(|| {
                format!(
                    "Auto-Install updated {updated} dependent composition{}.",
                    if updated == 1 { "" } else { "s" }
                )
            }));
        }
        Err(format!(
            "Workshop content was downloaded, but Auto-Install could not update every dependent composition: {}",
            failures.join("; ")
        ))
    }

    fn apply_workshop_completion(&mut self, completion: WorkshopOperationCompletion) {
        let mut succeeded = completion.result.is_ok();
        let mut failure = completion.result.as_ref().err().cloned();
        let mut summary = None;
        match completion.result {
            Ok(WorkshopOperationResult::Search(page)) => {
                summary = Some(format!(
                    "Steam Workshop returned {} items on page {} of {}.",
                    page.items.len(),
                    page.page,
                    page.total_results
                ));
                let revision = self
                    .workshop
                    .search
                    .as_ref()
                    .map_or(1, |search| search.revision.saturating_add(1));
                self.workshop.search = Some(workshop_search_snapshot(
                    match &completion.action {
                        WorkshopOperationKind::Search { query, .. } => query.clone(),
                        _ => String::new(),
                    },
                    page,
                    revision,
                ));
            }
            Ok(WorkshopOperationResult::Subscriptions(subscriptions)) => {
                self.fixture_workshop_subscriptions = subscriptions
                    .iter()
                    .map(|item| item.published_file_id.clone())
                    .collect();
                self.workshop.subscriptions = subscriptions
                    .iter()
                    .map(workshop_subscription_snapshot)
                    .collect();
                self.workshop.subscriptions_loaded = true;
                self.workshop.subscriptions_revision =
                    self.workshop.subscriptions_revision.saturating_add(1);
                summary = Some(format!(
                    "Loaded {} Steam Workshop subscriptions.",
                    subscriptions.len()
                ));
            }
            Ok(WorkshopOperationResult::SubscriptionWithAcquisition {
                subscriptions,
                acquisition,
            }) => {
                self.fixture_workshop_subscriptions = subscriptions
                    .iter()
                    .map(|item| item.published_file_id.clone())
                    .collect();
                self.workshop.subscriptions = subscriptions
                    .iter()
                    .map(workshop_subscription_snapshot)
                    .collect();
                self.workshop.subscriptions_loaded = true;
                self.workshop.subscriptions_revision =
                    self.workshop.subscriptions_revision.saturating_add(1);
                match acquisition {
                    Ok((items, requested, updates)) => {
                        let revision = self
                            .workshop
                            .acquisition
                            .as_ref()
                            .map_or(1, |batch| batch.revision.saturating_add(1));
                        let items = items
                            .iter()
                            .map(workshop_acquisition_snapshot)
                            .collect::<Vec<_>>();
                        let imported = items.len();
                        self.fixture_acquisitions
                            .extend(items.iter().cloned().map(|item| (item.id.clone(), item)));
                        self.workshop.acquisition = Some(WorkshopAcquisitionBatchSnapshot {
                            revision,
                            requested,
                            items,
                        });
                        summary = Some(format!(
                            "Subscribed and added {imported} Workshop item{} to your Library.",
                            if imported == 1 { "" } else { "s" }
                        ));
                        self.refresh_catalog();
                        match self.apply_auto_install_updates(updates) {
                            Ok(Some(message)) => summary = Some(message),
                            Err(message) => {
                                succeeded = false;
                                failure = Some(message);
                            }
                            Ok(None) => {}
                        }
                    }
                    Err(error) => {
                        succeeded = false;
                        failure = Some(format!(
                            "Subscription is active, but Vapor Client could not add the item to your Library: {error}"
                        ));
                    }
                }
            }
            Ok(WorkshopOperationResult::Acquisitions {
                items,
                requested,
                updates,
            }) => {
                let revision = self
                    .workshop
                    .acquisition
                    .as_ref()
                    .map_or(1, |batch| batch.revision.saturating_add(1));
                let items = items
                    .iter()
                    .map(workshop_acquisition_snapshot)
                    .collect::<Vec<_>>();
                let imported = items.len();
                if self.bootstrap.fixture_mode {
                    self.fixture_acquisitions
                        .extend(items.iter().cloned().map(|item| (item.id.clone(), item)));
                }
                self.workshop.acquisition = Some(WorkshopAcquisitionBatchSnapshot {
                    revision,
                    requested,
                    items,
                });
                summary = Some(format!(
                    "Added {} Workshop items to your Library.",
                    imported
                ));
                self.refresh_catalog();
                match self.apply_auto_install_updates(updates) {
                    Ok(Some(message)) => summary = Some(message),
                    Err(message) => {
                        succeeded = false;
                        failure = Some(message);
                    }
                    Ok(None) => {}
                }
            }
            Ok(WorkshopOperationResult::Review(preview)) => {
                let (selector, workspace_root) = match &completion.action {
                    WorkshopOperationKind::Stage {
                        selector,
                        workspace_root,
                    } => (selector.clone(), workspace_root.clone()),
                    _ => (String::new(), PathBuf::new()),
                };
                if self.bootstrap.workspace_root.as_deref() != Some(workspace_root.as_path()) {
                    succeeded = false;
                    failure = Some(
                        "The active workspace changed while this Workshop item was being staged. Stage it again from the current workspace.".to_owned(),
                    );
                } else {
                    self.workshop_review = Some(WorkshopReviewState {
                        review_id: completion.id.clone(),
                        selector,
                        workspace_root,
                        preview: preview.clone(),
                    });
                    summary = Some(format!(
                        "Staged {} · {} files · {} bytes for review.",
                        preview.id, preview.files, preview.bytes
                    ));
                }
            }
            Ok(WorkshopOperationResult::Publication(publication)) => {
                self.workshop_review = None;
                summary = Some(format!(
                    "{} Workshop item {}{}; receipt saved at {}.",
                    if publication.created {
                        "Created"
                    } else {
                        "Updated"
                    },
                    publication.id,
                    publication
                        .published_file_id
                        .as_ref()
                        .map(|id| format!(" · PublishedFileId {id}"))
                        .unwrap_or_default(),
                    publication.receipt.display()
                ));
                self.refresh_catalog();
            }
            Err(_) => {}
        }
        self.active_operation = None;
        self.revision.0 = self.revision.0.saturating_add(1);
        let action = workshop_operation_action_id(&completion.action).to_owned();
        let message = failure.or(summary);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: completion.id,
                composition_id: None,
                action,
                phase: OperationPhase::Complete,
                status: if succeeded {
                    OperationStatus::Succeeded
                } else {
                    OperationStatus::Failed {
                        message: message
                            .clone()
                            .unwrap_or_else(|| "Workshop operation failed.".to_owned()),
                    }
                },
                progress: Some(1.0),
                message,
            }));
    }

    fn apply_composition_completion(&mut self, completion: CompositionOperationCompletion) {
        let composition_id = completion.composition_id.clone();
        let current_inputs = self
            .durable_state
            .compositions
            .compositions
            .iter()
            .find(|composition| composition.id == composition_id)
            .map(|composition| CompositionOperationInputs::capture(composition, completion.action));
        if current_inputs.as_ref() != Some(&completion.inputs) {
            self.apply_obsolete_composition_completion(completion);
            return;
        }
        let previous_compositions = self.durable_state.compositions.compositions.clone();
        let previous_composition_revision = self.durable_state.composition_revision;
        let outcome = composition_result_outcome(&completion.result);
        let mut failure = apply_operation_result_to_composition(
            &mut self.composition_preview,
            &mut self.durable_state.compositions.compositions,
            &composition_id,
            completion.action,
            completion.result,
        );
        if failure.is_none() {
            failure = outcome.as_ref().err().cloned();
        }
        if self.durable_state.compositions.compositions != previous_compositions {
            self.durable_state.composition_revision =
                self.durable_state.composition_revision.saturating_add(1);
            if let Some(store) = &self.state_store
                && let Err(error) = store.save(&self.durable_state)
            {
                self.durable_state.compositions.compositions = previous_compositions;
                self.durable_state.composition_revision = previous_composition_revision;
                failure = Some(format!("Could not save the composition result: {error}"));
            }
        }
        if let Some(message) = failure {
            self.active_operation = None;
            self.revision.0 = self.revision.0.saturating_add(1);
            self.operation_events
                .push_back(ClientEvent::OperationChanged(OperationEvent {
                    revision: self.revision,
                    id: completion.id,
                    composition_id: Some(composition_id),
                    action: operation_action_id(completion.action).to_owned(),
                    phase: OperationPhase::Complete,
                    status: OperationStatus::Failed {
                        message: message.clone(),
                    },
                    progress: None,
                    message: Some(message),
                }));
            return;
        }
        let summary = outcome.unwrap_or_else(|_| "Completed".to_owned());
        self.active_operation = None;
        if matches!(
            completion.action,
            CompositionOperationKind::Install
                | CompositionOperationKind::Repair
                | CompositionOperationKind::Update
                | CompositionOperationKind::Launch
                | CompositionOperationKind::RunSource
        ) {
            self.refresh_catalog();
        }
        self.revision.0 = self.revision.0.saturating_add(1);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: completion.id,
                composition_id: Some(composition_id),
                action: operation_action_id(completion.action).to_owned(),
                phase: OperationPhase::Complete,
                status: OperationStatus::Succeeded,
                progress: Some(1.0),
                message: Some(summary),
            }));
    }

    fn apply_obsolete_composition_completion(
        &mut self,
        completion: CompositionOperationCompletion,
    ) {
        let outcome = composition_result_outcome(&completion.result);
        let composition_id = completion.composition_id;
        let action = completion.action;
        let side_effecting = matches!(
            action,
            CompositionOperationKind::Install
                | CompositionOperationKind::Repair
                | CompositionOperationKind::Update
                | CompositionOperationKind::Launch
                | CompositionOperationKind::RunSource
        );
        if side_effecting {
            // The external operation may have changed installed content even
            // though its result no longer belongs to the current composition intent.
            self.refresh_catalog();
        }
        let succeeded = outcome.is_ok() && !matches!(action, CompositionOperationKind::Preview);
        let summary = if matches!(action, CompositionOperationKind::Preview) {
            match outcome {
                Ok(_) => "The preview was discarded because the composition changed while it was running."
                    .to_owned(),
                Err(error) => format!(
                    "The preview was discarded because the composition changed while it was running. {error}"
                ),
            }
        } else {
            match outcome {
                Ok(summary) => {
                    let follow_up = match action {
                        CompositionOperationKind::Install
                        | CompositionOperationKind::Repair
                        | CompositionOperationKind::Update => {
                            "Review installed content before continuing."
                        }
                        CompositionOperationKind::Verify => {
                            "The current composition's verification state was left unchanged."
                        }
                        CompositionOperationKind::Launch | CompositionOperationKind::RunSource => {
                            "The operation used the previous selection; the current composition was left unchanged."
                        }
                        CompositionOperationKind::Preview => unreachable!(),
                    };
                    format!(
                        "{summary}. This completed for the previous composition selection. {follow_up}"
                    )
                }
                Err(error) => format!(
                    "The composition changed while this operation was running, so its result was not applied to the current composition. {error}"
                ),
            }
        };
        self.active_operation = None;
        self.revision.0 = self.revision.0.saturating_add(1);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: completion.id,
                composition_id: Some(composition_id),
                action: operation_action_id(action).to_owned(),
                phase: OperationPhase::Complete,
                status: if succeeded {
                    OperationStatus::Succeeded
                } else {
                    OperationStatus::Failed {
                        message: summary.clone(),
                    }
                },
                progress: succeeded.then_some(1.0),
                message: Some(summary),
            }));
    }

    fn refresh_catalog(&mut self) {
        self.refresh_workspace_content();
        if self.bootstrap.fixture_mode {
            let mut entries = vec![CatalogEntry {
                id: "fixture.engine".to_owned(),
                name: Some("Fixture Engine".to_owned()),
                version: Some("1.0.0".to_owned()),
                kind: "engine".to_owned(),
                published_file_id: None,
                dependencies: Vec::new(),
                conflicts: Vec::new(),
                locations: vec![CatalogLocation {
                    kind: CatalogLocationKind::Cache,
                    root: "fixture/cache/fixture.engine".to_owned(),
                    enabled: true,
                    fingerprint: Some("fixture:v1".to_owned()),
                    install_source: None,
                }],
                role: CatalogItemRole::Other,
                actions: Vec::new(),
            }];
            entries.extend(self.fixture_acquisitions.values().map(|item| CatalogEntry {
                id: item.id.clone(),
                name: Some(item.id.clone()),
                version: item.version.clone(),
                kind: item.kind.clone(),
                published_file_id: None,
                dependencies: Vec::new(),
                conflicts: Vec::new(),
                locations: vec![CatalogLocation {
                    kind: CatalogLocationKind::Cache,
                    root: item.cache_root.clone(),
                    enabled: true,
                    fingerprint: Some(item.fingerprint.clone()),
                    install_source: None,
                }],
                role: CatalogItemRole::Other,
                actions: Vec::new(),
            }));
            self.catalog.replace(entries);
            return;
        }
        match discover_content_catalog(
            &self.bootstrap.application_executable,
            self.bootstrap.workspace_root.as_deref(),
        ) {
            Ok(catalog) => self
                .catalog
                .replace(catalog.into_iter().map(catalog_entry).collect()),
            Err(error) => self.catalog.fail(error),
        }
    }

    fn refresh_workspace_content(&mut self) {
        let Some(root) = self.bootstrap.workspace_root.clone() else {
            self.workspace_content = WorkspaceContentSnapshot {
                source_projects_loaded: true,
                unregistered_projects_loaded: true,
                ..WorkspaceContentSnapshot::default()
            };
            return;
        };
        if self.bootstrap.fixture_mode {
            self.workspace_content = fixture_workspace_content(&root);
            return;
        }

        self.workspace_content.workspace_validation_error =
            vapor_client_application::validate_workspace(&root).err();

        match discover_source_content(&self.bootstrap.application_executable, &root) {
            Ok(projects) => {
                self.workspace_content.source_projects = projects
                    .into_iter()
                    .take(MAX_WORKSPACE_PROJECTS)
                    .map(source_project)
                    .collect();
                self.workspace_content.source_projects_loaded = true;
                self.workspace_content.source_projects_error = None;
            }
            Err(error) => self.workspace_content.source_projects_error = Some(error),
        }

        match scan_unregistered_content_projects(&self.bootstrap.application_executable, &root) {
            Ok(scan) => {
                self.workspace_content.unregistered_projects = scan
                    .candidates
                    .into_iter()
                    .take(MAX_WORKSPACE_PROJECTS)
                    .map(|candidate| WorkspaceProjectCandidate {
                        path: candidate.path.display().to_string(),
                        id: candidate.id,
                        kind: candidate.kind.to_string(),
                    })
                    .collect();
                self.workspace_content.unregistered_diagnostics = scan
                    .diagnostics
                    .into_iter()
                    .take(MAX_WORKSPACE_DIAGNOSTICS)
                    .collect();
                self.workspace_content.unregistered_projects_loaded = true;
                self.workspace_content.unregistered_projects_error = None;
            }
            Err(error) => self.workspace_content.unregistered_projects_error = Some(error),
        }
    }

    fn superworkspace_snapshot(&self) -> SuperworkspaceSnapshot {
        if self.bootstrap.fixture_mode {
            return SuperworkspaceSnapshot {
                status: "fixture".to_owned(),
                ..SuperworkspaceSnapshot::default()
            };
        }
        let installation = match vapor_core::VaporInstallation::discover() {
            Ok(installation) => installation,
            Err(error) => {
                return SuperworkspaceSnapshot {
                    status: "unavailable".to_owned(),
                    error: Some(error.to_string()),
                    ..SuperworkspaceSnapshot::default()
                };
            }
        };
        let configured = match vapor_core::source_state(&installation) {
            Ok(state) => state.superworkspace,
            Err(error) => {
                return SuperworkspaceSnapshot {
                    status: "unavailable".to_owned(),
                    error: Some(error.to_string()),
                    ..SuperworkspaceSnapshot::default()
                };
            }
        };
        let is_configured = configured.is_some();
        let root = configured.or_else(default_superworkspace_path);
        let Some(root) = root else {
            return SuperworkspaceSnapshot {
                status: "unavailable".to_owned(),
                error: Some("Your home folder could not be resolved.".to_owned()),
                ..SuperworkspaceSnapshot::default()
            };
        };
        let mut snapshot = SuperworkspaceSnapshot {
            status: "not-initialized".to_owned(),
            root: Some(root.display().to_string()),
            ..SuperworkspaceSnapshot::default()
        };
        match vapor_core::VaporSuperworkspace::discover_from(&root) {
            Ok(discovered) if discovered.root == root => {
                snapshot.status = "ready".to_owned();
                snapshot.repositories = discovered
                    .repositories
                    .into_iter()
                    .map(|repository| SuperworkspaceRepositorySnapshot {
                        name: repository.name,
                        root: repository.root.display().to_string(),
                        kind: repository.kind.to_string(),
                    })
                    .collect();
                snapshot.available_repositories = self
                    .superworkspace_sources
                    .iter()
                    .cloned()
                    .map(|mut repository| {
                        repository.acquired = snapshot
                            .repositories
                            .iter()
                            .any(|local| local.name == repository.checkout_path);
                        repository
                    })
                    .collect();
                snapshot.available_repositories_loaded = self.superworkspace_sources_loaded;
                snapshot.available_repositories_error = self.superworkspace_sources_error.clone();
            }
            Ok(discovered) => {
                snapshot.status = "unavailable".to_owned();
                snapshot.error = Some(format!(
                    "The selected folder is inside another Superworkspace at {}.",
                    discovered.root.display()
                ));
            }
            Err(error) if is_configured => {
                snapshot.status = "unavailable".to_owned();
                snapshot.error = Some(error.to_string());
            }
            Err(_) => {}
        }
        if let Some(error) = self.superworkspace_setup_error.as_ref() {
            snapshot.error = Some(error.clone());
            if snapshot.status != "ready" {
                snapshot.status = "unavailable".to_owned();
            }
        }
        snapshot
    }

    fn apply_composition_action(&mut self, action: CompositionAction) -> Result<(), ServiceError> {
        let may_release_content = matches!(&action, CompositionAction::Remove { .. });
        let previous_state = self.durable_state.clone();
        let application_executable = self.bootstrap.application_executable.clone();
        let fixture_mode = self.bootstrap.fixture_mode;
        let transition =
            vapor_client_domain::apply_composition_action(&mut self.durable_state, action, || {
                if fixture_mode {
                    Ok("fixture.pack".to_owned())
                } else {
                    vapor_client_application::default_packagepack_selector(
                        &application_executable,
                        "default",
                    )
                }
            });
        if let Err(message) = transition {
            self.durable_state = previous_state;
            self.active_composition_id = Some(self.durable_state.active_composition_id.clone());
            return Err(ServiceError {
                code: ServiceErrorCode::InvalidRequest,
                message,
                retryable: true,
            });
        }
        self.durable_state.composition_revision =
            previous_state.composition_revision.saturating_add(1);
        self.active_composition_id = Some(self.durable_state.active_composition_id.clone());
        if let Some(store) = &self.state_store
            && let Err(message) = store.save(&self.durable_state)
        {
            self.durable_state = previous_state;
            self.active_composition_id = Some(self.durable_state.active_composition_id.clone());
            return Err(ServiceError {
                code: ServiceErrorCode::PersistenceFailure,
                message,
                retryable: true,
            });
        }
        if may_release_content && let Err(message) = self.collect_unreferenced_auto_install() {
            self.catalog.fail(format!(
                "The composition was removed, but automatic Library cleanup could not finish: {message}"
            ));
        }
        Ok(())
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

fn operation_action_id(kind: CompositionOperationKind) -> &'static str {
    match kind {
        CompositionOperationKind::Preview => "composition.preview",
        CompositionOperationKind::Install => "composition.install",
        CompositionOperationKind::Verify => "composition.verify",
        CompositionOperationKind::Repair => "composition.repair",
        CompositionOperationKind::Update => "composition.update",
        CompositionOperationKind::Launch => "composition.launch",
        CompositionOperationKind::RunSource => "composition.run-source",
    }
}

fn invalid_workshop_request(message: &str) -> ServiceError {
    ServiceError {
        code: ServiceErrorCode::InvalidRequest,
        message: message.to_owned(),
        retryable: false,
    }
}

fn parse_workshop_ids(ids: Vec<String>) -> Result<Vec<u64>, ServiceError> {
    if ids.is_empty() {
        return Err(invalid_workshop_request(
            "Select at least one Workshop item.",
        ));
    }
    if ids.len() > vapor_client_application::MAX_WORKSHOP_SUBSCRIPTION_BATCH {
        return Err(invalid_workshop_request(&format!(
            "At most {} Workshop items can be changed at once.",
            vapor_client_application::MAX_WORKSHOP_SUBSCRIPTION_BATCH
        )));
    }
    let mut seen = BTreeSet::new();
    let mut parsed = Vec::new();
    for id in ids {
        let value = id.trim().parse::<u64>().ok().filter(|value| *value != 0);
        let Some(value) = value else {
            return Err(invalid_workshop_request(&format!(
                "Invalid Steam PublishedFileId: {}",
                id.trim()
            )));
        };
        if seen.insert(value) {
            parsed.push(value);
        }
    }
    Ok(parsed)
}

fn workshop_operation_action_id(kind: &WorkshopOperationKind) -> &'static str {
    match kind {
        WorkshopOperationKind::Search { .. } => "workshop.search",
        WorkshopOperationKind::LoadSubscriptions => "workshop.load-subscriptions",
        WorkshopOperationKind::ChangeSubscriptions {
            subscribe: true, ..
        } => "workshop.subscribe",
        WorkshopOperationKind::ChangeSubscriptions {
            subscribe: false, ..
        } => "workshop.unsubscribe",
        WorkshopOperationKind::Acquire { .. } => "workshop.acquire",
        WorkshopOperationKind::Stage { .. } => "workshop.stage",
        WorkshopOperationKind::Publish { .. } => "workshop.publish",
    }
}

fn validate_workspace_relative_path(path: &str) -> Result<PathBuf, ServiceError> {
    let path = PathBuf::from(path);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(invalid_workspace_request(
            "Choose a file path relative to the open workspace.",
        ));
    }
    Ok(path)
}

fn authoring_action_id(action: &AuthoringAction) -> &'static str {
    match action {
        AuthoringAction::LoadComposition { .. } => "authoring.load-composition",
        AuthoringAction::ListRegisteredSources => "authoring.list-registered-sources",
        AuthoringAction::SaveComposition { .. } => "authoring.save-composition",
        AuthoringAction::LoadWorkshopMetadata { .. } => "authoring.load-workshop-metadata",
        AuthoringAction::SaveWorkshopMetadata { .. } => "authoring.save-workshop-metadata",
        AuthoringAction::LoadRuntimeOutputs { .. } => "authoring.load-runtime-outputs",
        AuthoringAction::ValidateContent { .. } => "authoring.validate-content",
        AuthoringAction::SaveRuntimeOutputs { .. } => "authoring.save-runtime-outputs",
        AuthoringAction::CreateContent { .. } => "authoring.create-content",
        AuthoringAction::ForkContent { .. } => "authoring.fork-content",
        AuthoringAction::CreateWorkspace { .. } => "authoring.create-workspace",
    }
}

fn execute_authoring_operation(
    executable: &Path,
    active_workspace: Option<&Path>,
    action: AuthoringAction,
) -> Result<AuthoringResultKind, String> {
    use AuthoringAction as Action;
    match action {
        Action::ListRegisteredSources => {
            let installation =
                vapor_core::VaporInstallation::discover().map_err(|error| error.to_string())?;
            let state =
                vapor_core::source_state(&installation).map_err(|error| error.to_string())?;
            let mut roots = state.known.into_iter().collect::<BTreeSet<_>>();
            if let Some(start) = active_workspace.or(state.active.as_deref())
                && let Ok(superworkspace) = vapor_core::VaporSuperworkspace::discover_from(start)
            {
                roots.extend(
                    superworkspace
                        .repositories
                        .into_iter()
                        .map(|repository| repository.root),
                );
            }
            Ok(AuthoringResultKind::RegisteredSources {
                roots: roots
                    .into_iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect(),
                active: state.active.map(|path| path.to_string_lossy().into_owned()),
            })
        }
        Action::LoadComposition { project_id } => {
            let workspace = active_workspace.ok_or("Open a workspace before editing content.")?;
            vapor_client_application::load_source_composition(executable, workspace, &project_id)
                .map(|value| AuthoringResultKind::Composition {
                    id: value.id,
                    version: value.version,
                    kind: value.kind,
                    manifest: value.manifest.to_string_lossy().into_owned(),
                    source: value.source,
                    links: value
                        .links
                        .into_iter()
                        .map(|link| AuthoringCompositionLink {
                            id: link.id,
                            relationship: link.relationship,
                            optional: link.optional,
                            version: link.version,
                            workshop_id: link.workshop_id,
                            reason: link.reason,
                        })
                        .collect(),
                })
        }
        Action::SaveComposition {
            project_id,
            expected_fingerprint,
            dependencies,
        } => {
            let workspace = active_workspace.ok_or("Open a workspace before editing content.")?;
            let links = dependencies
                .into_iter()
                .map(|link| vapor_client_application::SourceCompositionLink {
                    id: link.project_id,
                    relationship: link.relationship,
                    optional: link.optional,
                    version: link.version_requirement,
                    workshop_id: link.workshop_id,
                    reason: link.reason,
                })
                .collect::<Vec<_>>();
            vapor_client_application::save_source_composition(
                executable,
                workspace,
                &project_id,
                &expected_fingerprint,
                &links,
            )
            .map(|value| AuthoringResultKind::Composition {
                id: value.id,
                version: value.version,
                kind: value.kind,
                manifest: value.manifest.to_string_lossy().into_owned(),
                source: value.source,
                links: value
                    .links
                    .into_iter()
                    .map(|link| AuthoringCompositionLink {
                        id: link.id,
                        relationship: link.relationship,
                        optional: link.optional,
                        version: link.version,
                        workshop_id: link.workshop_id,
                        reason: link.reason,
                    })
                    .collect(),
            })
        }
        Action::LoadWorkshopMetadata { project_id } => {
            let workspace = active_workspace.ok_or("Open a workspace before editing content.")?;
            vapor_client_application::load_source_workshop_metadata(
                executable,
                workspace,
                &project_id,
            )
            .map(|value| AuthoringResultKind::WorkshopMetadata {
                id: value.id,
                version: value.version,
                kind: value.kind,
                manifest: value.manifest.to_string_lossy().into_owned(),
                source: value.source,
                policy: workshop_policy_to_protocol(value.policy),
            })
        }
        Action::SaveWorkshopMetadata {
            project_id,
            expected_fingerprint,
            app_id,
            published_file_id,
            visibility,
            title,
            description,
            tags,
            change_note,
        } => {
            let workspace = active_workspace.ok_or("Open a workspace before editing content.")?;
            vapor_client_application::save_source_workshop_metadata(
                executable,
                workspace,
                &project_id,
                &expected_fingerprint,
                &vapor_client_application::WorkshopPolicyDraft {
                    app_id,
                    published_file_id,
                    visibility,
                    title,
                    description,
                    tags,
                    change_note,
                },
            )
            .map(|value| AuthoringResultKind::WorkshopMetadata {
                id: value.id,
                version: value.version,
                kind: value.kind,
                manifest: value.manifest.to_string_lossy().into_owned(),
                source: value.source,
                policy: workshop_policy_to_protocol(value.policy),
            })
        }
        Action::LoadRuntimeOutputs { project_id } => {
            let workspace = active_workspace.ok_or("Open a workspace before editing content.")?;
            vapor_client_application::load_source_runtime_outputs(
                executable,
                workspace,
                &project_id,
            )
            .map(|value| AuthoringResultKind::RuntimeOutputs {
                id: value.id,
                version: value.version,
                kind: value.kind,
                manifest: value.manifest.to_string_lossy().into_owned(),
                source: value.source,
                binaries: value.binaries,
                libraries: value.libraries,
            })
        }
        Action::ValidateContent { project_id } => {
            let workspace =
                active_workspace.ok_or("Open a workspace before validating content.")?;
            vapor_client_application::validate_composition_source(
                executable,
                workspace,
                &project_id,
            )
            .map(|summary| AuthoringResultKind::ContentValidated {
                id: project_id,
                summary,
            })
        }
        Action::SaveRuntimeOutputs {
            project_id,
            expected_fingerprint,
            binaries,
            libraries,
        } => {
            let workspace = active_workspace.ok_or("Open a workspace before editing content.")?;
            vapor_client_application::save_source_runtime_outputs(
                executable,
                workspace,
                &project_id,
                &expected_fingerprint,
                &binaries,
                &libraries,
            )
            .map(|value| AuthoringResultKind::RuntimeOutputs {
                id: value.id,
                version: value.version,
                kind: value.kind,
                manifest: value.manifest.to_string_lossy().into_owned(),
                source: value.source,
                binaries: value.binaries,
                libraries: value.libraries,
            })
        }
        Action::CreateContent {
            workspace_root,
            kind,
            id,
            version,
            dependencies,
            optional_dependencies,
            workshop,
        } => {
            let active = active_workspace.ok_or("Open a workspace before creating content.")?;
            let requested = PathBuf::from(workspace_root)
                .canonicalize()
                .map_err(|error| error.to_string())?;
            let active = active.canonicalize().map_err(|error| error.to_string())?;
            if requested != active {
                return Err("Content creation must target the currently open workspace.".to_owned());
            }
            let kind = parse_authoring_content_kind(&kind)?;
            let policy = vapor_client_application::WorkshopPolicyDraft {
                app_id: workshop.app_id,
                published_file_id: workshop.published_file_id,
                visibility: workshop.visibility,
                title: workshop.title,
                description: workshop.description,
                tags: workshop.tags,
                change_note: workshop.change_note,
            };
            let manifest =
                vapor_client_application::create_content_workspace_with_dependency_policies(
                    executable,
                    &active,
                    &id,
                    &version,
                    kind,
                    vapor_client_application::ContentDependencyDraft {
                        required: dependencies,
                        optional: optional_dependencies,
                    },
                    &policy,
                )?;
            let contents = std::fs::read_to_string(&manifest)
                .map_err(|error| format!("{}: {error}", manifest.display()))?;
            let files = vapor_client_application::discover_workspace_files(&active)?
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            Ok(AuthoringResultKind::ContentCreated {
                workspace_root: active.to_string_lossy().into_owned(),
                manifest: manifest
                    .strip_prefix(&active)
                    .unwrap_or(&manifest)
                    .to_string_lossy()
                    .into_owned(),
                manifest_contents: contents,
                files,
            })
        }
        Action::ForkContent {
            workspace_root,
            content_id,
            name,
        } => {
            let active = active_workspace.ok_or("Open a workspace before forking content.")?;
            let requested = PathBuf::from(workspace_root)
                .canonicalize()
                .map_err(|error| error.to_string())?;
            let active = active.canonicalize().map_err(|error| error.to_string())?;
            if requested != active {
                return Err("Content forks must target the currently open workspace.".to_owned());
            }
            if !valid_workspace_slug(&name) {
                return Err(
                    "Fork name must use lowercase letters, digits, and internal hyphens."
                        .to_owned(),
                );
            }
            let manifest = vapor_client_application::fork_acquired_content(
                executable,
                &active,
                &content_id,
                &name,
            )?;
            let contents = std::fs::read_to_string(&manifest)
                .map_err(|error| format!("{}: {error}", manifest.display()))?;
            let files = vapor_client_application::discover_workspace_files(&active)?
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            Ok(AuthoringResultKind::ContentCreated {
                workspace_root: active.to_string_lossy().into_owned(),
                manifest: manifest
                    .strip_prefix(&active)
                    .unwrap_or(&manifest)
                    .to_string_lossy()
                    .into_owned(),
                manifest_contents: contents,
                files,
            })
        }
        Action::CreateWorkspace {
            parent,
            organization,
            name,
        } => {
            if !valid_workspace_slug(&organization) || !valid_workspace_slug(&name) {
                return Err(
                    "Workspace organization and name must use lowercase letters, digits, and internal hyphens."
                        .to_owned(),
                );
            }
            let parent = PathBuf::from(parent);
            let parent = parent
                .canonicalize()
                .map_err(|error| format!("{}: {error}", parent.display()))?;
            if !parent.is_dir() {
                return Err(format!("{} is not a directory", parent.display()));
            }
            let destination = parent.join(&name);
            let root = vapor_client_application::create_basic_content_workspace(
                executable,
                &destination,
                &organization,
                &name,
            )?;
            let files = vapor_client_application::discover_workspace_files(&root)?
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            Ok(AuthoringResultKind::WorkspaceCreated {
                root: root.to_string_lossy().into_owned(),
                files,
            })
        }
    }
}

fn valid_workspace_slug(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value
            .bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn workshop_policy_to_protocol(
    value: vapor_client_application::WorkshopPolicyDraft,
) -> WorkshopMetadataDraft {
    WorkshopMetadataDraft {
        app_id: value.app_id,
        published_file_id: value.published_file_id,
        visibility: value.visibility,
        title: value.title,
        description: value.description,
        tags: value.tags,
        change_note: value.change_note,
    }
}

fn parse_authoring_content_kind(kind: &str) -> Result<vapor_core::ContentKind, String> {
    Ok(match kind {
        "engine" => vapor_core::ContentKind::Engine,
        "game" => vapor_core::ContentKind::Game,
        "packagepack" => vapor_core::ContentKind::Packagepack,
        "enginepack" => vapor_core::ContentKind::Enginepack,
        "gamepack" => vapor_core::ContentKind::Gamepack,
        "modpack" => vapor_core::ContentKind::Modpack,
        "engine-mod" => vapor_core::ContentKind::EngineMod,
        "game-mod" => vapor_core::ContentKind::GameMod,
        "extension-mod" => vapor_core::ContentKind::ExtensionMod,
        "crate" => vapor_core::ContentKind::Crate,
        _ => return Err(format!("Unsupported source content role: {kind}")),
    })
}

fn authoring_result_outcome(
    result: &Result<AuthoringResultKind, String>,
) -> Result<String, String> {
    result
        .as_ref()
        .map(authoring_result_message)
        .map_err(Clone::clone)
}

fn authoring_result_message(result: &AuthoringResultKind) -> String {
    match result {
        AuthoringResultKind::Composition { id, .. } => format!("Loaded composition for {id}."),
        AuthoringResultKind::RegisteredSources { roots, .. } => {
            format!(
                "Loaded {} registered workspace{}.",
                roots.len(),
                if roots.len() == 1 { "" } else { "s" }
            )
        }
        AuthoringResultKind::WorkshopMetadata { id, .. } => {
            format!("Loaded Workshop metadata for {id}.")
        }
        AuthoringResultKind::RuntimeOutputs { id, .. } => {
            format!("Loaded runtime outputs for {id}.")
        }
        AuthoringResultKind::ContentValidated { id, summary } => {
            format!("Validated {id}: {summary}")
        }
        AuthoringResultKind::ContentCreated { manifest, .. } => {
            format!("Created content at {manifest}.")
        }
        AuthoringResultKind::WorkspaceCreated { root, .. } => {
            format!("Created workspace at {root}.")
        }
    }
}

fn invalid_workspace_request(message: &str) -> ServiceError {
    ServiceError {
        code: ServiceErrorCode::InvalidRequest,
        message: message.to_owned(),
        retryable: false,
    }
}

fn workspace_operation_action_id(kind: &WorkspaceOperationKind) -> &'static str {
    match kind {
        WorkspaceOperationKind::DiscoverSuperworkspaceRepositories => {
            "workspace.discover-superworkspace-repositories"
        }
        WorkspaceOperationKind::AcquireSuperworkspaceRepositories(_) => {
            "workspace.acquire-superworkspace-repositories"
        }
        WorkspaceOperationKind::ListFiles => "workspace.list-files",
        WorkspaceOperationKind::OpenFile(_) => "workspace.open-file",
        WorkspaceOperationKind::SaveFile(_) => "workspace.save-file",
        WorkspaceOperationKind::CreateFile(_) => "workspace.create-file",
        WorkspaceOperationKind::Validate => "workspace.validate",
        WorkspaceOperationKind::Remember => "workspace.remember",
        WorkspaceOperationKind::AdoptProject(_) => "workspace.adopt-project",
        WorkspaceOperationKind::UnregisterProject(_) => "workspace.unregister-project",
    }
}

fn workspace_operation_phase(kind: &WorkspaceOperationKind) -> OperationPhase {
    match kind {
        WorkspaceOperationKind::DiscoverSuperworkspaceRepositories => OperationPhase::Resolving,
        WorkspaceOperationKind::AcquireSuperworkspaceRepositories(_) => OperationPhase::Acquiring,
        WorkspaceOperationKind::ListFiles | WorkspaceOperationKind::OpenFile(_) => {
            OperationPhase::Resolving
        }
        WorkspaceOperationKind::SaveFile(_) | WorkspaceOperationKind::CreateFile(_) => {
            OperationPhase::Building
        }
        WorkspaceOperationKind::Validate
        | WorkspaceOperationKind::Remember
        | WorkspaceOperationKind::AdoptProject(_)
        | WorkspaceOperationKind::UnregisterProject(_) => OperationPhase::Resolving,
    }
}

fn execute_workspace_operation(
    executable: &Path,
    root: &Path,
    kind: WorkspaceOperationKind,
    fixture_mode: bool,
) -> Result<WorkspaceOperationResult, String> {
    if fixture_mode && !is_isolated_fixture_workspace(root) {
        return Err(
            "Fixture workspace moved outside the temporary directory before the operation started."
                .to_owned(),
        );
    }
    if fixture_mode
        && !matches!(
            &kind,
            WorkspaceOperationKind::ListFiles
                | WorkspaceOperationKind::OpenFile(_)
                | WorkspaceOperationKind::SaveFile(_)
                | WorkspaceOperationKind::CreateFile(_)
        )
    {
        return Err("This operation is unavailable in fixture mode.".to_owned());
    }
    match kind {
        WorkspaceOperationKind::DiscoverSuperworkspaceRepositories => {
            let installation =
                vapor_core::VaporInstallation::discover().map_err(|error| error.to_string())?;
            let ecosystem = vapor_core::discover_ecosystem_repositories(&installation)
                .map_err(|error| error.to_string())?;
            Ok(WorkspaceOperationResult::SourceRepositories(
                ecosystem
                    .repositories
                    .into_iter()
                    .map(|repository| AvailableSuperworkspaceRepositorySnapshot {
                        id: repository.id,
                        name: repository.name,
                        kind: repository.kind,
                        checkout_path: repository.checkout_path,
                        acquired: false,
                    })
                    .collect(),
            ))
        }
        WorkspaceOperationKind::AcquireSuperworkspaceRepositories(repository_names) => {
            let installation =
                vapor_core::VaporInstallation::discover().map_err(|error| error.to_string())?;
            let repository_names = repository_names
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            let report =
                vapor_core::acquire_ecosystem_repositories(&installation, root, &repository_names)
                    .map_err(|error| error.to_string())?;
            Ok(WorkspaceOperationResult::AcquiredSourceRepositories(
                report
                    .repositories
                    .into_iter()
                    .map(|repository| repository.id)
                    .collect(),
            ))
        }
        WorkspaceOperationKind::ListFiles => {
            vapor_client_application::discover_workspace_files(root)
                .map(WorkspaceOperationResult::Listed)
        }
        WorkspaceOperationKind::OpenFile(path) => {
            vapor_client_application::read_workspace_file_with_fingerprint(root, &path).map(
                |(contents, fingerprint)| {
                    WorkspaceOperationResult::Opened(Box::new(WorkspaceFileOpen {
                        path,
                        contents,
                        fingerprint,
                    }))
                },
            )
        }
        WorkspaceOperationKind::SaveFile(save) => {
            vapor_client_application::write_workspace_file_if_fingerprint(
                root,
                &save.path,
                &save.expected_fingerprint,
                &save.contents,
            )
            .map(|fingerprint| WorkspaceOperationResult::Saved {
                path: save.path,
                fingerprint,
            })
        }
        WorkspaceOperationKind::CreateFile(path) => {
            vapor_client_application::create_new_workspace_file(root, &path)
                .map(WorkspaceOperationResult::Created)
        }
        WorkspaceOperationKind::Validate => {
            vapor_client_application::validate_workspace_source(executable, root)
                .map(|_| WorkspaceOperationResult::Completed)
        }
        WorkspaceOperationKind::Remember => vapor_client_application::remember_workspace(root)
            .map(WorkspaceOperationResult::Remembered),
        WorkspaceOperationKind::AdoptProject(path) => {
            vapor_client_application::adopt_workspace_content_project(executable, root, &path)
                .map(|_| WorkspaceOperationResult::Completed)
        }
        WorkspaceOperationKind::UnregisterProject(id) => {
            vapor_client_application::unregister_workspace_content_project(executable, root, &id)
                .map(|(_, _, identity)| WorkspaceOperationResult::Unregistered(identity))
        }
    }
}

fn is_isolated_fixture_workspace(root: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let Ok(temporary_root) = std::env::temp_dir().canonicalize() else {
        return false;
    };
    root.starts_with(temporary_root)
}

fn workspace_result_outcome(
    result: &Result<WorkspaceOperationResult, String>,
) -> Result<String, String> {
    match result {
        Ok(WorkspaceOperationResult::SourceRepositories(repositories)) => {
            Ok(format!("Found {} source repositories.", repositories.len()))
        }
        Ok(WorkspaceOperationResult::AcquiredSourceRepositories(repositories)) => {
            Ok(format!("Added {} source repositories.", repositories.len()))
        }
        Ok(WorkspaceOperationResult::Listed(files)) => {
            Ok(format!("Found {} workspace files.", files.len()))
        }
        Ok(WorkspaceOperationResult::Opened(opened)) => {
            Ok(format!("Opened {}.", opened.path.display()))
        }
        Ok(WorkspaceOperationResult::Saved { path, .. }) => {
            Ok(format!("Saved {}.", path.display()))
        }
        Ok(WorkspaceOperationResult::Created(path)) => Ok(format!("Created {}.", path.display())),
        Ok(WorkspaceOperationResult::Completed) => Ok("Workspace operation completed.".to_owned()),
        Ok(WorkspaceOperationResult::Remembered(paths)) => Ok(format!(
            "Registered {} workspace{}.",
            paths.len(),
            if paths.len() == 1 { "" } else { "s" }
        )),
        Ok(WorkspaceOperationResult::Unregistered(identity)) => {
            Ok(format!("Unregistered {}.", identity.id))
        }
        Err(error) => Err(error.clone()),
    }
}

fn is_content_manifest_path(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == "Vapor.toml" || name.to_string_lossy().ends_with(".vapor.toml"))
}

fn workshop_result_outcome(
    result: &Result<WorkshopOperationResult, String>,
) -> Result<String, String> {
    match result {
        Ok(WorkshopOperationResult::Search(page)) => Ok(format!(
            "Steam Workshop returned {} items on page {} of {}.",
            page.items.len(),
            page.page,
            page.total_results
        )),
        Ok(WorkshopOperationResult::Subscriptions(items)) => Ok(format!(
            "Loaded {} Steam Workshop subscriptions.",
            items.len()
        )),
        Ok(WorkshopOperationResult::SubscriptionWithAcquisition {
            acquisition: Ok((items, requested, _updates)),
            ..
        }) => {
            if items.len() == *requested {
                Ok(format!(
                    "Subscribed and added {} Workshop items to the Library.",
                    items.len()
                ))
            } else {
                Err(format!(
                    "Subscribed, but only {} of {requested} Workshop items were added to the Library.",
                    items.len()
                ))
            }
        }
        Ok(WorkshopOperationResult::SubscriptionWithAcquisition {
            acquisition: Err(error),
            ..
        }) => Err(format!(
            "Subscription is active, but Vapor Client could not add the item to the Library: {error}"
        )),
        Ok(WorkshopOperationResult::Acquisitions {
            items, requested, ..
        }) => {
            if items.len() == *requested {
                Ok(format!(
                    "Added {} Workshop items to the local cache.",
                    items.len()
                ))
            } else {
                Err(format!(
                    "Downloaded {} of {requested} requested Workshop items.",
                    items.len()
                ))
            }
        }
        Ok(WorkshopOperationResult::Review(preview)) => Ok(format!(
            "Staged {} · {} files · {} bytes for review.",
            preview.id, preview.files, preview.bytes
        )),
        Ok(WorkshopOperationResult::Publication(publication)) => Ok(format!(
            "{} Workshop item {}{}; receipt saved at {}.",
            if publication.created {
                "Created"
            } else {
                "Updated"
            },
            publication.id,
            publication
                .published_file_id
                .as_ref()
                .map(|id| format!(" · PublishedFileId {id}"))
                .unwrap_or_default(),
            publication.receipt.display()
        )),
        Err(error) => Err(error.clone()),
    }
}

fn execute_workshop_operation(
    application_executable: &Path,
    fixture_mode: bool,
    workshop_app_id: Option<u32>,
    fixture_subscriptions: BTreeSet<String>,
    auto_install: AutoInstallContext,
    kind: WorkshopOperationKind,
) -> Result<WorkshopOperationResult, String> {
    if fixture_mode {
        return Ok(fixture_workshop_operation(fixture_subscriptions, kind));
    }
    match kind {
        WorkshopOperationKind::Search { query, page } => {
            let app_id = required_workshop_app_id(workshop_app_id)?;
            vapor_client_application::search_workshop(app_id, &query, page)
                .map(WorkshopOperationResult::Search)
        }
        WorkshopOperationKind::LoadSubscriptions => {
            let app_id = required_workshop_app_id(workshop_app_id)?;
            vapor_client_application::workshop_subscriptions(app_id)
                .map(WorkshopOperationResult::Subscriptions)
        }
        WorkshopOperationKind::ChangeSubscriptions { ids, subscribe } => {
            let app_id = required_workshop_app_id(workshop_app_id)?;
            let change = if subscribe {
                vapor_client_application::subscribe_workshop_items(app_id, &ids)
            } else {
                vapor_client_application::unsubscribe_workshop_items(app_id, &ids)
            };
            change?;
            let subscriptions = vapor_client_application::workshop_subscriptions(app_id).map_err(|error| {
                format!("Subscription change completed, but the updated list could not be loaded: {error}")
            })?;
            if subscribe {
                let requested = ids.len();
                let ids = ids.into_iter().map(|id| id.to_string()).collect::<Vec<_>>();
                let acquisition = download_with_auto_install(
                    application_executable,
                    &ids,
                    requested,
                    &auto_install,
                );
                Ok(WorkshopOperationResult::SubscriptionWithAcquisition {
                    subscriptions,
                    acquisition,
                })
            } else {
                Ok(WorkshopOperationResult::Subscriptions(subscriptions))
            }
        }
        WorkshopOperationKind::Acquire { ids } => {
            let requested = ids.len();
            download_with_auto_install(application_executable, &ids, requested, &auto_install).map(
                |(items, _, updates)| WorkshopOperationResult::Acquisitions {
                    items,
                    requested,
                    updates,
                },
            )
        }
        WorkshopOperationKind::Stage {
            selector,
            workspace_root,
        } => vapor_client_application::stage_workshop_preview(
            application_executable,
            &workspace_root,
            &selector,
        )
        .map(WorkshopOperationResult::Review),
        WorkshopOperationKind::Publish {
            review,
            account,
            change_note,
        } => vapor_client_application::publish_reviewed_workshop_content(
            application_executable,
            &review.workspace_root,
            &review.selector,
            &review.preview,
            &account,
            change_note.as_deref(),
        )
        .map(WorkshopOperationResult::Publication),
    }
}

fn required_workshop_app_id(app_id: Option<u32>) -> Result<u32, String> {
    app_id.ok_or_else(|| {
        "This Vapor Client installation has no configured Workshop application.".to_owned()
    })
}

fn download_with_auto_install(
    application_executable: &Path,
    ids: &[String],
    requested: usize,
    context: &AutoInstallContext,
) -> Result<
    (
        Vec<vapor_client_application::WorkshopAcquisition>,
        usize,
        Vec<AutoInstallCompositionUpdate>,
    ),
    String,
> {
    let items = vapor_client_application::download_workshop_items_with_constraints(
        application_executable,
        ids,
        &context.constraints,
    )?;
    let acquired = items
        .iter()
        .map(|item| item.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut updates = Vec::new();
    let mut updated_compositions = BTreeSet::new();
    for (content_id, targets) in &context.compositions {
        if !acquired.contains(content_id.as_str()) {
            continue;
        }
        for target in targets {
            if !updated_compositions.insert(target.composition.id.clone()) {
                continue;
            }
            let selector = target
                .composition
                .root_package
                .as_ref()
                .map(|root| root.id.as_str())
                .ok_or_else(|| {
                    format!(
                        "Auto-Install could not update composition '{}' because it has no Packagepack.",
                        target.composition.name
                    )
                })?;
            let workspace = target
                .composition
                .workspace
                .as_ref()
                .map(|workspace| workspace.root.as_path());
            let result = vapor_client_application::update_composition(
                application_executable,
                workspace,
                selector,
            );
            updates.push(AutoInstallCompositionUpdate {
                composition_id: target.composition.id.clone(),
                inputs: target.inputs.clone(),
                result,
            });
        }
    }
    Ok((items, requested, updates))
}

fn fixture_workshop_operation(
    mut subscriptions: BTreeSet<String>,
    kind: WorkshopOperationKind,
) -> WorkshopOperationResult {
    let items = vec![
        fixture_workshop_item("1001", "Fixture Starter Pack"),
        fixture_workshop_item("1002", "Fixture Moon Pack"),
    ];
    match kind {
        WorkshopOperationKind::Search { query, page } => {
            let query = query.to_lowercase();
            let items = items
                .into_iter()
                .filter(|item| item.title.to_lowercase().contains(&query))
                .collect::<Vec<_>>();
            WorkshopOperationResult::Search(vapor_client_application::WorkshopPage {
                total_results: items.len() as u32,
                items,
                page,
            })
        }
        WorkshopOperationKind::LoadSubscriptions => WorkshopOperationResult::Subscriptions(
            subscriptions
                .iter()
                .map(|id| fixture_workshop_subscription(id))
                .collect(),
        ),
        WorkshopOperationKind::ChangeSubscriptions { ids, subscribe } => {
            let requested = ids.len();
            for id in &ids {
                if subscribe {
                    subscriptions.insert(id.to_string());
                } else {
                    subscriptions.remove(&id.to_string());
                }
            }
            let subscribed = subscriptions
                .iter()
                .map(|id| fixture_workshop_subscription(id))
                .collect();
            if subscribe {
                let items = ids
                    .into_iter()
                    .map(|id| vapor_client_application::WorkshopAcquisition {
                        id: format!("fixture.content.{id}"),
                        version: Some("1.0.0".to_owned()),
                        kind: "extension-mod".to_owned(),
                        cache_root: PathBuf::from(format!("fixture/library/fixture.content.{id}")),
                        fingerprint: format!("fixture:{id}"),
                    })
                    .collect();
                WorkshopOperationResult::SubscriptionWithAcquisition {
                    subscriptions: subscribed,
                    acquisition: Ok((items, requested, Vec::new())),
                }
            } else {
                WorkshopOperationResult::Subscriptions(subscribed)
            }
        }
        WorkshopOperationKind::Acquire { ids } => {
            let requested = ids.len();
            let items = ids
                .into_iter()
                .map(|id| vapor_client_application::WorkshopAcquisition {
                    id: format!("fixture.content.{id}"),
                    version: Some("1.0.0".to_owned()),
                    kind: "mod".to_owned(),
                    cache_root: PathBuf::from(format!("fixture/cache/fixture.content.{id}")),
                    fingerprint: format!("fixture:{id}"),
                })
                .collect();
            WorkshopOperationResult::Acquisitions {
                items,
                requested,
                updates: Vec::new(),
            }
        }
        WorkshopOperationKind::Stage {
            selector,
            workspace_root: _,
        } => {
            let (id, version) = selector
                .split_once('@')
                .map(|(id, version)| (id.to_owned(), Some(version.to_owned())))
                .unwrap_or((selector, Some("1.0.0".to_owned())));
            WorkshopOperationResult::Review(vapor_client_application::WorkshopPackagePreview {
                id,
                version,
                app_id: Some(2122620),
                published_file_id: None,
                title: Some("Fixture Workshop artifact".to_owned()),
                visibility: Some("public".to_owned()),
                dependencies: Vec::new(),
                root: PathBuf::from("fixture/staged/artifact"),
                target: "fixture".to_owned(),
                fingerprint: "fixture:review".to_owned(),
                files: 1,
                bytes: 128,
                file_paths: vec!["Vapor.toml".to_owned()],
            })
        }
        WorkshopOperationKind::Publish { review, .. } => {
            WorkshopOperationResult::Publication(vapor_client_application::WorkshopPublication {
                id: review.preview.id.clone(),
                published_file_id: Some("1002".to_owned()),
                receipt: PathBuf::from("fixture/receipts/workshop.json"),
                created: review.preview.published_file_id.is_none(),
            })
        }
    }
}

fn fixture_workshop_item(id: &str, title: &str) -> vapor_client_application::WorkshopItem {
    vapor_client_application::WorkshopItem {
        published_file_id: id.to_owned(),
        title: title.to_owned(),
        description: "Deterministic Vapor Client fixture; no Steam connection is used.".to_owned(),
        preview_url: None,
        updated_at_unix: 1_797_000_000,
    }
}

fn fixture_workshop_subscription(id: &str) -> vapor_client_application::WorkshopSubscription {
    let item = match id {
        "1001" => fixture_workshop_item(id, "Fixture Starter Pack"),
        "1002" => fixture_workshop_item(id, "Fixture Moon Pack"),
        _ => fixture_workshop_item(id, &format!("Fixture Workshop item {id}")),
    };
    vapor_client_application::WorkshopSubscription {
        published_file_id: item.published_file_id,
        title: item.title,
        description: item.description,
        preview_url: item.preview_url,
        updated_at_unix: item.updated_at_unix,
        installed: false,
        needs_update: false,
        downloading: false,
        download_progress: None,
    }
}

fn workshop_search_snapshot(
    query: String,
    page: vapor_client_application::WorkshopPage,
    revision: u64,
) -> WorkshopSearchSnapshot {
    WorkshopSearchSnapshot {
        revision,
        query,
        page: page.page,
        total_results: page.total_results,
        items: page.items.into_iter().map(workshop_item_snapshot).collect(),
    }
}

fn workshop_item_snapshot(item: vapor_client_application::WorkshopItem) -> WorkshopItemSnapshot {
    WorkshopItemSnapshot {
        published_file_id: item.published_file_id,
        title: item.title,
        description: item.description,
        preview_url: item.preview_url,
        updated_at_unix: item.updated_at_unix,
    }
}

fn workshop_subscription_snapshot(
    item: &vapor_client_application::WorkshopSubscription,
) -> WorkshopSubscriptionSnapshot {
    WorkshopSubscriptionSnapshot {
        published_file_id: item.published_file_id.clone(),
        title: item.title.clone(),
        description: item.description.clone(),
        preview_url: item.preview_url.clone(),
        updated_at_unix: item.updated_at_unix,
        installed: item.installed,
        needs_update: item.needs_update,
        downloading: item.downloading,
        download_progress: item.download_progress,
    }
}

fn workshop_acquisition_snapshot(
    item: &vapor_client_application::WorkshopAcquisition,
) -> WorkshopAcquisitionSnapshot {
    WorkshopAcquisitionSnapshot {
        id: item.id.clone(),
        version: item.version.clone(),
        kind: item.kind.clone(),
        cache_root: item.cache_root.display().to_string(),
        fingerprint: item.fingerprint.clone(),
    }
}

fn workshop_review_snapshot(review: &WorkshopReviewState) -> WorkshopReviewSnapshot {
    let preview = &review.preview;
    let visible_paths = preview
        .file_paths
        .iter()
        .take(MAX_WORKSHOP_REVIEW_FILE_PATHS)
        .cloned()
        .collect::<Vec<_>>();
    WorkshopReviewSnapshot {
        review_id: review.review_id.clone(),
        selector: review.selector.clone(),
        preview: WorkshopPackagePreviewSnapshot {
            id: preview.id.clone(),
            version: preview.version.clone(),
            app_id: preview.app_id,
            published_file_id: preview.published_file_id.clone(),
            title: preview.title.clone(),
            visibility: preview.visibility.clone(),
            dependencies: preview.dependencies.clone(),
            root: preview.root.display().to_string(),
            target: preview.target.clone(),
            fingerprint: preview.fingerprint.clone(),
            files: preview.files,
            bytes: preview.bytes,
            omitted_file_paths: preview.file_paths.len().saturating_sub(visible_paths.len()),
            file_paths: visible_paths,
        },
    }
}

fn initial_protocol_phase(kind: CompositionOperationKind) -> OperationPhase {
    match kind {
        CompositionOperationKind::Preview => OperationPhase::Resolving,
        CompositionOperationKind::Install
        | CompositionOperationKind::Repair
        | CompositionOperationKind::Update => OperationPhase::Acquiring,
        CompositionOperationKind::Verify => OperationPhase::Verifying,
        CompositionOperationKind::Launch | CompositionOperationKind::RunSource => {
            OperationPhase::Running
        }
    }
}

fn core_operation_phase(kind: CompositionOperationKind) -> application::OperationPhase {
    match kind {
        CompositionOperationKind::Preview => application::OperationPhase::Resolving,
        CompositionOperationKind::Install
        | CompositionOperationKind::Repair
        | CompositionOperationKind::Update => application::OperationPhase::Acquiring,
        CompositionOperationKind::Verify => application::OperationPhase::Verifying,
        CompositionOperationKind::Launch | CompositionOperationKind::RunSource => {
            application::OperationPhase::Running
        }
    }
}

fn protocol_phase(phase: application::OperationPhase) -> OperationPhase {
    match phase {
        application::OperationPhase::Queued => OperationPhase::Queued,
        application::OperationPhase::Resolving => OperationPhase::Resolving,
        application::OperationPhase::Acquiring => OperationPhase::Acquiring,
        application::OperationPhase::Installing => OperationPhase::Installing,
        application::OperationPhase::Verifying => OperationPhase::Verifying,
        application::OperationPhase::Building => OperationPhase::Building,
        application::OperationPhase::Running => OperationPhase::Running,
        application::OperationPhase::Staging => OperationPhase::Staging,
        application::OperationPhase::Publishing => OperationPhase::Publishing,
        application::OperationPhase::Finalizing => OperationPhase::Finalizing,
    }
}

fn protocol_status(status: application::OperationStatus) -> (OperationStatus, Option<String>) {
    match status {
        application::OperationStatus::Running => (OperationStatus::Running, None),
        application::OperationStatus::Succeeded { summary } => {
            (OperationStatus::Succeeded, Some(summary))
        }
        application::OperationStatus::Failed { message } => (
            OperationStatus::Failed {
                message: message.clone(),
            },
            Some(message),
        ),
        application::OperationStatus::Cancelled => (OperationStatus::Cancelled, None),
    }
}

fn composition_result_outcome(result: &CompositionOperationResult) -> Result<String, String> {
    match result {
        CompositionOperationResult::Preview(Ok(preview)) => Ok(format!(
            "Resolved {} composition items{}",
            preview.nodes.len(),
            if preview.blockers.is_empty() {
                String::new()
            } else {
                format!(" with {} blockers", preview.blockers.len())
            }
        )),
        CompositionOperationResult::Install(Ok(composition))
        | CompositionOperationResult::Repair(Ok(composition))
        | CompositionOperationResult::Update(Ok(composition)) => Ok(format!(
            "Composition installed with {} content items",
            composition.graph.nodes.len()
        )),
        CompositionOperationResult::Verify(Ok(check)) if check.ok => {
            Ok(format!("Verified {} content items", check.detail.len()))
        }
        CompositionOperationResult::Verify(Ok(check)) => Err(check.detail.join("; ")),
        CompositionOperationResult::Launch(Ok(result))
        | CompositionOperationResult::RunSource(Ok(result)) => {
            if result.composition_graph_matches {
                result
                    .launch_error
                    .clone()
                    .map_or_else(|| Ok("Composition launched".to_owned()), Err)
            } else {
                Err(result.launch_error.clone().unwrap_or_else(|| {
                    "The installed composition differs from the composition.".to_owned()
                }))
            }
        }
        CompositionOperationResult::Preview(Err(error))
        | CompositionOperationResult::Install(Err(error))
        | CompositionOperationResult::Verify(Err(error))
        | CompositionOperationResult::Repair(Err(error))
        | CompositionOperationResult::Update(Err(error))
        | CompositionOperationResult::Launch(Err(error))
        | CompositionOperationResult::RunSource(Err(error)) => Err(error.clone()),
    }
}

fn execute_composition_operation(
    bootstrap: &ClientHostConfig,
    composition: &Composition,
    kind: CompositionOperationKind,
    selector_override: Option<&str>,
) -> CompositionOperationResult {
    let selector = selector_override
        .map(ToOwned::to_owned)
        .or_else(|| {
            composition.root_package.as_ref().map(|root| {
                root.version.as_deref().map_or_else(
                    || root.id.clone(),
                    |version| format!("{}@{version}", root.id),
                )
            })
        })
        .expect("validated before spawn");
    let workspace = composition
        .workspace
        .as_ref()
        .map(|workspace| workspace.root.as_path());
    if bootstrap.fixture_mode {
        return fixture_composition_result(composition, selector, kind);
    }
    let executable = &bootstrap.application_executable;
    let graph_selectors = composition
        .resolved_graph
        .as_ref()
        .map(|graph| {
            graph
                .nodes
                .iter()
                .map(|node| {
                    node.content.version.as_deref().map_or_else(
                        || node.content.id.clone(),
                        |version| format!("{}@{version}", node.content.id),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![selector.clone()]);
    match kind {
        CompositionOperationKind::Preview => CompositionOperationResult::Preview(
            vapor_client_application::preview_composition(executable, workspace, &selector),
        ),
        CompositionOperationKind::Install => CompositionOperationResult::Install(
            vapor_client_application::install_composition(executable, workspace, &selector),
        ),
        CompositionOperationKind::Verify => CompositionOperationResult::Verify(
            vapor_client_application::verify_installation(executable, &selector, &graph_selectors),
        ),
        CompositionOperationKind::Repair => {
            CompositionOperationResult::Repair(vapor_client_application::repair_composition(
                executable,
                workspace,
                &selector,
                &graph_selectors,
            ))
        }
        CompositionOperationKind::Update => CompositionOperationResult::Update(
            vapor_client_application::update_composition(executable, workspace, &selector),
        ),
        CompositionOperationKind::Launch => {
            CompositionOperationResult::Launch(vapor_client_application::run_installed_composition(
                executable,
                workspace,
                &selector,
                composition.resolved_graph.as_ref(),
                &composition.launch.arguments,
            ))
        }
        CompositionOperationKind::RunSource => {
            let Some(workspace) = workspace else {
                return CompositionOperationResult::RunSource(Err(
                    "Open the composition's workspace before running its source composition."
                        .to_owned(),
                ));
            };
            CompositionOperationResult::RunSource(vapor_client_application::run_source_composition(
                executable,
                workspace,
                &selector,
                &composition.launch.arguments,
            ))
        }
    }
}

fn fixture_composition_result(
    composition: &Composition,
    selector: String,
    kind: CompositionOperationKind,
) -> CompositionOperationResult {
    use vapor_client_application::{
        CompositionPreview, CompositionPreviewNode, CompositionPreviewSource, InstallationCheck,
        InstalledArtifact, InstalledComposition,
    };
    let root = composition.root_package.clone().unwrap_or_else(|| {
        let (id, version) = selector.split_once('@').map_or_else(
            || (selector.as_str(), None),
            |(id, version)| (id, Some(version)),
        );
        ContentReference {
            id: id.to_owned(),
            version: version.map(ToOwned::to_owned),
        }
    });
    let preview = CompositionPreview {
        selector,
        nodes: vec![CompositionPreviewNode {
            id: root.id.clone(),
            version: root.version.clone(),
            kind: "packagepack".to_owned(),
            published_file_id: None,
            source: if matches!(
                &composition.installation,
                InstallationState::Installed { .. }
            ) {
                CompositionPreviewSource::Installed
            } else {
                CompositionPreviewSource::Workspace
            },
            already_installed: matches!(
                &composition.installation,
                InstallationState::Installed { .. }
            ),
            dependencies: Vec::new(),
        }],
        blockers: Vec::new(),
        warnings: vec![
            "Fixture mode: no content provider, Steam, or game process was used.".into(),
        ],
    };
    let graph = composition.resolved_graph.clone().unwrap_or_else(|| {
        let engine = ContentReference {
            id: "fixture.engine".to_owned(),
            version: Some("1.0.0".to_owned()),
        };
        vapor_client_protocol::ResolvedCompositionGraph {
            root: root.clone(),
            effective_engine: engine.clone(),
            effective_game: None,
            nodes: vec![
                vapor_client_protocol::ResolvedNodeIdentity {
                    content: root.clone(),
                    kind: "packagepack".to_owned(),
                    fingerprint: Some("fixture-root".to_owned()),
                    source: Some("fixture".to_owned()),
                    dependencies: vec![vapor_client_protocol::ResolvedDependencyIdentity {
                        binding: "engine".to_owned(),
                        content: engine.clone(),
                        version_requirement: None,
                    }],
                },
                vapor_client_protocol::ResolvedNodeIdentity {
                    content: engine,
                    kind: "engine".to_owned(),
                    fingerprint: Some("fixture-engine".to_owned()),
                    source: Some("fixture".to_owned()),
                    dependencies: Vec::new(),
                },
            ],
        }
    });
    let composition = InstalledComposition {
        artifacts: graph
            .nodes
            .iter()
            .map(|node| InstalledArtifact {
                id: node.content.id.clone(),
                version: node.content.version.clone(),
                root: PathBuf::from(format!("/fixture/installed/{}", node.content.id)),
                fingerprint: node.fingerprint.clone().unwrap_or_default(),
            })
            .collect(),
        graph: graph.clone(),
    };
    match kind {
        CompositionOperationKind::Preview => CompositionOperationResult::Preview(Ok(preview)),
        CompositionOperationKind::Install => CompositionOperationResult::Install(Ok(composition)),
        CompositionOperationKind::Verify => {
            CompositionOperationResult::Verify(Ok(InstallationCheck {
                ok: true,
                detail: vec!["Fixture installation verified".into()],
                artifacts: composition.artifacts.clone(),
                graph: Some(graph),
            }))
        }
        CompositionOperationKind::Repair => CompositionOperationResult::Repair(Ok(composition)),
        CompositionOperationKind::Update => CompositionOperationResult::Update(Ok(composition)),
        CompositionOperationKind::Launch => {
            CompositionOperationResult::Launch(Ok(vapor_client_application::CompositionRunResult {
                composition,
                composition_graph_matches: true,
                launch_error: None,
            }))
        }
        CompositionOperationKind::RunSource => CompositionOperationResult::RunSource(Ok(
            vapor_client_application::CompositionRunResult {
                composition,
                composition_graph_matches: true,
                launch_error: None,
            },
        )),
    }
}

fn apply_operation_result_to_composition(
    preview: &mut Option<vapor_client_protocol::CompositionPreviewSnapshot>,
    compositions: &mut [Composition],
    composition_id: &str,
    kind: CompositionOperationKind,
    result: CompositionOperationResult,
) -> Option<String> {
    use vapor_client_protocol::InstallationState;
    let Some(composition) = compositions
        .iter_mut()
        .find(|composition| composition.id == composition_id)
    else {
        return Some("The composition was removed while its operation was running.".to_owned());
    };
    match result {
        CompositionOperationResult::Preview(Ok(value)) => {
            *preview = Some(preview_snapshot(composition_id, value));
        }
        CompositionOperationResult::Preview(Err(error)) => return Some(error),
        CompositionOperationResult::Install(Ok(installed))
        | CompositionOperationResult::Repair(Ok(installed))
        | CompositionOperationResult::Update(Ok(installed)) => {
            let graph = installed.graph;
            if matches!(kind, CompositionOperationKind::Repair)
                && composition.resolved_graph.as_ref().is_some_and(|saved| {
                    !vapor_client_application::resolved_graph_matches(saved, &graph)
                })
            {
                composition.installation = InstallationState::OutOfDate {
                    detail: "Repair restored valid content, but its graph differs from this composition's saved setup.".into(),
                    actual_graph: graph,
                    artifacts: installed_records(installed.artifacts),
                    verified: false,
                };
            } else {
                composition.resolved_graph = Some(graph.clone());
                composition.installation = installed_state(graph, installed.artifacts);
            }
        }
        CompositionOperationResult::Verify(Ok(check)) => {
            let artifacts = protocol_artifacts(check.artifacts);
            if !check.ok {
                composition.installation = InstallationState::NeedsRepair {
                    detail: check.detail.join("; "),
                    artifacts,
                };
            } else if let Some(graph) = check.graph {
                if composition.resolved_graph.as_ref().is_some_and(|saved| {
                    !vapor_client_application::resolved_graph_matches(saved, &graph)
                }) {
                    composition.installation = InstallationState::OutOfDate {
                        detail:
                            "Verified installed content differs from this composition's saved setup."
                                .into(),
                        actual_graph: graph,
                        artifacts,
                        verified: true,
                    };
                } else {
                    composition.resolved_graph = Some(graph.clone());
                    composition.installation = InstallationState::Installed {
                        artifacts,
                        graph_fingerprint: vapor_client_application::fingerprint_resolved_graph(
                            &graph,
                        ),
                    };
                }
            } else {
                composition.installation = InstallationState::NeedsRepair {
                    detail: "Verification completed without a resolved installed graph.".into(),
                    artifacts,
                };
            }
        }
        CompositionOperationResult::Verify(Err(error)) => {
            let artifacts = installation_artifacts(&composition.installation);
            composition.installation = InstallationState::NeedsRepair {
                detail: error.clone(),
                artifacts,
            };
            return Some(error);
        }
        CompositionOperationResult::Launch(Ok(result))
        | CompositionOperationResult::RunSource(Ok(result)) => {
            let graph = result.composition.graph;
            let artifacts = protocol_artifacts(result.composition.artifacts);
            if result.composition_graph_matches {
                composition.resolved_graph = Some(graph.clone());
                composition.installation = InstallationState::Installed {
                    artifacts,
                    graph_fingerprint: vapor_client_application::fingerprint_resolved_graph(&graph),
                };
            } else {
                composition.installation = InstallationState::OutOfDate {
                    detail: result.launch_error.clone().unwrap_or_else(|| {
                        "The installed composition differs from this composition.".into()
                    }),
                    actual_graph: graph,
                    artifacts,
                    verified: false,
                };
            }
            if result.launch_error.is_some() || !result.composition_graph_matches {
                return result.launch_error.or_else(|| {
                    Some("The installed composition differs from this composition.".into())
                });
            }
        }
        CompositionOperationResult::Install(Err(error))
        | CompositionOperationResult::Repair(Err(error))
        | CompositionOperationResult::Update(Err(error)) => {
            composition.installation = InstallationState::Partial {
                detail: error.clone(),
            };
            return Some(error);
        }
        CompositionOperationResult::Launch(Err(error))
        | CompositionOperationResult::RunSource(Err(error)) => return Some(error),
    }
    None
}

fn preview_snapshot(
    composition_id: &str,
    preview: vapor_client_application::CompositionPreview,
) -> vapor_client_protocol::CompositionPreviewSnapshot {
    vapor_client_protocol::CompositionPreviewSnapshot {
        composition_id: composition_id.to_owned(),
        selector: preview.selector,
        nodes: preview.nodes,
        blockers: preview.blockers,
        warnings: preview.warnings,
    }
}

fn installed_state(
    graph: vapor_client_protocol::ResolvedCompositionGraph,
    artifacts: Vec<vapor_client_application::InstalledArtifact>,
) -> vapor_client_protocol::InstallationState {
    vapor_client_protocol::InstallationState::Installed {
        graph_fingerprint: vapor_client_application::fingerprint_resolved_graph(&graph),
        artifacts: installed_records(artifacts),
    }
}

fn installed_records(
    artifacts: Vec<vapor_client_application::InstalledArtifact>,
) -> Vec<vapor_client_protocol::InstalledArtifactRecord> {
    artifacts
        .into_iter()
        .map(|artifact| vapor_client_protocol::InstalledArtifactRecord {
            id: artifact.id,
            version: artifact.version,
            root: artifact.root,
            fingerprint: artifact.fingerprint,
        })
        .collect()
}

fn protocol_artifacts(
    artifacts: Vec<vapor_client_application::InstalledArtifact>,
) -> Vec<vapor_client_protocol::InstalledArtifactRecord> {
    installed_records(artifacts)
}

fn installation_artifacts(
    state: &InstallationState,
) -> Vec<vapor_client_protocol::InstalledArtifactRecord> {
    match state {
        InstallationState::Installed { artifacts, .. }
        | InstallationState::NeedsRepair { artifacts, .. }
        | InstallationState::OutOfDate { artifacts, .. } => artifacts.clone(),
        _ => Vec::new(),
    }
}

fn source_project(project: SourceContent) -> WorkspaceSourceProject {
    WorkspaceSourceProject {
        id: project.id,
        version: project.version,
        kind: project.kind,
        root: project.root.display().to_string(),
        manifest: project.manifest.display().to_string(),
        binaries: project.binaries,
        libraries: project.libraries,
        dependencies: project.dependencies,
        workshop: WorkspaceWorkshopPolicy {
            app_id: project.workshop.app_id,
            published_file_id: project.workshop.published_file_id,
            visibility: project.workshop.visibility,
            title: project.workshop.title,
            description: project.workshop.description,
            tags: project.workshop.tags,
            change_note: project.workshop.change_note,
        },
    }
}

fn fixture_workspace_content(root: &Path) -> WorkspaceContentSnapshot {
    WorkspaceContentSnapshot {
        source_projects: vec![WorkspaceSourceProject {
            id: "fixture.game".to_owned(),
            version: Some("1.0.0".to_owned()),
            kind: "game".to_owned(),
            root: root.join("game").display().to_string(),
            manifest: root.join("game/Vapor.toml").display().to_string(),
            binaries: vec!["fixture-game".to_owned()],
            libraries: Vec::new(),
            dependencies: vec!["required · fixture.engine".to_owned()],
            workshop: WorkspaceWorkshopPolicy::default(),
        }],
        source_projects_loaded: true,
        source_projects_error: None,
        unregistered_projects: vec![WorkspaceProjectCandidate {
            path: root.join("unregistered-addon").display().to_string(),
            id: "fixture.addon".to_owned(),
            kind: "game-mod".to_owned(),
        }],
        unregistered_diagnostics: vec!["fixture diagnostic".to_owned()],
        unregistered_projects_loaded: true,
        unregistered_projects_error: None,
        workspace_validation_error: None,
    }
}

fn catalog_entry(entry: CatalogContent) -> CatalogEntry {
    let role = catalog_item_role(&entry.kind);
    CatalogEntry {
        id: entry.id,
        name: entry.name,
        version: entry.version,
        kind: entry.kind,
        published_file_id: entry.published_file_id,
        dependencies: entry.dependencies,
        conflicts: entry
            .conflicts
            .into_iter()
            .map(|conflict| CatalogConflict {
                field: conflict.field,
                first_value: conflict.first_value,
                first_location: conflict.first_location.display().to_string(),
                conflicting_value: conflict.conflicting_value,
                conflicting_location: conflict.conflicting_location.display().to_string(),
            })
            .collect(),
        locations: entry
            .locations
            .into_iter()
            .map(|location| CatalogLocation {
                kind: match location.kind {
                    vapor_client_application::CatalogLocationKind::Workspace => {
                        CatalogLocationKind::Workspace
                    }
                    vapor_client_application::CatalogLocationKind::Cache => {
                        CatalogLocationKind::Cache
                    }
                    vapor_client_application::CatalogLocationKind::Installed => {
                        CatalogLocationKind::Installed
                    }
                },
                root: location.root.display().to_string(),
                enabled: location.enabled,
                fingerprint: location.fingerprint,
                install_source: location.install_source,
            })
            .collect(),
        role,
        actions: Vec::new(),
    }
}

fn catalog_projection(
    entry: &CatalogEntry,
    state: &ClientPersistentState,
    catalog: &[CatalogEntry],
) -> CatalogEntry {
    let mut projected = entry.clone();
    projected.role = catalog_item_role(&projected.kind);
    projected.actions = if !projected.conflicts.is_empty() {
        Vec::new()
    } else {
        match projected.role {
            CatalogItemRole::PlayableComposition => {
                let matching = state
                    .compositions
                    .compositions
                    .iter()
                    .filter(|composition| {
                        composition.root_package.as_ref().is_some_and(|root| {
                            root.id == projected.id && root.version == projected.version
                        })
                    })
                    .collect::<Vec<_>>();
                if matching.is_empty() {
                    vec![CatalogItemAction::CreateComposition]
                } else if matching
                    .iter()
                    .any(|composition| state.active_composition_id == composition.id)
                {
                    let active = matching
                        .iter()
                        .find(|composition| state.active_composition_id == composition.id);
                    if active.is_some_and(|composition| {
                        matches!(
                            &composition.installation,
                            InstallationState::Installed { .. }
                        )
                    }) {
                        vec![CatalogItemAction::Play]
                    } else if active.is_some_and(|composition| {
                        matches!(
                            &composition.installation,
                            InstallationState::OutOfDate { .. }
                                | InstallationState::Partial { .. }
                                | InstallationState::NeedsRepair { .. }
                        )
                    }) {
                        vec![CatalogItemAction::Review]
                    } else {
                        vec![CatalogItemAction::Install]
                    }
                } else {
                    vec![CatalogItemAction::SelectComposition]
                }
            }
            CatalogItemRole::RuntimeContent => {
                let addable = projected.locations.iter().any(|location| {
                    location.kind == CatalogLocationKind::Workspace
                        && state.compositions.compositions.iter().any(|composition| {
                            let Some(root) = composition.root_package.as_ref() else {
                                return false;
                            };
                            let Some(workspace) = composition.workspace.as_ref() else {
                                return false;
                            };
                            if workspace.root != location.root {
                                return false;
                            }
                            let Some(root_entry) = catalog.iter().find(|candidate| {
                                candidate.id == root.id && candidate.version == root.version
                            }) else {
                                return false;
                            };
                            let Ok(parent_kind) =
                                root_entry.kind.parse::<vapor_core::ContentKind>()
                            else {
                                return false;
                            };
                            let Ok(child_kind) = projected.kind.parse::<vapor_core::ContentKind>()
                            else {
                                return false;
                            };
                            vapor_client_application::source_dependency_allowed(
                                parent_kind,
                                child_kind,
                            )
                        })
                });
                let mut actions = Vec::new();
                if addable {
                    actions.push(CatalogItemAction::AddToComposition);
                }
                if projected
                    .locations
                    .iter()
                    .any(|location| location.kind == CatalogLocationKind::Workspace)
                {
                    actions.push(CatalogItemAction::OpenForAuthoring);
                }
                actions
            }
            CatalogItemRole::CreatorCrate => {
                if projected
                    .locations
                    .iter()
                    .any(|location| location.kind == CatalogLocationKind::Workspace)
                {
                    vec![CatalogItemAction::OpenForAuthoring]
                } else {
                    Vec::new()
                }
            }
            CatalogItemRole::Other => Vec::new(),
        }
    };
    projected
}

fn catalog_item_role(kind: &str) -> CatalogItemRole {
    match kind.parse::<vapor_core::ContentKind>() {
        Ok(vapor_core::ContentKind::Packagepack) => CatalogItemRole::PlayableComposition,
        Ok(vapor_core::ContentKind::Crate) => CatalogItemRole::CreatorCrate,
        Ok(_) => CatalogItemRole::RuntimeContent,
        Err(_) => CatalogItemRole::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use vapor_client_protocol::{ClientAction, ClientRequest, RequestId};

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
    fn workshop_stage_requires_a_current_workspace_identity() {
        let mut session = session();
        let response = session.dispatch(request(
            None,
            ClientAction::Workshop(WorkshopAction::Stage {
                selector: "fixture.game@1.0.0".into(),
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
