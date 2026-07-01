//! Foundational Vapor data types, validation primitives, capability/composition model,
//! Steam-facing protocol types, and shared contracts.

#![forbid(unsafe_code)]

pub mod composition;
pub mod content;

pub use composition::ChildContentRef;
pub use content::{allowed_pack_children, ContentSource, ContentType};
