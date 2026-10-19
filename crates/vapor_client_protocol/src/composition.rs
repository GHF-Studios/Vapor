//! Transport exports for Vapor Client composition domain records.

use serde::{Deserialize, Serialize};
pub use vapor_client_domain::{
    COMPOSITION_SCHEMA_VERSION, Composition, CompositionDocument, ContentReference,
    InstallationState, InstalledArtifactRecord, LaunchSettings, ResolvedCompositionGraph,
    ResolvedDependencyIdentity, ResolvedNodeIdentity, WorkspaceAssociation,
};

/// Values the GUI's former eframe store may offer during the first service startup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyStorageSeed {
    pub profiles: Option<String>,
    pub active_profile_id: Option<String>,
    pub active_profile_index: Option<String>,
    pub workspace_root: Option<String>,
    pub editor_drafts: Option<String>,
    pub operation_history: Option<String>,
    pub recovery_backup: Option<String>,
}
