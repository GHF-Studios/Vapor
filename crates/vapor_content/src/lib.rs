//! Shared semantic vocabulary and authored schema for Vapor content.

pub mod authored_manifest;
mod catalog;
mod content;
mod identity;
mod resolution;

pub use catalog::{ContentCandidate, ContentCandidates, DuplicateCandidate};
pub use content::{
    ContentKind, ContentType, DependencySpec, allowed_pack_children, dependency_choice_group,
    dependency_choice_group_for_relationship, dependency_kind_allowed, dependency_relationship,
    required_dependency_choice_groups,
};
pub use identity::{ContentReference, ContentVersionId, ParseVaporIdError, VaporId};
pub use resolution::{
    ResolutionError, ResolutionError as ContentResolutionError, ResolutionReport,
    ResolvedComposition, ResolvedContentGraph, ResolvedContentNode, resolve_composition,
    resolve_exact, resolve_from, resolve_latest, resolve_report_from, validate_content_graph,
    validate_content_relationships,
};
