//! Foundational Vapor data types, validation primitives, capability/composition model,
//! Steam-facing protocol types, and shared contracts.

#![forbid(unsafe_code)]

pub mod composition;
pub mod content;
pub mod manifest;
pub mod toolchain;

pub use composition::ChildContentRef;
pub use content::{ContentSource, ContentType, allowed_pack_children};
pub use manifest::{
    CANONICAL_WORKSPACE_MANIFEST_TEXT, ManifestError, ToolchainIntent, VaporManifest,
    WorkspaceIdentity, canonical_manifest, canonical_toolchain, parse_manifest,
};
pub use toolchain::{
    CanonicalToolchain, REQUIRED_TOOLCHAIN_COMPONENTS, SUPPORTED_HOST_TRIPLES,
    SUPPORTED_TARGET_TRIPLES, ToolchainComponent, current_host_triple,
};
