//! Read-only projections of service state for frontend snapshots.

use super::{
    ClientSession,
    catalog::catalog_projection,
    default_superworkspace_path,
    workshop::{workshop_batch_review_snapshot, workshop_review_snapshot},
};
use vapor_client_protocol::{
    AvailableAction, ClientSnapshot, DurableStateSnapshot, HostSummary, InstallationRoleSnapshot,
    InstallationState, RootDistributionIdentitySnapshot, SuperworkspaceRepositorySnapshot,
    SuperworkspaceSnapshot,
};

impl ClientSession {
    pub fn snapshot(&self) -> ClientSnapshot {
        let mut available_actions = vec![AvailableAction {
            id: "catalog.refresh".to_owned(),
            enabled: true,
            reason: None,
        }];
        available_actions.extend([
            AvailableAction {
                id: "workspace.initialize-superworkspace".to_owned(),
                enabled: !self.bootstrap.fixture_mode,
                reason: self
                    .bootstrap
                    .fixture_mode
                    .then(|| "Unavailable in fixture mode.".to_owned()),
            },
            AvailableAction {
                id: "workspace.set-superworkspace-location".to_owned(),
                enabled: !self.bootstrap.fixture_mode,
                reason: self
                    .bootstrap
                    .fixture_mode
                    .then(|| "Unavailable in fixture mode.".to_owned()),
            },
            AvailableAction {
                id: "workspace.discover-superworkspace-repositories".to_owned(),
                enabled: !self.bootstrap.fixture_mode && self.active_operation.is_none(),
                reason: if self.bootstrap.fixture_mode {
                    Some("Unavailable in fixture mode.".to_owned())
                } else if self.active_operation.is_some() {
                    Some("Wait for the current operation to finish.".to_owned())
                } else {
                    None
                },
            },
            AvailableAction {
                id: "workspace.acquire-superworkspace-repositories".to_owned(),
                enabled: !self.bootstrap.fixture_mode && self.active_operation.is_none(),
                reason: if self.bootstrap.fixture_mode {
                    Some("Unavailable in fixture mode.".to_owned())
                } else if self.active_operation.is_some() {
                    Some("Wait for the current operation to finish.".to_owned())
                } else {
                    None
                },
            },
            AvailableAction {
                id: "catalog.discover-content-projects".to_owned(),
                enabled: self.bootstrap.workspace_root.is_some(),
                reason: self
                    .bootstrap
                    .workspace_root
                    .is_none()
                    .then(|| "Open a workspace first.".to_owned()),
            },
            AvailableAction {
                id: "composition.select".to_owned(),
                enabled: true,
                reason: None,
            },
            AvailableAction {
                id: "catalog.search".to_owned(),
                enabled: true,
                reason: None,
            },
            AvailableAction {
                id: "catalog.inspect".to_owned(),
                enabled: !self.catalog.entries().is_empty(),
                reason: self
                    .catalog
                    .entries()
                    .is_empty()
                    .then(|| "Refresh the catalog first.".to_owned()),
            },
            AvailableAction {
                id: "workspace.open".to_owned(),
                enabled: true,
                reason: None,
            },
        ]);
        let active_composition =
            self.durable_state
                .compositions
                .compositions
                .iter()
                .find(|composition| {
                    self.durable_state.active_composition_id.as_str() == composition.id.as_str()
                });
        let composition_has_root =
            active_composition.is_some_and(|composition| composition.root_package.is_some());
        let composition_is_out_of_date = active_composition.is_some_and(|composition| {
            matches!(
                &composition.installation,
                InstallationState::OutOfDate { .. }
            )
        });
        let has_workspace_source = active_composition.is_some_and(|composition| {
            composition.root_package.is_some() && composition.workspace.is_some()
        });
        let operation_is_idle = self.active_operation.is_none();
        let launch_reason = if !operation_is_idle {
            Some("Wait for the current operation to finish.".to_owned())
        } else if !composition_has_root {
            Some("Choose a playable composition first.".to_owned())
        } else if composition_is_out_of_date {
            Some("Review or update this composition before launching.".to_owned())
        } else {
            None
        };
        let build_reason = if !operation_is_idle {
            Some("Wait for the current operation to finish.".to_owned())
        } else if !has_workspace_source {
            Some("Open a project-backed composition before building and launching.".to_owned())
        } else {
            None
        };
        available_actions.extend([
            AvailableAction {
                id: "composition.launch".to_owned(),
                enabled: launch_reason.is_none(),
                reason: launch_reason,
            },
            AvailableAction {
                id: "composition.build-and-launch".to_owned(),
                enabled: build_reason.is_none(),
                reason: build_reason,
            },
        ]);
        let mut workshop = self.workshop.clone();
        workshop.review = self.workshop_review.as_ref().map(workshop_review_snapshot);
        workshop.batch_review = self
            .workshop_batch_review
            .as_ref()
            .map(workshop_batch_review_snapshot);
        ClientSnapshot {
            revision: self.revision,
            host: HostSummary {
                app_root: self
                    .host_context
                    .as_ref()
                    .and_then(|context| context.app_root.as_ref())
                    .map(|root| root.display().to_string())
                    .or_else(|| {
                        self.bootstrap
                            .application_executable
                            .parent()
                            .and_then(|path| path.parent())
                            .and_then(|path| path.parent())
                            .map(|root| root.display().to_string())
                    }),
                workspace: self
                    .bootstrap
                    .workspace_root
                    .as_ref()
                    .map(|path| path.display().to_string()),
                mode: if self.bootstrap.fixture_mode {
                    "fixture".to_owned()
                } else {
                    "production".to_owned()
                },
                platform: self.host_context.as_ref().map_or_else(
                    || std::env::consts::OS.to_owned(),
                    |context| format!("{:?}", context.platform).to_lowercase(),
                ),
                steam_client_available: self
                    .host_context
                    .as_ref()
                    .is_some_and(|context| context.steam_client_available),
            },
            root_distribution: RootDistributionIdentitySnapshot {
                app_id: vapor_sdk_core::root::types::ROOT_STEAM_APP_ID,
                depot_id: vapor_sdk_core::root::types::ROOT_STEAM_DEPOT_ID,
            },
            active_composition_id: self.active_composition_id.clone(),
            durable_state: DurableStateSnapshot {
                schema_version: self.durable_state.schema_version,
                composition_revision: self.durable_state.composition_revision,
                compositions: self.durable_state.compositions.clone(),
                library_policies: self.durable_state.library_policies.clone(),
                active_composition_id: self.durable_state.active_composition_id.clone(),
                authoring_workspace_root: self.durable_state.authoring_workspace_root.clone(),
                editor_drafts: self.durable_state.editor_drafts.clone(),
                operation_history: self.durable_state.operation_history.clone(),
                recovery_backup: self.durable_state.recovery_backup.clone(),
            },
            active_operation: self.active_operation.clone(),
            composition_preview: self.composition_preview.clone(),
            catalog: self
                .catalog
                .entries()
                .iter()
                .map(|entry| catalog_projection(entry, &self.durable_state, self.catalog.entries()))
                .collect(),
            catalog_status: self.catalog.status().clone(),
            catalog_search: self.catalog.search(),
            workspace_content: self.workspace_content.clone(),
            superworkspace: self.superworkspace_snapshot(),
            workshop,
            root_publishing: self
                .root_publish_review
                .as_ref()
                .map(|review| review.snapshot.clone()),
            application_release: self.application_release_snapshot.clone(),
            profile_traces: self.profile_traces.clone(),
            profile_trace_error: self.profile_trace_error.clone(),
            installation_role: if self.bootstrap.fixture_mode {
                InstallationRoleSnapshot {
                    role: "player".to_owned(),
                    ..Default::default()
                }
            } else {
                super::application::installation_role_snapshot()
            },
            selected_content_id: self.selected_content_id.clone(),
            available_actions,
        }
    }

    fn superworkspace_snapshot(&self) -> SuperworkspaceSnapshot {
        if self.bootstrap.fixture_mode {
            return SuperworkspaceSnapshot {
                status: "fixture".to_owned(),
                ..SuperworkspaceSnapshot::default()
            };
        }
        let installation = match vapor_core::VaporInstallation::discover() {
            Ok(installation) => installation,
            Err(error) => {
                return SuperworkspaceSnapshot {
                    status: "unavailable".to_owned(),
                    error: Some(error.to_string()),
                    ..SuperworkspaceSnapshot::default()
                };
            }
        };
        let configured = match vapor_core::source_state(&installation) {
            Ok(state) => state.superworkspace,
            Err(error) => {
                return SuperworkspaceSnapshot {
                    status: "unavailable".to_owned(),
                    error: Some(error.to_string()),
                    ..SuperworkspaceSnapshot::default()
                };
            }
        };
        let is_configured = configured.is_some();
        let root = configured.or_else(default_superworkspace_path);
        let Some(root) = root else {
            return SuperworkspaceSnapshot {
                status: "unavailable".to_owned(),
                error: Some("Your home folder could not be resolved.".to_owned()),
                ..SuperworkspaceSnapshot::default()
            };
        };
        let mut snapshot = SuperworkspaceSnapshot {
            status: "not-initialized".to_owned(),
            root: Some(root.display().to_string()),
            ..SuperworkspaceSnapshot::default()
        };
        match vapor_core::VaporSuperworkspace::discover_from(&root) {
            Ok(discovered) if discovered.root == root => {
                snapshot.status = "ready".to_owned();
                snapshot.repositories = discovered
                    .repositories
                    .into_iter()
                    .map(|repository| SuperworkspaceRepositorySnapshot {
                        name: repository.name,
                        root: repository.root.display().to_string(),
                        kind: repository.kind.to_string(),
                    })
                    .collect();
                snapshot.available_repositories = self
                    .superworkspace_sources
                    .iter()
                    .cloned()
                    .map(|mut repository| {
                        repository.acquired = snapshot
                            .repositories
                            .iter()
                            .any(|local| local.name == repository.checkout_path);
                        repository
                    })
                    .collect();
                snapshot.available_repositories_loaded = self.superworkspace_sources_loaded;
                snapshot.available_repositories_error = self.superworkspace_sources_error.clone();
            }
            Ok(discovered) => {
                snapshot.status = "unavailable".to_owned();
                snapshot.error = Some(format!(
                    "The selected folder is inside another Superworkspace at {}.",
                    discovered.root.display()
                ));
            }
            Err(error) if is_configured => {
                snapshot.status = "unavailable".to_owned();
                snapshot.error = Some(error.to_string());
            }
            Err(_) => {}
        }
        if let Some(error) = self.superworkspace_setup_error.as_ref() {
            snapshot.error = Some(error.clone());
            if snapshot.status != "ready" {
                snapshot.status = "unavailable".to_owned();
            }
        }
        snapshot
    }
}
