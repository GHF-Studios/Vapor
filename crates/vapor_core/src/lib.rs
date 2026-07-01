//! Foundational Vapor data types, validation primitives, capability/composition model,
//! Steam-facing protocol types, and shared contracts.

#![forbid(unsafe_code)]

pub mod composition;
pub mod content;
pub mod manifest;
pub mod toolchain;

pub use composition::ChildContentRef;
pub use content::{allowed_pack_children, ContentSource, ContentType};
pub use manifest::{
    canonical_manifest, canonical_toolchain, parse_manifest, ManifestError, ToolchainIntent,
    VaporIdentity, VaporManifest, VaporManifestKind, CANONICAL_VAPOR_MANIFEST_TEXT,
};
pub use toolchain::{
    current_host_triple, CanonicalToolchain, ToolchainComponent, REQUIRED_TOOLCHAIN_COMPONENTS,
    SUPPORTED_HOST_TRIPLES, SUPPORTED_TARGET_TRIPLES,
};
