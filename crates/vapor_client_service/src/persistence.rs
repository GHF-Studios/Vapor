use crate::legacy_import::import_legacy_storage;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use vapor_client_protocol::LegacyStorageSeed;
pub use vapor_client_domain::ClientStateDocument as ClientPersistentState;

const STATE_SCHEMA_VERSION: u32 = vapor_client_domain::CLIENT_STATE_SCHEMA_VERSION;
const STATE_FILE_NAME: &str = "studio-state.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedClientState {
    pub state: ClientPersistentState,
    pub legacy_imported: bool,
    pub recovery_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ClientStateStore {
    directory: PathBuf,
}

impl ClientStateStore {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub fn state_path(&self) -> PathBuf {
        self.directory.join(STATE_FILE_NAME)
    }

    fn temporary_path(&self) -> PathBuf {
        self.directory
            .join(format!(".{STATE_FILE_NAME}.tmp-{}", std::process::id()))
    }

    pub fn load_or_import(
        &self,
        legacy: Option<&LegacyStorageSeed>,
    ) -> Result<LoadedClientState, String> {
        let path = self.state_path();
        match fs::read(&path) {
            Ok(bytes) => {
                let mut state: ClientPersistentState = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("could not read saved Vapor Client state: {error}"))?;
                if state.schema_version == 1 {
                    state.schema_version = STATE_SCHEMA_VERSION;
                    self.save(&state)?;
                } else if state.schema_version != STATE_SCHEMA_VERSION {
                    return Err(format!(
                        "saved Vapor Client state schema {} is unsupported",
                        state.schema_version
                    ));
                }
                clear_runtime_installation_state(&mut state);
                normalize_active_composition(&mut state);
                Ok(LoadedClientState {
                    state,
                    legacy_imported: false,
                    recovery_message: None,
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let imported = import_legacy_storage(legacy);
                self.save(&imported.state)?;
                Ok(LoadedClientState {
                    state: imported.state,
                    legacy_imported: legacy.is_some(),
                    recovery_message: imported.recovery_message,
                })
            }
            Err(error) => Err(format!("could not read saved Vapor Client state: {error}")),
        }
    }

    pub fn save(&self, state: &ClientPersistentState) -> Result<(), String> {
        fs::create_dir_all(&self.directory)
            .map_err(|error| format!("could not create Vapor Client state directory: {error}"))?;
        let mut durable = state.clone();
        clear_runtime_installation_state(&mut durable);
        let bytes = serde_json::to_vec_pretty(&durable)
            .map_err(|error| format!("could not encode Vapor Client state: {error}"))?;
        let temporary = self.temporary_path();
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| format!("could not prepare Vapor Client state replacement: {error}"))?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| format!("could not write Vapor Client state replacement: {error}"))?;
            fs::rename(&temporary, self.state_path())
                .map_err(|error| format!("could not replace saved Vapor Client state: {error}"))?;
            sync_directory(&self.directory)
                .map_err(|error| format!("could not sync Vapor Client state directory: {error}"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn clear_runtime_installation_state(state: &mut ClientPersistentState) {
    for composition in &mut state.compositions.compositions {
        composition.installation = vapor_client_protocol::InstallationState::Unknown;
    }
}

fn normalize_active_composition(state: &mut ClientPersistentState) {
    if !state
        .compositions
        .compositions
        .iter()
        .any(|composition| composition.id == state.active_composition_id)
    {
        state.active_composition_id = state
            .compositions
            .compositions
            .first()
            .map(|composition| composition.id.clone())
            .unwrap_or_else(|| "default".to_owned());
    }
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> std::io::Result<()> {
    fs::File::open(directory)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_directory(test_name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "vapor-studio-{test_name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).expect("create temporary state directory");
        directory
    }

    #[test]
    fn imports_legacy_egui_values_once_and_round_trips_after_restart() {
        let directory = temp_directory("legacy-import");
        let store = ClientStateStore::new(directory.clone());
        let seed = LegacyStorageSeed {
            profiles: Some(
                r#"{"schema_version":1,"profiles":[{"id":"work","name":"Work"}]}"#.into(),
            ),
            active_profile_id: Some("work".into()),
            active_profile_index: Some("0".into()),
            workspace_root: Some("/projects/work".into()),
            editor_drafts: Some(r#"{"files":["draft"]}"#.into()),
            operation_history: Some(r#"[{"action":"refresh"}]"#.into()),
            recovery_backup: Some("older-backup".into()),
        };

        let imported = store
            .load_or_import(Some(&seed))
            .expect("import legacy state");
        assert!(imported.legacy_imported);
        assert_eq!(imported.state.active_composition_id, "work");
        assert_eq!(
            imported.state.authoring_workspace_root.as_deref(),
            Some("/projects/work")
        );
        assert_eq!(
            imported.state.editor_drafts.as_deref(),
            Some(r#"{"files":["draft"]}"#)
        );
        assert_eq!(
            imported.state.operation_history.as_deref(),
            Some(r#"[{"action":"refresh"}]"#)
        );

        let replacement_seed = LegacyStorageSeed {
            profiles: None,
            active_profile_id: Some("wrong".into()),
            active_profile_index: None,
            workspace_root: None,
            editor_drafts: None,
            operation_history: None,
            recovery_backup: None,
        };
        let restarted = ClientStateStore::new(directory.clone())
            .load_or_import(Some(&replacement_seed))
            .expect("load persisted state after restart");
        assert!(!restarted.legacy_imported);
        assert_eq!(restarted.state.active_composition_id, "work");
        assert_eq!(restarted.state.compositions.compositions[0].name, "Work");
        assert_eq!(
            restarted.state.recovery_backup.as_deref(),
            Some("older-backup")
        );

        fs::remove_dir_all(directory).expect("remove temporary state directory");
    }

    #[test]
    fn malformed_legacy_profiles_recover_to_default_and_preserve_the_original() {
        let directory = temp_directory("malformed-legacy");
        let seed = LegacyStorageSeed {
            profiles: Some("{ definitely not profile json".into()),
            active_profile_id: Some("missing".into()),
            active_profile_index: Some("999".into()),
            workspace_root: None,
            editor_drafts: None,
            operation_history: None,
            recovery_backup: None,
        };

        let imported = ClientStateStore::new(directory.clone())
            .load_or_import(Some(&seed))
            .expect("malformed legacy data recovers safely");
        assert!(imported.legacy_imported);
        assert_eq!(imported.state.compositions.compositions.len(), 1);
        assert_eq!(imported.state.compositions.compositions[0].id, "default");
        assert_eq!(imported.state.active_composition_id, "default");
        assert_eq!(
            imported.state.recovery_backup.as_deref(),
            Some("{ definitely not profile json")
        );
        assert!(imported.recovery_message.is_some());

        fs::remove_dir_all(directory).expect("remove temporary state directory");
    }

    #[test]
    fn failed_atomic_replacement_keeps_the_previous_state_intact() {
        let directory = temp_directory("failed-save");
        let store = ClientStateStore::new(directory.clone());
        let initial = store
            .load_or_import(None)
            .expect("write initial state")
            .state;
        let original_bytes = fs::read(store.state_path()).expect("read initial state file");
        fs::create_dir(store.temporary_path()).expect("block temporary replacement path");

        let mut changed = initial;
        changed.active_composition_id = "changed".into();
        assert!(store.save(&changed).is_err());
        assert_eq!(
            fs::read(store.state_path()).expect("read preserved state"),
            original_bytes
        );

        fs::remove_dir_all(directory).expect("remove temporary state directory");
    }

    #[test]
    fn runtime_installation_state_is_cleared_from_the_durable_file() {
        let directory = temp_directory("runtime-installation");
        let store = ClientStateStore::new(directory.clone());
        let mut state = ClientPersistentState::default();
        state.compositions.compositions[0].installation =
            vapor_client_protocol::InstallationState::NotInstalled;

        store.save(&state).expect("save durable profile state");
        let bytes = fs::read(store.state_path()).expect("read durable state file");
        let saved: ClientPersistentState =
            serde_json::from_slice(&bytes).expect("decode durable state");
        assert_eq!(
            saved.compositions.compositions[0].installation,
            vapor_client_protocol::InstallationState::Unknown
        );

        let loaded = store
            .load_or_import(None)
            .expect("load durable state after restart");
        assert_eq!(
            loaded.state.compositions.compositions[0].installation,
            vapor_client_protocol::InstallationState::Unknown
        );
        fs::remove_dir_all(directory).expect("remove temporary state directory");
    }

    #[test]
    fn migrates_profile_named_state_once_without_losing_launch_settings() {
        let directory = temp_directory("composition-state-migration");
        let store = ClientStateStore::new(directory.clone());
        fs::write(
            store.state_path(),
            r#"{
                "schema_version": 1,
                "composition_revision": 12,
                "profiles": {
                    "schema_version": 1,
                    "profiles": [{
                        "id": "flight",
                        "name": "Flight test",
                        "root_package": {"id": "org.example.launcher", "version": "1.2.0"},
                        "resolved_graph": null,
                        "workspace": null,
                        "launch": {"arguments": ["--safe", "--log-level=debug"]},
                        "installation": {"state": "unknown"}
                    }]
                },
                "active_composition_id": "flight",
                "workspace_root": "/workbench",
                "editor_drafts": null,
                "operation_history": null,
                "recovery_backup": null
            }"#,
        )
        .expect("write schema 1 state fixture");

        let loaded = store
            .load_or_import(None)
            .expect("migrate profile-named state");
        assert_eq!(loaded.state.schema_version, 2);
        assert_eq!(loaded.state.active_composition_id, "flight");
        assert_eq!(loaded.state.composition_revision, 12);
        assert_eq!(
            loaded.state.compositions.compositions[0].name,
            "Flight test"
        );
        assert_eq!(
            loaded.state.compositions.compositions[0].launch.arguments,
            ["--safe", "--log-level=debug"]
        );
        assert_eq!(
            loaded.state.compositions.compositions[0]
                .root_package
                .as_ref()
                .map(|root| root.id.as_str()),
            Some("org.example.launcher")
        );

        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(store.state_path()).expect("read migrated state"))
                .expect("decode migrated state JSON");
        assert_eq!(saved["schema_version"], 2);
        assert!(saved.get("compositions").is_some());
        assert!(saved.get("active_composition_id").is_some());
        assert!(saved.get("profiles").is_none());
        assert!(saved.get("active_profile_id").is_none());

        let restarted = store.load_or_import(None).expect("load migrated state");
        assert_eq!(restarted.state, loaded.state);
        fs::remove_dir_all(directory).expect("remove temporary state directory");
    }
}
