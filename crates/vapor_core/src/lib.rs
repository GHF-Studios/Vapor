//! Core semantic model and orchestration primitives for Vapor.

#![forbid(unsafe_code)]

pub mod cargo;
pub mod content;
pub mod identity;
pub mod local;
pub mod manifest;
pub mod resolution;
pub mod toolchain;

pub use cargo::{
    CargoRealization, CargoRealizationError, build_cargo_realization,
    generate_local_cargo_realization, run_cargo_realization,
};

pub use content::{ContentKind, DependencySpec};

pub use identity::{ContentVersionId, ParseVaporIdError, VaporId};

pub use local::{
    CONTENT_MANIFEST_FILE_NAME, LocalCatalog, LocalContent, LocalDiscoveryError,
    discover_local_content,
};

pub use manifest::{ContentHeader, ContentManifest, ManifestError, parse_content_manifest};

pub use resolution::{
    ResolutionError, ResolvedComposition, ResolvedContentNode, resolve_local_packagepack,
};

pub use toolchain::{
    ManagedToolchain, ToolchainError, ToolchainPin, VAPOR_HOME_ENV, WORKSPACE_MANIFEST_FILE_NAME,
};
