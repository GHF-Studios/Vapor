//! Read-only projections of service state for frontend snapshots.

use super::{
    ClientSession,
    catalog::catalog_projection,
    default_superworkspace_path,
    operation_registry::OperationResourceKey,
    workshop::{workshop_batch_review_snapshot, workshop_review_snapshot},
};
use vapor_client_protocol::{
    ActionProfile, AvailableAction, CatalogAction, ClientAction, ClientSnapshot, CompositionAction,
    DurableStateSnapshot, HostSummary, InstallationRoleSnapshot, InstallationState,
    LocalCapability, RootDistributionIdentitySnapshot, SuperworkspaceRepositorySnapshot,
    SuperworkspaceSnapshot, WorkflowTier, WorkspaceAction,
};

impl ClientSession {
    pub fn snapshot(&self) -> ClientSnapshot {
        let mut available_actions = vec![AvailableAction::from_action(
            &ClientAction::Catalog(CatalogAction::Refresh),
            self.can_start_operation(OperationResourceKey::CatalogRefresh),
            (!self.can_start_operation(OperationResourceKey::CatalogRefresh))
                .then(|| "A Library refresh is already running.".to_owned()),
        )];
        available_actions.extend([
            AvailableAction::from_action(
                &ClientAction::Workspace(WorkspaceAction::InitializeSuperworkspace),
                !self.bootstrap.fixture_mode,
                self.bootstrap
                    .fixture_mode
                    .then(|| "Unavailable in fixture mode.".to_owned()),
            ),
            AvailableAction::from_action(
                &ClientAction::Workspace(WorkspaceAction::SetSuperworkspaceLocation {
                    root: String::new(),
                }),
                !self.bootstrap.fixture_mode,
                self.bootstrap
                    .fixture_mode
                    .then(|| "Unavailable in fixture mode.".to_owned()),
            ),
            AvailableAction::from_action(
                &ClientAction::Workspace(WorkspaceAction::DiscoverSuperworkspaceRepositories),
                !self.bootstrap.fixture_mode
                    && self.can_start_operation(OperationResourceKey::Workspace(
                        self.bootstrap.workspace_root.clone().unwrap_or_default(),
                    )),
                if self.bootstrap.fixture_mode {
                    Some("Unavailable in fixture mode.".to_owned())
                } else if !self.can_start_operation(OperationResourceKey::Workspace(
                    self.bootstrap.workspace_root.clone().unwrap_or_default(),
                )) {
                    Some("A conflicting workspace operation is already running.".to_owned())
                } else {
                    None
                },
            ),
            AvailableAction::from_action(
                &ClientAction::Workspace(WorkspaceAction::AcquireSuperworkspaceRepositories {
                    repository_names: Vec::new(),
                }),
                !self.bootstrap.fixture_mode
                    && self.can_start_operation(OperationResourceKey::Workspace(
                        self.bootstrap.workspace_root.clone().unwrap_or_default(),
                    )),
                if self.bootstrap.fixture_mode {
                    Some("Unavailable in fixture mode.".to_owned())
                } else if !self.can_start_operation(OperationResourceKey::Workspace(
                    self.bootstrap.workspace_root.clone().unwrap_or_default(),
                )) {
                    Some("A conflicting workspace operation is already running.".to_owned())
                } else {
                    None
                },
            ),
            AvailableAction::from_action(
                &ClientAction::Catalog(CatalogAction::DiscoverContentProjects),
                self.bootstrap.workspace_root.is_some()
                    && self.can_start_operation(OperationResourceKey::WorkspaceScan(
                        self.bootstrap.workspace_root.clone().unwrap_or_default(),
                    )),
                if self.bootstrap.workspace_root.is_none() {
                    Some("Open a workspace first.".to_owned())
                } else if !self.can_start_operation(OperationResourceKey::WorkspaceScan(
                    self.bootstrap.workspace_root.clone().unwrap_or_default(),
                )) {
                    Some("Workspace discovery is already running.".to_owned())
                } else {
                    None
                },
            ),
            AvailableAction::from_action(
                &ClientAction::Composition(CompositionAction::Select {
                    composition_id: String::new(),
                }),
                true,
                None,
            ),
            AvailableAction::from_action(
                &ClientAction::Catalog(CatalogAction::Search {
                    query: String::new(),
                }),
                true,
                None,
            ),
            AvailableAction::from_action(
                &ClientAction::Catalog(CatalogAction::Inspect {
                    content_id: String::new(),
                    version: None,
                }),
                !self.catalog.entries().is_empty(),
                self.catalog
                    .entries()
                    .is_empty()
                    .then(|| "Refresh the catalog first.".to_owned()),
            ),
            AvailableAction::from_action(
                &ClientAction::Workspace(WorkspaceAction::Open {
                    root: String::new(),
                }),
                true,
                None,
            ),
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
        let composition_needs_repair = active_composition.is_some_and(|composition| {
            matches!(
                &composition.installation,
                InstallationState::NeedsRepair { .. } | InstallationState::Partial { .. }
            )
        });
        let composition_is_installed = active_composition.is_some_and(|composition| {
            matches!(
                &composition.installation,
                InstallationState::Installed { .. }
            )
        });
        let has_workspace_source = active_composition.is_some_and(|composition| {
            composition.root_package.is_some() && composition.workspace.is_some()
        });
        let active_composition_key =
            OperationResourceKey::Composition(self.durable_state.active_composition_id.clone());
        let composition_operation_conflict =
            !self.can_start_operation(active_composition_key.clone());
        let workspace_key = OperationResourceKey::Workspace(
            self.bootstrap.workspace_root.clone().unwrap_or_default(),
        );
        let workspace_operation_conflict = !self.can_start_operation(workspace_key.clone());
        let installation_role = if self.bootstrap.fixture_mode {
            InstallationRoleSnapshot {
                role: "player".to_owned(),
                ..Default::default()
            }
        } else {
            super::application::installation_role_snapshot()
        };
        let launch_reason = if composition_operation_conflict {
            Some("This composition already has an operation running.".to_owned())
        } else if !composition_has_root {
            Some("Choose a playable composition first.".to_owned())
        } else if composition_is_out_of_date {
            Some("Review or update this composition before launching.".to_owned())
        } else {
            None
        };
        let composition_operation_reason = if composition_operation_conflict {
            Some("This composition already has an operation running.".to_owned())
        } else if !composition_has_root {
            Some("Choose a playable composition first.".to_owned())
        } else {
            None
        };
        let install_reason = composition_operation_reason.clone().or_else(|| {
            composition_is_installed.then(|| "This composition is already installed.".to_owned())
        });
        let verify_reason = composition_operation_reason.clone().or_else(|| {
            (!composition_is_installed && !composition_needs_repair && !composition_is_out_of_date)
                .then(|| "Install this composition before verifying it.".to_owned())
        });
        let repair_reason = composition_operation_reason.clone().or_else(|| {
            (!composition_needs_repair)
                .then(|| "This composition does not currently need repair.".to_owned())
        });
        let update_reason = composition_operation_reason.clone().or_else(|| {
            (!composition_is_out_of_date)
                .then(|| "This composition has no pending update.".to_owned())
        });
        let build_reason = if composition_operation_conflict {
            Some("This composition already has an operation running.".to_owned())
        } else if workspace_operation_conflict {
            Some("A conflicting workspace operation is already running.".to_owned())
        } else if !has_workspace_source {
            Some("Open a project-backed composition before building and launching.".to_owned())
        } else if !self.bootstrap.fixture_mode && !installation_role.rust_ready {
            Some(installation_role.error.clone().unwrap_or_else(|| {
                "Set up Vapor-managed Rust/Cargo in Vapor Installer before building.".to_owned()
            }))
        } else {
            None
        };
        available_actions.extend([
            AvailableAction::from_action(
                &ClientAction::Composition(CompositionAction::Install {
                    composition_id: String::new(),
                }),
                install_reason.is_none(),
                install_reason,
            )
            .with_queueable(
                composition_operation_conflict && composition_has_root && !composition_is_installed,
            ),
            AvailableAction::from_action(
                &ClientAction::Composition(CompositionAction::Verify {
                    composition_id: String::new(),
                }),
                verify_reason.is_none(),
                verify_reason,
            )
            .with_queueable(
                composition_operation_conflict
                    && composition_has_root
                    && (composition_is_installed
                        || composition_needs_repair
                        || composition_is_out_of_date),
            ),
            AvailableAction::from_action(
                &ClientAction::Composition(CompositionAction::Repair {
                    composition_id: String::new(),
                }),
                repair_reason.is_none(),
                repair_reason,
            )
            .with_queueable(composition_operation_conflict && composition_needs_repair),
            AvailableAction::from_action(
                &ClientAction::Composition(CompositionAction::Update {
                    composition_id: String::new(),
                }),
                update_reason.is_none(),
                update_reason,
            )
            .with_queueable(composition_operation_conflict && composition_is_out_of_date),
            AvailableAction::from_action(
                &ClientAction::Composition(CompositionAction::Launch {
                    composition_id: String::new(),
                }),
                launch_reason.is_none(),
                launch_reason,
            )
            .with_queueable(
                composition_operation_conflict
                    && composition_has_root
                    && !composition_is_out_of_date,
            ),
            AvailableAction::from_profile(
                "composition.build-and-launch",
                build_reason.is_none(),
                build_reason,
                ActionProfile::new(WorkflowTier::Composer)
                    .with_local_capabilities(&[LocalCapability::ManagedRustToolchain]),
            )
            .with_queueable(
                (composition_operation_conflict || workspace_operation_conflict)
                    && has_workspace_source
                    && (self.bootstrap.fixture_mode || installation_role.rust_ready),
            ),
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
            active_operation: self.operation_registry.primary().cloned(),
            active_operations: self.operation_registry.active_summaries(),
            operation_history: self.operation_registry.history().iter().cloned().collect(),
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
            installation_role,
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
