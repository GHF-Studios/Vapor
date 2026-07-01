//! Shared composition references used before full graph resolution exists.

use crate::ContentType;

/// A child content reference inside a pack composition operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildContentRef {
    /// The child's content kind. Validation later checks compatibility with the parent pack.
    pub content_type: ContentType,
    /// The child's Vapor identity.
    pub content_id: String,
}
