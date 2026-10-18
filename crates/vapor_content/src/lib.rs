//! Shared semantic vocabulary and authored schema for Vapor content.

pub mod authored_manifest;
mod catalog;
mod content;
mod identity;
mod resolution;

pub use content::{
    ContentKind, ContentType, DependencySpec, allowed_pack_children, dependency_kind_allowed,
};
pub use catalog::{ContentCandidate, ContentCandidates, DuplicateCandidate};
pub use identity::{ContentReference, ContentVersionId, ParseVaporIdError, VaporId};
pub use resolution::{
    ResolvedComposition, ResolvedContentGraph, ResolvedContentNode, ResolutionError,
    ResolutionError as ContentResolutionError, ResolutionReport, resolve_composition, resolve_exact,
    resolve_from, resolve_latest, resolve_report_from, validate_content_graph,
    validate_content_relationships,
};
