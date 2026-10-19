//! Vapor Client application operations and source-workspace adapters.
//!
//! Content workflows consume Vapor-owned domain and provider operations. The
//! Vapor Client service owns workflow state and orchestration around them.

mod catalog;
mod composition;
mod content;
mod operation;
mod publishing;
mod workspace;
mod workspace_files;

pub use catalog::{
    CatalogConflict, CatalogContent, CatalogLocation, CatalogLocationKind, discover_content_catalog,
};
pub use composition::{
    CURRENT_SCHEMA_VERSION, Composition, CompositionDocument, CompositionRecovery,
    ContentReference, InstallationState, InstalledArtifactRecord, LaunchSettings,
    ResolvedCompositionGraph, ResolvedDependencyIdentity, ResolvedNodeIdentity,
    WorkspaceAssociation, add_composition, decode_compositions, duplicate_composition,
    fingerprint_resolved_graph, remove_composition, rename_composition, resolved_graph_matches,
};
pub use content::{
    CompositionPreview, CompositionPreviewDependency, CompositionPreviewNode,
    CompositionPreviewSource, CompositionRunResult, InstallationCheck, InstalledArtifact,
    InstalledComposition, WorkshopAcquisition, collect_library_item, default_packagepack_selector,
    download_workshop_item, download_workshop_items, download_workshop_items_with_constraints,
    install_composition, preview_composition, repair_composition, run_installed_composition,
    run_source_composition, update_composition, verify_installation,
};
pub use operation::{
    CoreOperationEvent, CoreOperationRequest, OperationContext, OperationEvent, OperationEventSink,
    OperationId, OperationKind, OperationPhase, OperationRequest, OperationService,
    OperationStatus, Progress,
};
pub use publishing::{
    WorkshopPackagePreview, WorkshopPublication, publish_reviewed_workshop_content,
    stage_workshop_preview,
};
pub use vapor_steam::{
    MAX_SUBSCRIPTION_BATCH_SIZE as MAX_WORKSHOP_SUBSCRIPTION_BATCH, search_workshop,
    subscribe_item as subscribe_workshop_item, subscribe_items as subscribe_workshop_items,
    subscribed_items as workshop_subscriptions, unsubscribe_item as unsubscribe_workshop_item,
    unsubscribe_items as unsubscribe_workshop_items,
};
pub use vapor_steam::{WorkshopItem, WorkshopPage, WorkshopSubscription};

pub use workspace::{
    ContentDependencyDraft, SourceComposition, SourceCompositionLink, SourceContent,
    SourceRuntimeOutputs, SourceWorkshopMetadata, SourceWorkshopPolicy,
    UnregisteredProjectCandidate, UnregisteredProjectScan, WorkshopPolicyDraft,
    adopt_workspace_content_project, create_basic_content_workspace, create_content_workspace,
    create_content_workspace_with_dependency_policies, create_content_workspace_with_workshop,
    discover_source_content, fork_acquired_content, load_source_composition,
    load_source_runtime_outputs, load_source_workshop_metadata, remember_workspace,
    save_source_composition, save_source_runtime_outputs, save_source_workshop_metadata,
    scan_unregistered_content_projects, source_dependency_allowed, source_dependency_choice_group,
    source_dependency_relationship, source_project_kind, source_required_dependency_choice_groups,
    unregister_workspace_content_project, validate_composition_source, validate_workspace,
    validate_workspace_source,
};
pub use workspace_files::{
    create_new_workspace_file, discover_workspace_files, read_workspace_file,
    read_workspace_file_with_fingerprint, workspace_file_fingerprint, write_workspace_file,
    write_workspace_file_if_fingerprint,
};
