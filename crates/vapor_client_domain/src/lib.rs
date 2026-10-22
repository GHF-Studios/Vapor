//! Vapor Client's host-independent application domain.
//!
//! This crate owns user policy and lifecycle meaning that sits above authored
//! content semantics and below frontends, transport, and provider effects.

mod actions;
mod catalog;
mod composition;
mod developer;
mod install_plan;
mod library;
mod state;

pub use actions::{
    AuthoringAction, CatalogAction, ClientAction, CompositionDependency, DeveloperAction,
    LanguageServerAction, PublishingAction, SessionAction, WorkshopAction, WorkshopMetadataDraft,
    WorkspaceAction, WorkspaceProjectSelection, WorkspaceToolOperation,
};
pub use catalog::{
    CatalogConflict, CatalogContent, CatalogDependency, CatalogLocation, CatalogLocationKind,
    merge_catalog_entries,
};
pub use composition::{
    COMPOSITION_SCHEMA_VERSION, CURRENT_SCHEMA_VERSION, Composition, CompositionAction,
    CompositionDocument, InstallationState, InstalledArtifactRecord, LaunchSettings,
    ResolvedCompositionGraph, ResolvedDependencyIdentity, ResolvedNodeIdentity,
    WorkspaceAssociation, add_composition, apply_composition_action, duplicate_composition,
    fingerprint_resolved_graph, remove_composition, rename_composition, resolved_graph_matches,
};
pub use developer::{ProfileAnalysisOptions, ProfileTraceSort};
pub use install_plan::{
    InstallCandidateAvailability, InstallContentProvider, InstallPlan, InstallPlanDependency,
    InstallPlanNode, InstallPlanSource, SelectedInstallCandidate, resolve_install_plan,
};
pub use library::{
    LibraryAction, LibraryItemPolicy, MissingLibraryConstraint, add_auto_install_owner,
    can_auto_install_library_item, can_collect_library_item, dependent_composition_id,
    library_auto_install_enabled, library_constraints, remove_auto_install_owner,
    select_library_version, select_library_version_for_compositions, set_library_auto_install,
};
pub use state::{CLIENT_STATE_SCHEMA_VERSION, ClientStateDocument};
pub use vapor_content::{
    ContentKind, ContentReference, dependency_choice_group,
    dependency_choice_group_for_relationship, dependency_kind_allowed, dependency_relationship,
    required_dependency_choice_groups,
};
