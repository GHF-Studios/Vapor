use crate::{CompositionDocument, LibraryItemPolicy};
use serde::{Deserialize, Serialize};

pub const CLIENT_STATE_SCHEMA_VERSION: u32 = 2;

/// Durable user-owned Vapor Client state shared by every frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientStateDocument {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default, alias = "profile_revision")]
    pub composition_revision: u64,
    #[serde(default, alias = "profiles")]
    pub compositions: CompositionDocument,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub library_policies: Vec<LibraryItemPolicy>,
    #[serde(default = "default_composition_id", alias = "active_profile_id")]
    pub active_composition_id: String,
    #[serde(default, alias = "workspace_root")]
    pub authoring_workspace_root: Option<String>,
    #[serde(default)]
    pub editor_drafts: Option<String>,
    #[serde(default)]
    pub operation_history: Option<String>,
    #[serde(default)]
    pub recovery_backup: Option<String>,
}

impl Default for ClientStateDocument {
    fn default() -> Self {
        Self {
            schema_version: CLIENT_STATE_SCHEMA_VERSION,
            composition_revision: 0,
            compositions: CompositionDocument::default(),
            library_policies: Vec::new(),
            active_composition_id: default_composition_id(),
            authoring_workspace_root: None,
            editor_drafts: None,
            operation_history: None,
            recovery_backup: None,
        }
    }
}

fn default_schema_version() -> u32 {
    CLIENT_STATE_SCHEMA_VERSION
}

fn default_composition_id() -> String {
    "default".to_owned()
}
