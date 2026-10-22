//! Service-owned discovery refresh and catalog projections.

use super::{
    BackgroundOperationMessage, BackgroundOperationSpec, CatalogRefreshCompletion, ClientSession,
    MAX_WORKSPACE_DIAGNOSTICS, MAX_WORKSPACE_PROJECTS, WorkspaceDiscoveryCompletion,
};
use crate::persistence::ClientPersistentState;
use std::path::Path;
use vapor_client_application::{
    CatalogContent, SourceContent, discover_local_content_catalog, discover_source_content,
    scan_unregistered_content_projects,
};
use vapor_client_protocol::{
    CatalogConflict, CatalogEntry, CatalogItemAction, CatalogItemRole, CatalogLocation,
    CatalogLocationKind, ClientEvent, InstallationState, OperationEvent, OperationPhase,
    OperationStatus, WorkspaceContentSnapshot, WorkspaceProjectCandidate, WorkspaceSourceProject,
    WorkspaceWorkshopPolicy,
};

impl ClientSession {
    pub(super) fn start_workspace_discovery_operation(
        &mut self,
    ) -> Result<(), vapor_client_protocol::ServiceError> {
        let workspace = self.bootstrap.workspace_root.clone().ok_or_else(|| {
            vapor_client_protocol::ServiceError {
                code: vapor_client_protocol::ServiceErrorCode::ActionUnavailable,
                message: "Open a workspace before discovering its content projects.".to_owned(),
                retryable: false,
            }
        })?;
        let executable = self.bootstrap.application_executable.clone();
        let execution_workspace = workspace.clone();
        self.spawn_background_operation(
            BackgroundOperationSpec {
                domain: "studio-authoring",
                action: "authoring.discover-content-projects",
                thread_prefix: "studio-workspace-discovery",
                failure_label: "workspace content discovery",
                composition_id: None,
                phase: OperationPhase::Resolving,
            },
            move |context| {
                discover_workspace_content(
                    &executable,
                    Some(&execution_workspace),
                    false,
                    Some(&context),
                )
            },
            |snapshot| {
                Ok(format!(
                    "Found {} source projects and {} unregistered candidates.",
                    snapshot.source_projects.len(),
                    snapshot.unregistered_projects.len()
                ))
            },
            move |id, result| {
                BackgroundOperationMessage::CompleteWorkspaceDiscovery(Box::new(
                    WorkspaceDiscoveryCompletion {
                        id,
                        workspace,
                        result,
                    },
                ))
            },
        )?;
        self.workspace_content.source_projects_loaded = false;
        self.workspace_content.unregistered_projects_loaded = false;
        self.workspace_content.source_projects_error = None;
        self.workspace_content.unregistered_projects_error = None;
        self.pending_workspace_discovery = false;
        Ok(())
    }

    pub(super) fn apply_workspace_discovery_completion(
        &mut self,
        completion: WorkspaceDiscoveryCompletion,
    ) {
        let Some(active) = self
            .operation_registry
            .get(&completion.id)
            .map(|operation| operation.summary.clone())
        else {
            return;
        };
        let same_workspace =
            self.bootstrap.workspace_root.as_deref() == Some(&completion.workspace);
        let succeeded = same_workspace;
        let message = (!same_workspace).then(|| {
            "The active workspace changed during discovery; the result was discarded.".to_owned()
        });
        if same_workspace {
            self.workspace_content = completion.result;
        }
        let status = if let Some(message) = &message {
            OperationStatus::Failed {
                message: message.clone(),
            }
        } else {
            OperationStatus::Succeeded
        };
        self.finish_operation(&completion.id, status.clone());
        self.revision.0 = self.revision.0.saturating_add(1);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: completion.id,
                composition_id: active.composition_id,
                action: active.action,
                phase: OperationPhase::Complete,
                status,
                progress: succeeded.then_some(1.0),
                message,
            }));
    }

    pub(super) fn start_catalog_refresh_operation(
        &mut self,
    ) -> Result<(), vapor_client_protocol::ServiceError> {
        if self.bootstrap.fixture_mode {
            self.refresh_catalog();
            return Ok(());
        }
        let executable = self.bootstrap.application_executable.clone();
        self.spawn_background_operation(
            BackgroundOperationSpec {
                domain: "vapor-library",
                action: "catalog.refresh",
                thread_prefix: "vapor-catalog-refresh",
                failure_label: "Library refresh",
                composition_id: None,
                phase: OperationPhase::Resolving,
            },
            move |context| {
                context.set_phase(vapor_client_application::OperationPhase::Resolving);
                context.report_progress(vapor_client_application::Progress {
                    completed: 0,
                    total: Some(1),
                    unit: "scan".to_owned(),
                    message: Some("Scanning installed and cached content.".to_owned()),
                });
                let result = discover_catalog_entries(&executable);
                context.report_progress(vapor_client_application::Progress {
                    completed: 1,
                    total: Some(1),
                    unit: "scan".to_owned(),
                    message: Some("Library scan finished.".to_owned()),
                });
                result
            },
            |result| {
                result
                    .as_ref()
                    .map(|entries| format!("Found {} local items.", entries.len()))
                    .map_err(Clone::clone)
            },
            |id, result| {
                BackgroundOperationMessage::CompleteCatalogRefresh(Box::new(
                    CatalogRefreshCompletion { id, result },
                ))
            },
        )?;
        self.pending_catalog_refresh = false;
        Ok(())
    }

    pub(super) fn apply_catalog_refresh_completion(
        &mut self,
        completion: CatalogRefreshCompletion,
    ) {
        let Some(active) = self
            .operation_registry
            .get(&completion.id)
            .map(|operation| operation.summary.clone())
        else {
            return;
        };
        let result = completion.result;
        match &result {
            Ok(entries) => self.catalog.replace(entries.clone()),
            Err(message) => self.catalog.fail(message.clone()),
        }
        let status = match &result {
            Ok(_) => OperationStatus::Succeeded,
            Err(message) => OperationStatus::Failed {
                message: message.clone(),
            },
        };
        self.finish_operation(&completion.id, status.clone());
        self.revision.0 = self.revision.0.saturating_add(1);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: completion.id,
                composition_id: active.composition_id,
                action: active.action,
                phase: OperationPhase::Complete,
                status,
                progress: result.as_ref().ok().map(|_| 1.0),
                message: result
                    .map(|entries| format!("Found {} local items.", entries.len()))
                    .err(),
            }));
    }

    pub(super) fn refresh_all_content_views(&mut self) {
        self.refresh_catalog();
        self.refresh_workspace_content();
    }

    pub(super) fn refresh_catalog(&mut self) {
        if self.bootstrap.fixture_mode {
            let mut entries = vec![CatalogEntry {
                id: "fixture.engine".to_owned(),
                name: Some("Fixture Engine".to_owned()),
                version: Some("1.0.0".to_owned()),
                kind: "engine".to_owned(),
                published_file_id: None,
                dependencies: Vec::new(),
                dependency_details: Vec::new(),
                conflicts: Vec::new(),
                locations: vec![CatalogLocation {
                    kind: CatalogLocationKind::Cache,
                    root: "fixture/cache/fixture.engine".to_owned(),
                    enabled: true,
                    fingerprint: Some("fixture:v1".to_owned()),
                    install_source: None,
                }],
                role: CatalogItemRole::Other,
                actions: Vec::new(),
            }];
            if self.bootstrap.workspace_root.is_some() {
                entries.push(CatalogEntry {
                    id: "fixture.game".to_owned(),
                    name: Some("Fixture Game".to_owned()),
                    version: Some("1.0.0".to_owned()),
                    kind: "game".to_owned(),
                    published_file_id: Some("1003".to_owned()),
                    dependencies: Vec::new(),
                    dependency_details: Vec::new(),
                    conflicts: vec![CatalogConflict {
                        field: "fingerprint".to_owned(),
                        first_value: "source".to_owned(),
                        first_location: "fixture/workspace/fixture.game".to_owned(),
                        conflicting_value: "cache".to_owned(),
                        conflicting_location: "fixture/cache/fixture.game".to_owned(),
                    }],
                    locations: vec![CatalogLocation {
                        kind: CatalogLocationKind::Cache,
                        root: "fixture/cache/fixture.game".to_owned(),
                        enabled: true,
                        fingerprint: Some("fixture:old".to_owned()),
                        install_source: None,
                    }],
                    role: CatalogItemRole::Other,
                    actions: Vec::new(),
                });
            }
            entries.extend(self.fixture_acquisitions.values().map(|item| CatalogEntry {
                id: item.id.clone(),
                name: Some(item.id.clone()),
                version: item.version.clone(),
                kind: item.kind.clone(),
                published_file_id: None,
                dependencies: Vec::new(),
                dependency_details: Vec::new(),
                conflicts: Vec::new(),
                locations: vec![CatalogLocation {
                    kind: CatalogLocationKind::Cache,
                    root: item.cache_root.clone(),
                    enabled: true,
                    fingerprint: Some(item.fingerprint.clone()),
                    install_source: None,
                }],
                role: CatalogItemRole::Other,
                actions: Vec::new(),
            }));
            self.catalog.replace(entries);
            return;
        }
        if let Err(error) = self.start_catalog_refresh_operation() {
            if error.code == vapor_client_protocol::ServiceErrorCode::OperationInProgress {
                self.pending_catalog_refresh = true;
            } else {
                self.catalog.fail(error.message);
            }
        }
    }

    pub(super) fn refresh_workspace_content(&mut self) {
        if self.bootstrap.fixture_mode || self.bootstrap.workspace_root.is_none() {
            self.workspace_content = discover_workspace_content(
                &self.bootstrap.application_executable,
                self.bootstrap.workspace_root.as_deref(),
                self.bootstrap.fixture_mode,
                None,
            );
        } else {
            if self.start_workspace_discovery_operation().is_err() {
                self.pending_workspace_discovery = true;
            }
        }
    }
}

fn discover_workspace_content(
    executable: &Path,
    root: Option<&Path>,
    fixture_mode: bool,
    context: Option<&vapor_client_application::OperationContext>,
) -> WorkspaceContentSnapshot {
    let Some(root) = root else {
        return WorkspaceContentSnapshot {
            source_projects_loaded: true,
            unregistered_projects_loaded: true,
            ..WorkspaceContentSnapshot::default()
        };
    };
    if fixture_mode {
        return fixture_workspace_content(root);
    }
    let mut snapshot = WorkspaceContentSnapshot {
        workspace_validation_error: vapor_client_application::validate_workspace(root).err(),
        ..WorkspaceContentSnapshot::default()
    };
    if let Some(context) = context {
        context.set_phase(vapor_client_application::OperationPhase::Resolving);
        context.report_progress(vapor_client_application::Progress {
            completed: 0,
            total: Some(2),
            unit: "discovery phase".to_owned(),
            message: Some("Discovering registered source projects.".to_owned()),
        });
        if context.is_cancelled() {
            return snapshot;
        }
    }
    match discover_source_content(executable, root) {
        Ok(projects) => {
            snapshot.source_projects = projects
                .into_iter()
                .take(MAX_WORKSPACE_PROJECTS)
                .map(source_project)
                .collect();
            snapshot.source_projects_loaded = true;
        }
        Err(error) => snapshot.source_projects_error = Some(error),
    }
    if let Some(context) = context {
        context.report_progress(vapor_client_application::Progress {
            completed: 1,
            total: Some(2),
            unit: "discovery phase".to_owned(),
            message: Some("Scanning for unregistered content projects.".to_owned()),
        });
        if context.is_cancelled() {
            return snapshot;
        }
    }
    match scan_unregistered_content_projects(executable, root) {
        Ok(scan) => {
            snapshot.unregistered_projects = scan
                .candidates
                .into_iter()
                .take(MAX_WORKSPACE_PROJECTS)
                .map(|candidate| WorkspaceProjectCandidate {
                    path: candidate.path.display().to_string(),
                    id: candidate.id,
                    kind: candidate.kind.to_string(),
                })
                .collect();
            snapshot.unregistered_diagnostics = scan
                .diagnostics
                .into_iter()
                .take(MAX_WORKSPACE_DIAGNOSTICS)
                .collect();
            snapshot.unregistered_projects_loaded = true;
        }
        Err(error) => snapshot.unregistered_projects_error = Some(error),
    }
    if let Some(context) = context {
        context.report_progress(vapor_client_application::Progress {
            completed: 2,
            total: Some(2),
            unit: "discovery phase".to_owned(),
            message: Some("Workspace discovery finished.".to_owned()),
        });
    }
    snapshot
}

fn discover_catalog_entries(executable: &Path) -> Result<Vec<CatalogEntry>, String> {
    discover_local_content_catalog(executable)
        .map(|catalog| catalog.into_iter().map(catalog_entry).collect())
}

fn source_project(project: SourceContent) -> WorkspaceSourceProject {
    WorkspaceSourceProject {
        id: project.id,
        version: project.version,
        kind: project.kind,
        root: project.root.display().to_string(),
        manifest: project.manifest.display().to_string(),
        binaries: project.binaries,
        libraries: project.libraries,
        dependencies: project.dependencies,
        workshop: WorkspaceWorkshopPolicy {
            app_id: project.workshop.app_id,
            published_file_id: project.workshop.published_file_id,
            visibility: project.workshop.visibility,
            title: project.workshop.title,
            description: project.workshop.description,
            tags: project.workshop.tags,
            change_note: project.workshop.change_note,
        },
    }
}

fn fixture_workspace_content(root: &Path) -> WorkspaceContentSnapshot {
    WorkspaceContentSnapshot {
        source_projects: vec![WorkspaceSourceProject {
            id: "fixture.game".to_owned(),
            version: Some("1.0.0".to_owned()),
            kind: "game".to_owned(),
            root: root.join("game").display().to_string(),
            manifest: root.join("game/Vapor.toml").display().to_string(),
            binaries: vec!["fixture-game".to_owned()],
            libraries: Vec::new(),
            dependencies: vec!["required · fixture.engine".to_owned()],
            workshop: WorkspaceWorkshopPolicy::default(),
        }],
        source_projects_loaded: true,
        source_projects_error: None,
        unregistered_projects: vec![WorkspaceProjectCandidate {
            path: root.join("unregistered-addon").display().to_string(),
            id: "fixture.addon".to_owned(),
            kind: "game-mod".to_owned(),
        }],
        unregistered_diagnostics: vec!["fixture diagnostic".to_owned()],
        unregistered_projects_loaded: true,
        unregistered_projects_error: None,
        workspace_validation_error: None,
    }
}

fn catalog_entry(entry: CatalogContent) -> CatalogEntry {
    let role = catalog_item_role(&entry.kind);
    let dependency_labels = entry
        .dependencies
        .iter()
        .map(|dependency| format!("{}: {}", dependency.relationship, dependency.id))
        .collect();
    CatalogEntry {
        id: entry.id,
        name: entry.name,
        version: entry.version,
        kind: entry.kind,
        published_file_id: entry.published_file_id,
        dependencies: dependency_labels,
        dependency_details: entry.dependencies,
        conflicts: entry
            .conflicts
            .into_iter()
            .map(|conflict| CatalogConflict {
                field: conflict.field,
                first_value: conflict.first_value,
                first_location: conflict.first_location.display().to_string(),
                conflicting_value: conflict.conflicting_value,
                conflicting_location: conflict.conflicting_location.display().to_string(),
            })
            .collect(),
        locations: entry
            .locations
            .into_iter()
            .map(|location| CatalogLocation {
                kind: match location.kind {
                    vapor_client_application::CatalogLocationKind::Workspace => {
                        CatalogLocationKind::Workspace
                    }
                    vapor_client_application::CatalogLocationKind::Cache => {
                        CatalogLocationKind::Cache
                    }
                    vapor_client_application::CatalogLocationKind::Installed => {
                        CatalogLocationKind::Installed
                    }
                },
                root: location.root.display().to_string(),
                enabled: location.enabled,
                fingerprint: location.fingerprint,
                install_source: location.install_source,
            })
            .collect(),
        role,
        actions: Vec::new(),
    }
}

pub(super) fn catalog_projection(
    entry: &CatalogEntry,
    state: &ClientPersistentState,
    catalog: &[CatalogEntry],
) -> CatalogEntry {
    let mut projected = entry.clone();
    projected.role = catalog_item_role(&projected.kind);
    projected.actions = if !projected.conflicts.is_empty() {
        Vec::new()
    } else {
        match projected.role {
            CatalogItemRole::PlayableComposition => {
                let matching = state
                    .compositions
                    .compositions
                    .iter()
                    .filter(|composition| {
                        composition.root_package.as_ref().is_some_and(|root| {
                            root.id == projected.id && root.version == projected.version
                        })
                    })
                    .collect::<Vec<_>>();
                if matching.is_empty() {
                    vec![CatalogItemAction::CreateComposition]
                } else if matching
                    .iter()
                    .any(|composition| state.active_composition_id == composition.id)
                {
                    let active = matching
                        .iter()
                        .find(|composition| state.active_composition_id == composition.id);
                    if active.is_some_and(|composition| {
                        matches!(
                            &composition.installation,
                            InstallationState::Installed { .. }
                        )
                    }) {
                        vec![CatalogItemAction::Play]
                    } else if active.is_some_and(|composition| {
                        matches!(
                            &composition.installation,
                            InstallationState::OutOfDate { .. }
                                | InstallationState::Partial { .. }
                                | InstallationState::NeedsRepair { .. }
                        )
                    }) {
                        vec![CatalogItemAction::Review]
                    } else {
                        vec![CatalogItemAction::Install]
                    }
                } else {
                    vec![CatalogItemAction::SelectComposition]
                }
            }
            CatalogItemRole::RuntimeContent => {
                let addable = projected.locations.iter().any(|location| {
                    location.kind == CatalogLocationKind::Workspace
                        && state.compositions.compositions.iter().any(|composition| {
                            let Some(root) = composition.root_package.as_ref() else {
                                return false;
                            };
                            let Some(workspace) = composition.workspace.as_ref() else {
                                return false;
                            };
                            if workspace.root != location.root {
                                return false;
                            }
                            let Some(root_entry) = catalog.iter().find(|candidate| {
                                candidate.id == root.id && candidate.version == root.version
                            }) else {
                                return false;
                            };
                            let Ok(parent_kind) =
                                root_entry.kind.parse::<vapor_core::ContentKind>()
                            else {
                                return false;
                            };
                            let Ok(child_kind) = projected.kind.parse::<vapor_core::ContentKind>()
                            else {
                                return false;
                            };
                            vapor_client_application::source_dependency_allowed(
                                parent_kind,
                                child_kind,
                            )
                        })
                });
                let mut actions = Vec::new();
                if addable {
                    actions.push(CatalogItemAction::AddToComposition);
                }
                if projected
                    .locations
                    .iter()
                    .any(|location| location.kind == CatalogLocationKind::Workspace)
                {
                    actions.push(CatalogItemAction::OpenForAuthoring);
                }
                actions
            }
            CatalogItemRole::CreatorCrate => {
                if projected
                    .locations
                    .iter()
                    .any(|location| location.kind == CatalogLocationKind::Workspace)
                {
                    vec![CatalogItemAction::OpenForAuthoring]
                } else {
                    Vec::new()
                }
            }
            CatalogItemRole::Other => Vec::new(),
        }
    };
    projected
}

fn catalog_item_role(kind: &str) -> CatalogItemRole {
    match kind.parse::<vapor_core::ContentKind>() {
        Ok(vapor_core::ContentKind::Packagepack) => CatalogItemRole::PlayableComposition,
        Ok(vapor_core::ContentKind::Crate) => CatalogItemRole::CreatorCrate,
        Ok(_) => CatalogItemRole::RuntimeContent,
        Err(_) => CatalogItemRole::Other,
    }
}
