//! Core semantic model and orchestration primitives for Vapor.
//!
//! This crate is being rebuilt from the current Vapor domain model rather than
//! preserving compatibility with the legacy implementation.

#![forbid(unsafe_code)]

pub mod content;
pub mod identity;
pub mod manifest;
pub mod resolution;

pub use content::{ContentKind, DependencySpec};
pub use identity::{ParseVaporIdError, ResolvedContentId, VaporId};
pub use manifest::{ContentHeader, ContentManifest, ManifestError, parse_content_manifest};
pub use resolution::{ResolvedComposition, ResolvedContentNode};
