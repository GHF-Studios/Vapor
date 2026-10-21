//! Shared-service orchestration for composition install, update, and launch.

use super::*;

impl ClientSession {
    pub(super) fn start_composition_operation(
        &mut self,
        action: vapor_client_protocol::CompositionAction,
    ) -> Result<(), ServiceError> {
        self.reject_if_operation_in_progress()?;
        use vapor_client_protocol::CompositionAction as Action;
        let (composition_id, kind, selector_override) = match action {
            Action::Preview {
                composition_id,
                selector,
            } => (composition_id, CompositionOperationKind::Preview, selector),
            Action::Install { composition_id } => {
                (composition_id, CompositionOperationKind::Install, None)
            }
            Action::Verify { composition_id } => {
                (composition_id, CompositionOperationKind::Verify, None)
            }
            Action::Repair { composition_id } => {
                (composition_id, CompositionOperationKind::Repair, None)
            }
            Action::Update { composition_id } => {
                (composition_id, CompositionOperationKind::Update, None)
            }
            Action::Launch { composition_id } => {
                (composition_id, CompositionOperationKind::Launch, None)
            }
            Action::RunSource { composition_id } => {
                (composition_id, CompositionOperationKind::RunSource, None)
            }
            _ => {
                return Err(ServiceError {
                    code: ServiceErrorCode::InvalidRequest,
                    message: "This action changes composition settings and cannot start a runtime operation.".to_owned(),
                    retryable: false,
                });
            }
        };
        let composition = self
            .durable_state
            .compositions
            .compositions
            .iter()
            .find(|composition| composition.id == composition_id)
            .cloned()
            .ok_or_else(|| ServiceError {
                code: ServiceErrorCode::InvalidRequest,
                message: "The selected composition no longer exists.".to_owned(),
                retryable: true,
            })?;
        if composition.root_package.is_none() && selector_override.is_none() {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "Choose a packagepack for this composition first.".to_owned(),
                retryable: false,
            });
        }
        if matches!(kind, CompositionOperationKind::Launch)
            && matches!(
                &composition.installation,
                InstallationState::OutOfDate { .. }
            )
        {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "Review or update this composition before launching.".to_owned(),
                retryable: false,
            });
        }
        if selector_override.is_some() && !matches!(kind, CompositionOperationKind::Preview) {
            return Err(ServiceError {
                code: ServiceErrorCode::InvalidRequest,
                message: "Only preview accepts an explicit content selector.".to_owned(),
                retryable: false,
            });
        }
        if matches!(kind, CompositionOperationKind::RunSource) && composition.workspace.is_none() {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "Open the composition's workspace before running its source composition."
                    .to_owned(),
                retryable: false,
            });
        }
        self.composition_preview = None;
        let action_id = match kind {
            CompositionOperationKind::Preview => "composition.preview",
            CompositionOperationKind::Install => "composition.install",
            CompositionOperationKind::Verify => "composition.verify",
            CompositionOperationKind::Repair => "composition.repair",
            CompositionOperationKind::Update => "composition.update",
            CompositionOperationKind::Launch => "composition.launch",
            CompositionOperationKind::RunSource => "composition.run-source",
        };
        let bootstrap = self.bootstrap.clone();
        let operation_selector = selector_override.clone();
        let operation_inputs = CompositionOperationInputs::capture(&composition, kind);
        let completion_composition_id = composition_id;
        self.spawn_background_operation(
            BackgroundOperationSpec {
                domain: "studio-composition",
                action: action_id,
                thread_prefix: "studio-composition",
                failure_label: "composition operation",
                composition_id: Some(completion_composition_id.clone()),
                phase: initial_protocol_phase(kind),
            },
            move |context| {
                context.set_phase(core_operation_phase(kind));
                execute_composition_operation(
                    &bootstrap,
                    &composition,
                    kind,
                    operation_selector.as_deref(),
                )
            },
            composition_result_outcome,
            move |id, result| {
                BackgroundOperationMessage::Complete(Box::new(CompositionOperationCompletion {
                    id,
                    composition_id: completion_composition_id,
                    action: kind,
                    inputs: operation_inputs,
                    result,
                }))
            },
        )
    }

    pub(super) fn apply_composition_completion(
        &mut self,
        completion: CompositionOperationCompletion,
    ) {
        let composition_id = completion.composition_id.clone();
        let current_inputs = self
            .durable_state
            .compositions
            .compositions
            .iter()
            .find(|composition| composition.id == composition_id)
            .map(|composition| CompositionOperationInputs::capture(composition, completion.action));
        if current_inputs.as_ref() != Some(&completion.inputs) {
            self.apply_obsolete_composition_completion(completion);
            return;
        }
        let previous_compositions = self.durable_state.compositions.compositions.clone();
        let previous_composition_revision = self.durable_state.composition_revision;
        let outcome = composition_result_outcome(&completion.result);
        let mut failure = apply_operation_result_to_composition(
            &mut self.composition_preview,
            &mut self.durable_state.compositions.compositions,
            &composition_id,
            completion.action,
            completion.result,
        );
        if failure.is_none() {
            failure = outcome.as_ref().err().cloned();
        }
        if self.durable_state.compositions.compositions != previous_compositions {
            self.durable_state.composition_revision =
                self.durable_state.composition_revision.saturating_add(1);
            if let Some(store) = &self.state_store
                && let Err(error) = store.save(&self.durable_state)
            {
                self.durable_state.compositions.compositions = previous_compositions;
                self.durable_state.composition_revision = previous_composition_revision;
                failure = Some(format!("Could not save the composition result: {error}"));
            }
        }
        if let Some(message) = failure {
            self.clear_active_operation();
            self.revision.0 = self.revision.0.saturating_add(1);
            self.operation_events
                .push_back(ClientEvent::OperationChanged(OperationEvent {
                    revision: self.revision,
                    id: completion.id,
                    composition_id: Some(composition_id),
                    action: operation_action_id(completion.action).to_owned(),
                    phase: OperationPhase::Complete,
                    status: OperationStatus::Failed {
                        message: message.clone(),
                    },
                    progress: None,
                    message: Some(message),
                }));
            return;
        }
        let summary = outcome.unwrap_or_else(|_| "Completed".to_owned());
        self.clear_active_operation();
        if matches!(
            completion.action,
            CompositionOperationKind::Install
                | CompositionOperationKind::Repair
                | CompositionOperationKind::Update
                | CompositionOperationKind::Launch
                | CompositionOperationKind::RunSource
        ) {
            self.refresh_catalog();
        }
        self.revision.0 = self.revision.0.saturating_add(1);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: completion.id,
                composition_id: Some(composition_id),
                action: operation_action_id(completion.action).to_owned(),
                phase: OperationPhase::Complete,
                status: OperationStatus::Succeeded,
                progress: Some(1.0),
                message: Some(summary),
            }));
    }

    fn apply_obsolete_composition_completion(
        &mut self,
        completion: CompositionOperationCompletion,
    ) {
        let outcome = composition_result_outcome(&completion.result);
        let composition_id = completion.composition_id;
        let action = completion.action;
        let side_effecting = matches!(
            action,
            CompositionOperationKind::Install
                | CompositionOperationKind::Repair
                | CompositionOperationKind::Update
                | CompositionOperationKind::Launch
                | CompositionOperationKind::RunSource
        );
        if side_effecting {
            // The external operation may have changed installed content even
            // though its result no longer belongs to the current composition intent.
            self.refresh_catalog();
        }
        let succeeded = outcome.is_ok() && !matches!(action, CompositionOperationKind::Preview);
        let summary = if matches!(action, CompositionOperationKind::Preview) {
            match outcome {
                Ok(_) => "The preview was discarded because the composition changed while it was running."
                    .to_owned(),
                Err(error) => format!(
                    "The preview was discarded because the composition changed while it was running. {error}"
                ),
            }
        } else {
            match outcome {
                Ok(summary) => {
                    let follow_up = match action {
                        CompositionOperationKind::Install
                        | CompositionOperationKind::Repair
                        | CompositionOperationKind::Update => {
                            "Review installed content before continuing."
                        }
                        CompositionOperationKind::Verify => {
                            "The current composition's verification state was left unchanged."
                        }
                        CompositionOperationKind::Launch | CompositionOperationKind::RunSource => {
                            "The operation used the previous selection; the current composition was left unchanged."
                        }
                        CompositionOperationKind::Preview => unreachable!(),
                    };
                    format!(
                        "{summary}. This completed for the previous composition selection. {follow_up}"
                    )
                }
                Err(error) => format!(
                    "The composition changed while this operation was running, so its result was not applied to the current composition. {error}"
                ),
            }
        };
        self.clear_active_operation();
        self.revision.0 = self.revision.0.saturating_add(1);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: completion.id,
                composition_id: Some(composition_id),
                action: operation_action_id(action).to_owned(),
                phase: OperationPhase::Complete,
                status: if succeeded {
                    OperationStatus::Succeeded
                } else {
                    OperationStatus::Failed {
                        message: summary.clone(),
                    }
                },
                progress: succeeded.then_some(1.0),
                message: Some(summary),
            }));
    }

    pub(super) fn apply_composition_action(
        &mut self,
        action: CompositionAction,
    ) -> Result<(), ServiceError> {
        if let CompositionAction::CreateFromContent {
            content_id,
            content_version,
            ..
        } = &action
        {
            let Some(entry) = self.catalog.entries().iter().find(|entry| {
                entry.id == *content_id && entry.version.as_deref() == Some(content_version)
            }) else {
                return Err(ServiceError {
                    code: ServiceErrorCode::ActionUnavailable,
                    message: "This playable item is no longer available in the Library. Refresh it and try again.".to_owned(),
                    retryable: true,
                });
            };
            let projected = super::catalog::catalog_projection(
                entry,
                &self.durable_state,
                self.catalog.entries(),
            );
            let available_here = entry.locations.iter().any(|location| {
                matches!(
                    location.kind,
                    vapor_client_protocol::CatalogLocationKind::Cache
                        | vapor_client_protocol::CatalogLocationKind::Installed
                        | vapor_client_protocol::CatalogLocationKind::Workspace
                )
            });
            if !matches!(
                entry.kind.parse::<vapor_core::ContentKind>(),
                Ok(vapor_core::ContentKind::Packagepack)
            ) || !entry.conflicts.is_empty()
                || !available_here
                || !projected
                    .actions
                    .contains(&vapor_client_protocol::CatalogItemAction::CreateComposition)
            {
                return Err(ServiceError {
                    code: ServiceErrorCode::ActionUnavailable,
                    message:
                        "Only an available, conflict-free Packagepack can start a new composition."
                            .to_owned(),
                    retryable: false,
                });
            }
        }
        let may_release_content = matches!(&action, CompositionAction::Remove { .. });
        let previous_state = self.durable_state.clone();
        let application_executable = self.bootstrap.application_executable.clone();
        let fixture_mode = self.bootstrap.fixture_mode;
        let transition =
            vapor_client_domain::apply_composition_action(&mut self.durable_state, action, || {
                if fixture_mode {
                    Ok("fixture.pack".to_owned())
                } else {
                    vapor_client_application::default_packagepack_selector(
                        &application_executable,
                        "default",
                    )
                }
            });
        if let Err(message) = transition {
            self.durable_state = previous_state;
            self.active_composition_id = Some(self.durable_state.active_composition_id.clone());
            return Err(ServiceError {
                code: ServiceErrorCode::InvalidRequest,
                message,
                retryable: true,
            });
        }
        self.durable_state.composition_revision =
            previous_state.composition_revision.saturating_add(1);
        self.active_composition_id = Some(self.durable_state.active_composition_id.clone());
        if let Some(store) = &self.state_store
            && let Err(message) = store.save(&self.durable_state)
        {
            self.durable_state = previous_state;
            self.active_composition_id = Some(self.durable_state.active_composition_id.clone());
            return Err(ServiceError {
                code: ServiceErrorCode::PersistenceFailure,
                message,
                retryable: true,
            });
        }
        if may_release_content && let Err(message) = self.collect_unreferenced_auto_install() {
            self.catalog.fail(format!(
                "The composition was removed, but automatic Library cleanup could not finish: {message}"
            ));
        }
        Ok(())
    }
}

fn operation_action_id(kind: CompositionOperationKind) -> &'static str {
    match kind {
        CompositionOperationKind::Preview => "composition.preview",
        CompositionOperationKind::Install => "composition.install",
        CompositionOperationKind::Verify => "composition.verify",
        CompositionOperationKind::Repair => "composition.repair",
        CompositionOperationKind::Update => "composition.update",
        CompositionOperationKind::Launch => "composition.launch",
        CompositionOperationKind::RunSource => "composition.run-source",
    }
}

fn initial_protocol_phase(kind: CompositionOperationKind) -> OperationPhase {
    match kind {
        CompositionOperationKind::Preview => OperationPhase::Resolving,
        CompositionOperationKind::Install
        | CompositionOperationKind::Repair
        | CompositionOperationKind::Update => OperationPhase::Acquiring,
        CompositionOperationKind::Verify => OperationPhase::Verifying,
        CompositionOperationKind::Launch | CompositionOperationKind::RunSource => {
            OperationPhase::Running
        }
    }
}

fn core_operation_phase(kind: CompositionOperationKind) -> application::OperationPhase {
    match kind {
        CompositionOperationKind::Preview => application::OperationPhase::Resolving,
        CompositionOperationKind::Install
        | CompositionOperationKind::Repair
        | CompositionOperationKind::Update => application::OperationPhase::Acquiring,
        CompositionOperationKind::Verify => application::OperationPhase::Verifying,
        CompositionOperationKind::Launch | CompositionOperationKind::RunSource => {
            application::OperationPhase::Running
        }
    }
}

pub(super) fn protocol_phase(phase: application::OperationPhase) -> OperationPhase {
    match phase {
        application::OperationPhase::Queued => OperationPhase::Queued,
        application::OperationPhase::Resolving => OperationPhase::Resolving,
        application::OperationPhase::Acquiring => OperationPhase::Acquiring,
        application::OperationPhase::Installing => OperationPhase::Installing,
        application::OperationPhase::Verifying => OperationPhase::Verifying,
        application::OperationPhase::Building => OperationPhase::Building,
        application::OperationPhase::Running => OperationPhase::Running,
        application::OperationPhase::Staging => OperationPhase::Staging,
        application::OperationPhase::Publishing => OperationPhase::Publishing,
        application::OperationPhase::Finalizing => OperationPhase::Finalizing,
    }
}

pub(super) fn protocol_status(
    status: application::OperationStatus,
) -> (OperationStatus, Option<String>) {
    match status {
        application::OperationStatus::Running => (OperationStatus::Running, None),
        application::OperationStatus::Cancelling => (OperationStatus::Cancelling, None),
        application::OperationStatus::Succeeded { summary } => {
            (OperationStatus::Succeeded, Some(summary))
        }
        application::OperationStatus::Failed { message } => (
            OperationStatus::Failed {
                message: message.clone(),
            },
            Some(message),
        ),
        application::OperationStatus::Cancelled => (OperationStatus::Cancelled, None),
    }
}

fn composition_result_outcome(result: &CompositionOperationResult) -> Result<String, String> {
    match result {
        CompositionOperationResult::Preview(Ok(preview)) => Ok(format!(
            "Resolved {} composition items{}",
            preview.nodes.len(),
            if preview.blockers.is_empty() {
                String::new()
            } else {
                format!(" with {} blockers", preview.blockers.len())
            }
        )),
        CompositionOperationResult::Install(Ok(composition))
        | CompositionOperationResult::Repair(Ok(composition))
        | CompositionOperationResult::Update(Ok(composition)) => Ok(format!(
            "Composition installed with {} content items",
            composition.graph.nodes.len()
        )),
        CompositionOperationResult::Verify(Ok(check)) if check.ok => {
            if let Some(update) = &check.update_required {
                Ok(format!(
                    "Verified {} content items; update required: {update}",
                    check.detail.len()
                ))
            } else {
                Ok(format!("Verified {} content items", check.detail.len()))
            }
        }
        CompositionOperationResult::Verify(Ok(check)) => Err(check.detail.join("; ")),
        CompositionOperationResult::Launch(Ok(result))
        | CompositionOperationResult::RunSource(Ok(result)) => {
            if result.composition_graph_matches {
                result
                    .launch_error
                    .clone()
                    .map_or_else(|| Ok("Composition launched".to_owned()), Err)
            } else {
                Err(result.launch_error.clone().unwrap_or_else(|| {
                    "The installed composition differs from the composition.".to_owned()
                }))
            }
        }
        CompositionOperationResult::Preview(Err(error))
        | CompositionOperationResult::Install(Err(error))
        | CompositionOperationResult::Verify(Err(error))
        | CompositionOperationResult::Repair(Err(error))
        | CompositionOperationResult::Update(Err(error))
        | CompositionOperationResult::Launch(Err(error))
        | CompositionOperationResult::RunSource(Err(error)) => Err(error.clone()),
    }
}

fn execute_composition_operation(
    bootstrap: &ClientHostConfig,
    composition: &Composition,
    kind: CompositionOperationKind,
    selector_override: Option<&str>,
) -> CompositionOperationResult {
    let selector = selector_override
        .map(ToOwned::to_owned)
        .or_else(|| {
            composition.root_package.as_ref().map(|root| {
                root.version.as_deref().map_or_else(
                    || root.id.clone(),
                    |version| format!("{}@{version}", root.id),
                )
            })
        })
        .expect("validated before spawn");
    let workspace = composition
        .workspace
        .as_ref()
        .map(|workspace| workspace.root.as_path());
    if bootstrap.fixture_mode {
        return fixture_composition_result(composition, selector, kind);
    }
    let executable = &bootstrap.application_executable;
    let graph_selectors = composition
        .resolved_graph
        .as_ref()
        .map(|graph| {
            graph
                .nodes
                .iter()
                .map(|node| {
                    node.content.version.as_deref().map_or_else(
                        || node.content.id.clone(),
                        |version| format!("{}@{version}", node.content.id),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![selector.clone()]);
    match kind {
        CompositionOperationKind::Preview => CompositionOperationResult::Preview(
            vapor_client_application::preview_composition(executable, workspace, &selector),
        ),
        CompositionOperationKind::Install => CompositionOperationResult::Install(
            vapor_client_application::install_composition(executable, workspace, &selector),
        ),
        CompositionOperationKind::Verify => CompositionOperationResult::Verify(
            vapor_client_application::verify_installation(executable, &selector, &graph_selectors),
        ),
        CompositionOperationKind::Repair => {
            CompositionOperationResult::Repair(vapor_client_application::repair_composition(
                executable,
                workspace,
                &selector,
                &graph_selectors,
            ))
        }
        CompositionOperationKind::Update => CompositionOperationResult::Update(
            vapor_client_application::update_composition(executable, workspace, &selector),
        ),
        CompositionOperationKind::Launch => {
            CompositionOperationResult::Launch(vapor_client_application::run_installed_composition(
                executable,
                workspace,
                &selector,
                composition.resolved_graph.as_ref(),
                &composition.launch.arguments,
            ))
        }
        CompositionOperationKind::RunSource => {
            let Some(workspace) = workspace else {
                return CompositionOperationResult::RunSource(Err(
                    "Open the composition's workspace before running its source composition."
                        .to_owned(),
                ));
            };
            CompositionOperationResult::RunSource(vapor_client_application::run_source_composition(
                executable,
                workspace,
                &selector,
                &composition.launch.arguments,
            ))
        }
    }
}

pub(super) fn fixture_composition_result(
    composition: &Composition,
    selector: String,
    kind: CompositionOperationKind,
) -> CompositionOperationResult {
    use vapor_client_application::{
        CompositionPreview, CompositionPreviewNode, CompositionPreviewSource, InstallationCheck,
        InstalledArtifact, InstalledComposition,
    };
    let root = composition.root_package.clone().unwrap_or_else(|| {
        let (id, version) = selector.split_once('@').map_or_else(
            || (selector.as_str(), None),
            |(id, version)| (id, Some(version)),
        );
        ContentReference {
            id: id.to_owned(),
            version: version.map(ToOwned::to_owned),
        }
    });
    let preview = CompositionPreview {
        selector,
        nodes: vec![CompositionPreviewNode {
            id: root.id.clone(),
            version: root.version.clone(),
            kind: "packagepack".to_owned(),
            published_file_id: None,
            source: if matches!(
                &composition.installation,
                InstallationState::Installed { .. }
            ) {
                CompositionPreviewSource::Installed
            } else {
                CompositionPreviewSource::Workspace
            },
            already_installed: matches!(
                &composition.installation,
                InstallationState::Installed { .. }
            ),
            dependencies: Vec::new(),
        }],
        blockers: Vec::new(),
        warnings: vec![
            "Fixture mode: no content provider, Steam, or game process was used.".into(),
        ],
    };
    let graph = composition.resolved_graph.clone().unwrap_or_else(|| {
        let engine = ContentReference {
            id: "fixture.engine".to_owned(),
            version: Some("1.0.0".to_owned()),
        };
        vapor_client_protocol::ResolvedCompositionGraph {
            root: root.clone(),
            effective_engine: engine.clone(),
            effective_game: None,
            nodes: vec![
                vapor_client_protocol::ResolvedNodeIdentity {
                    content: root.clone(),
                    kind: "packagepack".to_owned(),
                    fingerprint: Some("fixture-root".to_owned()),
                    source: Some("fixture".to_owned()),
                    dependencies: vec![vapor_client_protocol::ResolvedDependencyIdentity {
                        binding: "engine".to_owned(),
                        content: engine.clone(),
                        version_requirement: None,
                    }],
                },
                vapor_client_protocol::ResolvedNodeIdentity {
                    content: engine,
                    kind: "engine".to_owned(),
                    fingerprint: Some("fixture-engine".to_owned()),
                    source: Some("fixture".to_owned()),
                    dependencies: Vec::new(),
                },
            ],
        }
    });
    let composition = InstalledComposition {
        artifacts: graph
            .nodes
            .iter()
            .map(|node| InstalledArtifact {
                id: node.content.id.clone(),
                version: node.content.version.clone(),
                root: PathBuf::from(format!("/fixture/installed/{}", node.content.id)),
                fingerprint: node.fingerprint.clone().unwrap_or_default(),
            })
            .collect(),
        graph: graph.clone(),
    };
    match kind {
        CompositionOperationKind::Preview => CompositionOperationResult::Preview(Ok(preview)),
        CompositionOperationKind::Install => CompositionOperationResult::Install(Ok(composition)),
        CompositionOperationKind::Verify => {
            CompositionOperationResult::Verify(Ok(InstallationCheck {
                ok: true,
                detail: vec!["Fixture installation verified".into()],
                artifacts: composition.artifacts.clone(),
                graph: Some(graph),
                update_required: None,
            }))
        }
        CompositionOperationKind::Repair => CompositionOperationResult::Repair(Ok(composition)),
        CompositionOperationKind::Update => CompositionOperationResult::Update(Ok(composition)),
        CompositionOperationKind::Launch => {
            CompositionOperationResult::Launch(Ok(vapor_client_application::CompositionRunResult {
                composition,
                composition_graph_matches: true,
                launch_error: None,
            }))
        }
        CompositionOperationKind::RunSource => CompositionOperationResult::RunSource(Ok(
            vapor_client_application::CompositionRunResult {
                composition,
                composition_graph_matches: true,
                launch_error: None,
            },
        )),
    }
}

pub(super) fn apply_operation_result_to_composition(
    preview: &mut Option<vapor_client_protocol::CompositionPreviewSnapshot>,
    compositions: &mut [Composition],
    composition_id: &str,
    kind: CompositionOperationKind,
    result: CompositionOperationResult,
) -> Option<String> {
    use vapor_client_protocol::InstallationState;
    let Some(composition) = compositions
        .iter_mut()
        .find(|composition| composition.id == composition_id)
    else {
        return Some("The composition was removed while its operation was running.".to_owned());
    };
    match result {
        CompositionOperationResult::Preview(Ok(value)) => {
            *preview = Some(preview_snapshot(composition_id, value));
        }
        CompositionOperationResult::Preview(Err(error)) => return Some(error),
        CompositionOperationResult::Install(Ok(installed))
        | CompositionOperationResult::Repair(Ok(installed))
        | CompositionOperationResult::Update(Ok(installed)) => {
            let graph = installed.graph;
            if matches!(kind, CompositionOperationKind::Update) {
                composition.root_package = Some(graph.root.clone());
            }
            if matches!(kind, CompositionOperationKind::Repair)
                && composition.resolved_graph.as_ref().is_some_and(|saved| {
                    !vapor_client_application::resolved_graph_matches(saved, &graph)
                })
            {
                composition.installation = InstallationState::OutOfDate {
                    detail: "Repair restored valid content, but its graph differs from this composition's saved setup.".into(),
                    actual_graph: graph,
                    artifacts: installed_records(installed.artifacts),
                    verified: false,
                };
            } else {
                composition.resolved_graph = Some(graph.clone());
                composition.installation = installed_state(graph, installed.artifacts);
            }
        }
        CompositionOperationResult::Verify(Ok(check)) => {
            let update_required = check.update_required;
            let artifacts = protocol_artifacts(check.artifacts);
            if !check.ok {
                composition.installation = InstallationState::NeedsRepair {
                    detail: check.detail.join("; "),
                    artifacts,
                };
            } else if let Some(graph) = check.graph {
                if let Some(detail) = update_required {
                    composition.installation = InstallationState::OutOfDate {
                        detail,
                        actual_graph: graph,
                        artifacts,
                        verified: true,
                    };
                } else if composition.resolved_graph.as_ref().is_some_and(|saved| {
                    !vapor_client_application::resolved_graph_matches(saved, &graph)
                }) {
                    composition.installation = InstallationState::OutOfDate {
                        detail:
                            "Verified installed content differs from this composition's saved setup."
                                .into(),
                        actual_graph: graph,
                        artifacts,
                        verified: true,
                    };
                } else {
                    composition.resolved_graph = Some(graph.clone());
                    composition.installation = InstallationState::Installed {
                        artifacts,
                        graph_fingerprint: vapor_client_application::fingerprint_resolved_graph(
                            &graph,
                        ),
                    };
                }
            } else {
                composition.installation = InstallationState::NeedsRepair {
                    detail: "Verification completed without a resolved installed graph.".into(),
                    artifacts,
                };
            }
        }
        CompositionOperationResult::Verify(Err(error)) => {
            let artifacts = installation_artifacts(&composition.installation);
            composition.installation = InstallationState::NeedsRepair {
                detail: error.clone(),
                artifacts,
            };
            return Some(error);
        }
        CompositionOperationResult::Launch(Ok(result))
        | CompositionOperationResult::RunSource(Ok(result)) => {
            let graph = result.composition.graph;
            let artifacts = protocol_artifacts(result.composition.artifacts);
            if result.composition_graph_matches {
                composition.resolved_graph = Some(graph.clone());
                composition.installation = InstallationState::Installed {
                    artifacts,
                    graph_fingerprint: vapor_client_application::fingerprint_resolved_graph(&graph),
                };
            } else {
                composition.installation = InstallationState::OutOfDate {
                    detail: result.launch_error.clone().unwrap_or_else(|| {
                        "The installed composition differs from this composition.".into()
                    }),
                    actual_graph: graph,
                    artifacts,
                    verified: false,
                };
            }
            if result.launch_error.is_some() || !result.composition_graph_matches {
                return result.launch_error.or_else(|| {
                    Some("The installed composition differs from this composition.".into())
                });
            }
        }
        CompositionOperationResult::Install(Err(error))
        | CompositionOperationResult::Repair(Err(error))
        | CompositionOperationResult::Update(Err(error)) => {
            composition.installation = InstallationState::Partial {
                detail: error.clone(),
            };
            return Some(error);
        }
        CompositionOperationResult::Launch(Err(error))
        | CompositionOperationResult::RunSource(Err(error)) => return Some(error),
    }
    None
}

fn preview_snapshot(
    composition_id: &str,
    preview: vapor_client_application::CompositionPreview,
) -> vapor_client_protocol::CompositionPreviewSnapshot {
    vapor_client_protocol::CompositionPreviewSnapshot {
        composition_id: composition_id.to_owned(),
        selector: preview.selector,
        nodes: preview.nodes,
        blockers: preview.blockers,
        warnings: preview.warnings,
    }
}

fn installed_state(
    graph: vapor_client_protocol::ResolvedCompositionGraph,
    artifacts: Vec<vapor_client_application::InstalledArtifact>,
) -> vapor_client_protocol::InstallationState {
    vapor_client_protocol::InstallationState::Installed {
        graph_fingerprint: vapor_client_application::fingerprint_resolved_graph(&graph),
        artifacts: installed_records(artifacts),
    }
}

fn installed_records(
    artifacts: Vec<vapor_client_application::InstalledArtifact>,
) -> Vec<vapor_client_protocol::InstalledArtifactRecord> {
    artifacts
        .into_iter()
        .map(|artifact| vapor_client_protocol::InstalledArtifactRecord {
            id: artifact.id,
            version: artifact.version,
            root: artifact.root,
            fingerprint: artifact.fingerprint,
        })
        .collect()
}

fn protocol_artifacts(
    artifacts: Vec<vapor_client_application::InstalledArtifact>,
) -> Vec<vapor_client_protocol::InstalledArtifactRecord> {
    installed_records(artifacts)
}

fn installation_artifacts(
    state: &InstallationState,
) -> Vec<vapor_client_protocol::InstalledArtifactRecord> {
    match state {
        InstallationState::Installed { artifacts, .. }
        | InstallationState::NeedsRepair { artifacts, .. }
        | InstallationState::OutOfDate { artifacts, .. } => artifacts.clone(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod update_version_tests {
    use super::{
        CompositionOperationKind, apply_operation_result_to_composition, fixture_composition_result,
    };
    use vapor_client_protocol::{Composition, ContentReference};

    #[test]
    fn successful_update_advances_the_composition_root_version() {
        let mut saved = Composition::default_composition();
        saved.id = "composition-1".to_owned();
        saved.root_package = Some(ContentReference {
            id: "ghf-studios/example/packagepack".to_owned(),
            version: Some("0.1.1".to_owned()),
        });

        let mut updated = saved.clone();
        updated.root_package = Some(ContentReference {
            id: "ghf-studios/example/packagepack".to_owned(),
            version: Some("0.5.0".to_owned()),
        });
        let result = fixture_composition_result(
            &updated,
            "ghf-studios/example/packagepack@0.5.0".to_owned(),
            CompositionOperationKind::Update,
        );
        let mut preview = None;
        let mut compositions = vec![saved];

        let failure = apply_operation_result_to_composition(
            &mut preview,
            &mut compositions,
            "composition-1",
            CompositionOperationKind::Update,
            result,
        );

        assert_eq!(failure, None);
        assert_eq!(
            compositions[0]
                .root_package
                .as_ref()
                .and_then(|root| root.version.as_deref()),
            Some("0.5.0")
        );
    }
}
