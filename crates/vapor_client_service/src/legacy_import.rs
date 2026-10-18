use crate::persistence::ClientPersistentState;
use vapor_client_protocol::LegacyStorageSeed;
use vapor_client_application::decode_compositions;

pub struct LegacyImport {
    pub state: ClientPersistentState,
    pub recovery_message: Option<String>,
}

pub fn import_legacy_storage(seed: Option<&LegacyStorageSeed>) -> LegacyImport {
    let Some(seed) = seed else {
        return LegacyImport {
            state: ClientPersistentState::default(),
            recovery_message: None,
        };
    };
    let (compositions, profile_recovery) = decode_compositions(seed.profiles.as_deref());
    let active_composition_id = seed
        .active_profile_id
        .as_deref()
        .filter(|id| {
            compositions
                .compositions
                .iter()
                .any(|composition| composition.id == *id)
        })
        .map(str::to_owned)
        .or_else(|| {
            seed.active_profile_index
                .as_deref()
                .and_then(|index| index.parse::<usize>().ok())
                .and_then(|index| compositions.compositions.get(index))
                .map(|composition| composition.id.clone())
        })
        .unwrap_or_else(|| compositions.compositions[0].id.clone());
    // The old frontend sometimes inferred the active authoring folder from the
    // selected composition's source binding. Preserve only a folder the user
    // explicitly saved as active authoring context.
    let authoring_workspace_root = seed.workspace_root.clone();
    let recovery_backup = seed.recovery_backup.clone().or_else(|| {
        profile_recovery
            .as_ref()
            .and_then(|_| seed.profiles.clone())
    });
    LegacyImport {
        state: ClientPersistentState {
            schema_version: vapor_client_domain::CLIENT_STATE_SCHEMA_VERSION,
            composition_revision: 0,
            compositions,
            library_policies: Vec::new(),
            active_composition_id,
            authoring_workspace_root,
            editor_drafts: seed.editor_drafts.clone(),
            operation_history: seed.operation_history.clone(),
            recovery_backup,
        },
        recovery_message: profile_recovery.map(|recovery| recovery.message),
    }
}
