//! Shared-service orchestration for workspace workflows.

use super::*;

impl ClientSession {
    pub(super) fn start_workspace_operation(
        &mut self,
        action: WorkspaceAction,
    ) -> Result<(), ServiceError> {
        self.reject_if_operation_in_progress()?;
        let is_superworkspace_operation = matches!(
            &action,
            WorkspaceAction::DiscoverSuperworkspaceRepositories
                | WorkspaceAction::AcquireSuperworkspaceRepositories { .. }
        );
        let run_configuration_root = match &action {
            WorkspaceAction::RunConfiguration { workspace_root, .. } => {
                Some(PathBuf::from(workspace_root))
            }
            _ => None,
        };
        if run_configuration_root.is_some() && self.bootstrap.fixture_mode {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "Fixture sessions cannot run source workspaces.".to_owned(),
                retryable: false,
            });
        }
        if is_superworkspace_operation && self.bootstrap.fixture_mode {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message:
                    "Fixture sessions cannot access the real source registry or Superworkspace."
                        .to_owned(),
                retryable: false,
            });
        }
        let root = if is_superworkspace_operation {
            configured_superworkspace_root().map_err(|message| ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message,
                retryable: true,
            })?
        } else if let Some(requested) = run_configuration_root {
            if !requested.is_absolute() {
                return Err(invalid_workspace_request(
                    "A Run Configuration must name an absolute workspace path.",
                ));
            }
            requested.canonicalize().map_err(|error| {
                invalid_workspace_request(&format!("{}: {error}", requested.display()))
            })?
        } else {
            self.bootstrap
                .workspace_root
                .clone()
                .ok_or_else(|| ServiceError {
                    code: ServiceErrorCode::ActionUnavailable,
                    message: "Open a workspace before using its file editor.".to_owned(),
                    retryable: false,
                })?
        };
        if self.bootstrap.fixture_mode && !is_isolated_fixture_workspace(&root) {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "Fixture mode can edit only temporary test workspaces; it cannot access a player workspace.".to_owned(),
                retryable: false,
            });
        }
        let kind = match action {
            WorkspaceAction::DiscoverSuperworkspaceRepositories => {
                WorkspaceOperationKind::DiscoverSuperworkspaceRepositories
            }
            WorkspaceAction::AcquireSuperworkspaceRepositories { repository_names } => {
                if repository_names.is_empty()
                    || repository_names.len() > 64
                    || repository_names
                        .iter()
                        .any(|name| name.trim().is_empty() || name.len() > 128)
                {
                    return Err(invalid_workspace_request(
                        "Choose between one and 64 valid source repositories.",
                    ));
                }
                WorkspaceOperationKind::AcquireSuperworkspaceRepositories(repository_names)
            }
            WorkspaceAction::ListFiles { path } => {
                if !path.is_empty() && path != "." {
                    return Err(invalid_workspace_request(
                        "Workspace file discovery currently starts at the workspace root.",
                    ));
                }
                WorkspaceOperationKind::ListFiles
            }
            WorkspaceAction::OpenFile { path } => {
                WorkspaceOperationKind::OpenFile(validate_workspace_relative_path(&path)?)
            }
            WorkspaceAction::SaveFile {
                path,
                expected_fingerprint,
                contents,
            } => {
                if contents.len() > 4 * 1024 * 1024 {
                    return Err(invalid_workspace_request(
                        "Workspace editor saves are limited to 4 MiB per file.",
                    ));
                }
                WorkspaceOperationKind::SaveFile(Box::new(WorkspaceFileSave {
                    path: validate_workspace_relative_path(&path)?,
                    expected_fingerprint,
                    contents,
                }))
            }
            WorkspaceAction::CreateFile { path, contents } => {
                if !contents.is_empty() {
                    return Err(invalid_workspace_request(
                        "New workspace files are created empty; edit and save them after creation.",
                    ));
                }
                WorkspaceOperationKind::CreateFile(validate_workspace_relative_path(&path)?)
            }
            WorkspaceAction::Validate => WorkspaceOperationKind::Validate,
            WorkspaceAction::BuildDocumentation => WorkspaceOperationKind::BuildDocumentation,
            WorkspaceAction::PublishDocumentation => WorkspaceOperationKind::PublishDocumentation,
            WorkspaceAction::RunTool {
                operation,
                project_selection,
                target_triples,
            } => {
                super::validate_runtime_target_matrix(&target_triples)?;
                if matches!(&project_selection, WorkspaceProjectSelection::Named(name) if name.trim().is_empty() || name.len() > 128)
                {
                    return Err(invalid_workspace_request(
                        "Choose a valid Cargo project name of 1 to 128 bytes.",
                    ));
                }
                WorkspaceOperationKind::RunTool(operation, project_selection, target_triples)
            }
            WorkspaceAction::RunConfiguration {
                workspace_root: _,
                configuration,
                profiling,
            } => {
                if configuration.trim().is_empty() || configuration.len() > 128 {
                    return Err(invalid_workspace_request(
                        "Choose a Run Configuration name of 1 to 128 bytes.",
                    ));
                }
                WorkspaceOperationKind::RunConfiguration {
                    name: configuration,
                    profiling,
                }
            }
            WorkspaceAction::DeployApplication {
                target_triples,
                skip_documentation,
            } => {
                super::validate_runtime_target_matrix(&target_triples)?;
                WorkspaceOperationKind::DeployApplication {
                    target_triples,
                    skip_documentation,
                }
            }
            WorkspaceAction::PackageApplication { target_triples } => {
                super::validate_runtime_target_matrix(&target_triples)?;
                if target_triples.is_empty() {
                    return Err(invalid_workspace_request(
                        "Choose at least one runtime target before packaging the application.",
                    ));
                }
                WorkspaceOperationKind::PackageApplication { target_triples }
            }
            WorkspaceAction::Clean {
                confirmed_workspace_root,
            } => {
                validate_clean_confirmation(&root, &confirmed_workspace_root)?;
                WorkspaceOperationKind::Clean
            }
            WorkspaceAction::Remember => WorkspaceOperationKind::Remember,
            WorkspaceAction::AdoptProject { path } => {
                let candidate = PathBuf::from(path);
                let candidate = if candidate.is_absolute() {
                    candidate
                } else {
                    root.join(candidate)
                };
                let canonical = candidate.canonicalize().map_err(|error| {
                    invalid_workspace_request(&format!("{}: {error}", candidate.display()))
                })?;
                let canonical_root = root.canonicalize().map_err(|error| {
                    invalid_workspace_request(&format!("{}: {error}", root.display()))
                })?;
                if !canonical.starts_with(&canonical_root) || !canonical.is_dir() {
                    return Err(invalid_workspace_request(
                        "Choose a content project folder inside the active workspace.",
                    ));
                }
                WorkspaceOperationKind::AdoptProject(canonical)
            }
            WorkspaceAction::UnregisterProject { project_id } => {
                if project_id.trim().is_empty() || project_id.len() > 256 {
                    return Err(invalid_workspace_request(
                        "Choose a valid content project ID.",
                    ));
                }
                WorkspaceOperationKind::UnregisterProject(project_id)
            }
            _ => {
                return Err(ServiceError {
                    code: ServiceErrorCode::ActionUnavailable,
                    message: "This workspace action is not a file editor operation.".to_owned(),
                    retryable: false,
                });
            }
        };
        if self.bootstrap.fixture_mode
            && !matches!(
                &kind,
                WorkspaceOperationKind::ListFiles
                    | WorkspaceOperationKind::OpenFile(_)
                    | WorkspaceOperationKind::SaveFile(_)
                    | WorkspaceOperationKind::CreateFile(_)
                    | WorkspaceOperationKind::RunTool(..)
                    | WorkspaceOperationKind::Clean
            )
        {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "This operation needs the real Vapor installation and is unavailable in fixture mode.".to_owned(),
                retryable: false,
            });
        }
        let action_id = workspace_operation_action_id(&kind);
        let action_for_worker = kind.clone();
        let root_for_worker = root.clone();
        let executable_for_worker = self.bootstrap.application_executable.clone();
        let fixture_mode = self.bootstrap.fixture_mode;
        self.spawn_background_operation(
            BackgroundOperationSpec {
                domain: "studio-workspace",
                action: action_id,
                thread_prefix: "studio-workspace",
                failure_label: "workspace file operation",
                composition_id: None,
                phase: workspace_operation_phase(&kind),
            },
            move |context| {
                execute_workspace_operation(
                    &executable_for_worker,
                    &root_for_worker,
                    action_for_worker,
                    fixture_mode,
                    context,
                )
            },
            workspace_result_outcome,
            move |id, result| {
                BackgroundOperationMessage::CompleteWorkspace(Box::new(
                    WorkspaceOperationCompletion {
                        id,
                        action: kind,
                        root,
                        result,
                    },
                ))
            },
        )
    }

    pub(super) fn apply_workspace_completion(&mut self, completion: WorkspaceOperationCompletion) {
        let source_context_is_current = if matches!(
            &completion.action,
            WorkspaceOperationKind::DiscoverSuperworkspaceRepositories
                | WorkspaceOperationKind::AcquireSuperworkspaceRepositories(_)
        ) {
            configured_superworkspace_root().ok().as_deref() == Some(completion.root.as_path())
        } else {
            self.bootstrap.workspace_root.as_deref() == Some(&completion.root)
                || matches!(
                    &completion.action,
                    WorkspaceOperationKind::RunConfiguration { .. }
                        | WorkspaceOperationKind::DeployApplication { .. }
                        | WorkspaceOperationKind::PackageApplication { .. }
                        | WorkspaceOperationKind::PublishDocumentation
                )
        };
        let mut result = completion.result;
        if !source_context_is_current {
            result =
                Err("The selected source folder changed while the operation was running.".into());
        }
        let mut workspace_file_result = None;
        let mut operation_succeeded = false;
        let message = match result {
            Ok(WorkspaceOperationResult::SourceRepositories(repositories)) => {
                operation_succeeded = true;
                self.superworkspace_sources = repositories;
                self.superworkspace_sources_loaded = true;
                self.superworkspace_sources_error = None;
                format!(
                    "Found {} source repositories in the Vapor registry.",
                    self.superworkspace_sources.len()
                )
            }
            Ok(WorkspaceOperationResult::AcquiredSourceRepositories(repository_ids)) => {
                operation_succeeded = true;
                format!(
                    "Added {} source repositories to the Vapor source folder.",
                    repository_ids.len()
                )
            }
            Ok(WorkspaceOperationResult::Listed(files)) => {
                operation_succeeded = true;
                let files = files
                    .into_iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                let message = format!("Found {} workspace files.", files.len());
                workspace_file_result = Some(WorkspaceFileResult::Listed {
                    operation_id: completion.id.clone(),
                    files,
                });
                message
            }
            Ok(WorkspaceOperationResult::Opened(opened)) => {
                operation_succeeded = true;
                let path = opened.path.to_string_lossy().into_owned();
                workspace_file_result = Some(WorkspaceFileResult::Opened {
                    operation_id: completion.id.clone(),
                    path: path.clone(),
                    contents: opened.contents,
                    fingerprint: opened.fingerprint,
                });
                format!("Opened {path}.")
            }
            Ok(WorkspaceOperationResult::Saved { path, fingerprint }) => {
                operation_succeeded = true;
                let path = path.to_string_lossy().into_owned();
                self.workshop_review = None;
                self.workshop_batch_review = None;
                self.application_release_review = None;
                self.application_release_snapshot = None;
                if is_content_manifest_path(Path::new(&path)) {
                    self.refresh_catalog();
                }
                workspace_file_result = Some(WorkspaceFileResult::Saved {
                    operation_id: completion.id.clone(),
                    path: path.clone(),
                    fingerprint,
                });
                format!("Saved {path}.")
            }
            Ok(WorkspaceOperationResult::Created(path)) => {
                operation_succeeded = true;
                let path = path.to_string_lossy().into_owned();
                self.workshop_review = None;
                self.workshop_batch_review = None;
                self.application_release_review = None;
                self.application_release_snapshot = None;
                workspace_file_result = Some(WorkspaceFileResult::Created {
                    operation_id: completion.id.clone(),
                    path: path.clone(),
                });
                format!("Created {path}.")
            }
            Ok(WorkspaceOperationResult::CompletedWithSummary(summary)) => {
                operation_succeeded = true;
                summary
            }
            Ok(WorkspaceOperationResult::Completed) => {
                operation_succeeded = true;
                if matches!(
                    &completion.action,
                    WorkspaceOperationKind::AdoptProject(_)
                        | WorkspaceOperationKind::UnregisterProject(_)
                ) {
                    self.workshop_review = None;
                    self.workshop_batch_review = None;
                    self.application_release_review = None;
                    self.application_release_snapshot = None;
                    self.refresh_catalog();
                }
                workspace_file_result = Some(WorkspaceFileResult::Completed {
                    operation_id: completion.id.clone(),
                });
                match &completion.action {
                    WorkspaceOperationKind::RunConfiguration { name, .. } => {
                        workspace_file_result = None;
                        format!("Run Configuration `{name}` finished.")
                    }
                    WorkspaceOperationKind::DeployApplication { .. } => {
                        "Application deployment completed.".to_owned()
                    }
                    WorkspaceOperationKind::PackageApplication { .. } => {
                        "Application package completed.".to_owned()
                    }
                    WorkspaceOperationKind::Validate => {
                        "Workspace validation completed.".to_owned()
                    }
                    WorkspaceOperationKind::BuildDocumentation => {
                        "Workspace documentation build completed.".to_owned()
                    }
                    WorkspaceOperationKind::PublishDocumentation => {
                        "Workspace documentation published.".to_owned()
                    }
                    WorkspaceOperationKind::RunTool(operation, _, _) => format!(
                        "Workspace {} completed.",
                        match operation {
                            WorkspaceToolOperation::Format => "formatting",
                            WorkspaceToolOperation::Check => "check",
                            WorkspaceToolOperation::Test => "tests",
                            WorkspaceToolOperation::Build => "build",
                            WorkspaceToolOperation::BuildRelease => "release build",
                            WorkspaceToolOperation::Validate => "validation",
                        }
                    ),
                    WorkspaceOperationKind::Clean => {
                        "Generated workspace build files were removed.".to_owned()
                    }
                    WorkspaceOperationKind::AdoptProject(_) => {
                        "Content project added to the workspace.".to_owned()
                    }
                    WorkspaceOperationKind::UnregisterProject(_) => {
                        "Content project removed from discovery.".to_owned()
                    }
                    _ => "Workspace operation completed.".to_owned(),
                }
            }
            Ok(WorkspaceOperationResult::Unregistered(identity)) => {
                operation_succeeded = true;
                self.workshop_review = None;
                self.workshop_batch_review = None;
                self.application_release_review = None;
                self.application_release_snapshot = None;
                self.refresh_catalog();
                workspace_file_result = Some(WorkspaceFileResult::UnregisteredContent {
                    operation_id: completion.id.clone(),
                    content_id: identity.id,
                    version: identity.version,
                });
                "Content project removed from discovery.".to_owned()
            }
            Ok(WorkspaceOperationResult::Remembered(roots)) => {
                operation_succeeded = true;
                let roots = roots
                    .into_iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                workspace_file_result = Some(WorkspaceFileResult::RememberedSources {
                    operation_id: completion.id.clone(),
                    roots: roots.clone(),
                });
                format!(
                    "Registered {} workspace{}.",
                    roots.len(),
                    if roots.len() == 1 { "" } else { "s" }
                )
            }
            Err(error) => {
                if matches!(
                    &completion.action,
                    WorkspaceOperationKind::DiscoverSuperworkspaceRepositories
                        | WorkspaceOperationKind::AcquireSuperworkspaceRepositories(_)
                ) {
                    self.superworkspace_sources_error = Some(error.clone());
                }
                error
            }
        };
        self.clear_active_operation();
        self.revision.0 = self.revision.0.saturating_add(1);
        let succeeded = operation_succeeded;
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: completion.id.clone(),
                composition_id: None,
                action: workspace_operation_action_id(&completion.action).to_owned(),
                phase: OperationPhase::Complete,
                status: if succeeded {
                    OperationStatus::Succeeded
                } else {
                    OperationStatus::Failed {
                        message: message.clone(),
                    }
                },
                progress: Some(1.0),
                message: Some(message),
            }));
        if let Some(result) = workspace_file_result {
            self.operation_events
                .push_back(ClientEvent::WorkspaceFileResult(Box::new(result)));
        }
    }
}

fn validate_workspace_relative_path(path: &str) -> Result<PathBuf, ServiceError> {
    let path = PathBuf::from(path);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(invalid_workspace_request(
            "Choose a file path relative to the open workspace.",
        ));
    }
    Ok(path)
}

fn invalid_workspace_request(message: &str) -> ServiceError {
    ServiceError {
        code: ServiceErrorCode::InvalidRequest,
        message: message.to_owned(),
        retryable: false,
    }
}

fn validate_clean_confirmation(
    active_root: &Path,
    confirmed_root: &str,
) -> Result<(), ServiceError> {
    if confirmed_root.trim().is_empty() {
        return Err(invalid_workspace_request(
            "Confirm cleanup by naming the active workspace.",
        ));
    }
    let confirmed = PathBuf::from(confirmed_root.trim());
    let confirmed = confirmed.canonicalize().map_err(|error| {
        invalid_workspace_request(&format!(
            "Could not verify the confirmed workspace '{}': {error}",
            confirmed.display()
        ))
    })?;
    let active = active_root.canonicalize().map_err(|error| {
        invalid_workspace_request(&format!(
            "Could not verify the active workspace '{}': {error}",
            active_root.display()
        ))
    })?;
    if confirmed == active {
        Ok(())
    } else {
        Err(invalid_workspace_request(
            "Cleanup confirmation does not match the active workspace.",
        ))
    }
}

fn workspace_operation_action_id(kind: &WorkspaceOperationKind) -> &'static str {
    match kind {
        WorkspaceOperationKind::DiscoverSuperworkspaceRepositories => {
            "workspace.discover-superworkspace-repositories"
        }
        WorkspaceOperationKind::AcquireSuperworkspaceRepositories(_) => {
            "workspace.acquire-superworkspace-repositories"
        }
        WorkspaceOperationKind::ListFiles => "workspace.list-files",
        WorkspaceOperationKind::OpenFile(_) => "workspace.open-file",
        WorkspaceOperationKind::SaveFile(_) => "workspace.save-file",
        WorkspaceOperationKind::CreateFile(_) => "workspace.create-file",
        WorkspaceOperationKind::Validate => "workspace.validate",
        WorkspaceOperationKind::BuildDocumentation => "workspace.build-documentation",
        WorkspaceOperationKind::PublishDocumentation => "workspace.publish-documentation",
        WorkspaceOperationKind::RunTool(WorkspaceToolOperation::Format, _, _) => "workspace.fmt",
        WorkspaceOperationKind::RunTool(WorkspaceToolOperation::Check, _, _) => "workspace.check",
        WorkspaceOperationKind::RunTool(WorkspaceToolOperation::Test, _, _) => "workspace.test",
        WorkspaceOperationKind::RunTool(WorkspaceToolOperation::Build, _, _) => "workspace.build",
        WorkspaceOperationKind::RunTool(WorkspaceToolOperation::BuildRelease, _, _) => {
            "workspace.build-release"
        }
        WorkspaceOperationKind::RunTool(WorkspaceToolOperation::Validate, _, _) => {
            "workspace.validate"
        }
        WorkspaceOperationKind::RunConfiguration { .. } => "workspace.run-configuration",
        WorkspaceOperationKind::DeployApplication { .. } => "workspace.deploy-application",
        WorkspaceOperationKind::PackageApplication { .. } => "workspace.package-application",
        WorkspaceOperationKind::Clean => "workspace.clean",
        WorkspaceOperationKind::Remember => "workspace.remember",
        WorkspaceOperationKind::AdoptProject(_) => "workspace.adopt-project",
        WorkspaceOperationKind::UnregisterProject(_) => "workspace.unregister-project",
    }
}

fn workspace_operation_phase(kind: &WorkspaceOperationKind) -> OperationPhase {
    match kind {
        WorkspaceOperationKind::DiscoverSuperworkspaceRepositories => OperationPhase::Resolving,
        WorkspaceOperationKind::AcquireSuperworkspaceRepositories(_) => OperationPhase::Acquiring,
        WorkspaceOperationKind::ListFiles | WorkspaceOperationKind::OpenFile(_) => {
            OperationPhase::Resolving
        }
        WorkspaceOperationKind::SaveFile(_) | WorkspaceOperationKind::CreateFile(_) => {
            OperationPhase::Building
        }
        WorkspaceOperationKind::RunTool(..)
        | WorkspaceOperationKind::BuildDocumentation
        | WorkspaceOperationKind::Clean => OperationPhase::Building,
        WorkspaceOperationKind::PublishDocumentation => OperationPhase::Publishing,
        WorkspaceOperationKind::RunConfiguration { .. } => OperationPhase::Running,
        WorkspaceOperationKind::DeployApplication { .. } => OperationPhase::Staging,
        WorkspaceOperationKind::PackageApplication { .. } => OperationPhase::Building,
        WorkspaceOperationKind::Validate
        | WorkspaceOperationKind::Remember
        | WorkspaceOperationKind::AdoptProject(_)
        | WorkspaceOperationKind::UnregisterProject(_) => OperationPhase::Resolving,
    }
}

fn execute_workspace_operation(
    executable: &Path,
    root: &Path,
    kind: WorkspaceOperationKind,
    fixture_mode: bool,
    context: application::OperationContext,
) -> Result<WorkspaceOperationResult, String> {
    if fixture_mode && !is_isolated_fixture_workspace(root) {
        return Err(
            "Fixture workspace moved outside the temporary directory before the operation started."
                .to_owned(),
        );
    }
    if fixture_mode
        && !matches!(
            &kind,
            WorkspaceOperationKind::ListFiles
                | WorkspaceOperationKind::OpenFile(_)
                | WorkspaceOperationKind::SaveFile(_)
                | WorkspaceOperationKind::CreateFile(_)
        )
    {
        return Err("This operation is unavailable in fixture mode.".to_owned());
    }
    match kind {
        WorkspaceOperationKind::DiscoverSuperworkspaceRepositories => {
            let installation =
                vapor_core::VaporInstallation::discover().map_err(|error| error.to_string())?;
            let ecosystem = vapor_core::discover_ecosystem_repositories(&installation)
                .map_err(|error| error.to_string())?;
            Ok(WorkspaceOperationResult::SourceRepositories(
                ecosystem
                    .repositories
                    .into_iter()
                    .map(|repository| AvailableSuperworkspaceRepositorySnapshot {
                        id: repository.id,
                        name: repository.name,
                        kind: repository.kind,
                        checkout_path: repository.checkout_path,
                        acquired: false,
                    })
                    .collect(),
            ))
        }
        WorkspaceOperationKind::AcquireSuperworkspaceRepositories(repository_names) => {
            let installation =
                vapor_core::VaporInstallation::discover().map_err(|error| error.to_string())?;
            let repository_names = repository_names
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            let report =
                vapor_core::acquire_ecosystem_repositories(&installation, root, &repository_names)
                    .map_err(|error| error.to_string())?;
            Ok(WorkspaceOperationResult::AcquiredSourceRepositories(
                report
                    .repositories
                    .into_iter()
                    .map(|repository| repository.id)
                    .collect(),
            ))
        }
        WorkspaceOperationKind::ListFiles => {
            vapor_client_application::discover_workspace_files(root)
                .map(WorkspaceOperationResult::Listed)
        }
        WorkspaceOperationKind::OpenFile(path) => {
            vapor_client_application::read_workspace_file_with_fingerprint(root, &path).map(
                |(contents, fingerprint)| {
                    WorkspaceOperationResult::Opened(Box::new(WorkspaceFileOpen {
                        path,
                        contents,
                        fingerprint,
                    }))
                },
            )
        }
        WorkspaceOperationKind::SaveFile(save) => {
            vapor_client_application::write_workspace_file_if_fingerprint(
                root,
                &save.path,
                &save.expected_fingerprint,
                &save.contents,
            )
            .map(|fingerprint| WorkspaceOperationResult::Saved {
                path: save.path,
                fingerprint,
            })
        }
        WorkspaceOperationKind::CreateFile(path) => {
            vapor_client_application::create_new_workspace_file(root, &path)
                .map(WorkspaceOperationResult::Created)
        }
        WorkspaceOperationKind::Validate => {
            vapor_client_application::validate_workspace_source(executable, root)
                .map(|_| WorkspaceOperationResult::Completed)
        }
        WorkspaceOperationKind::BuildDocumentation => {
            vapor_client_application::build_documentation_with_context(executable, root, &context)
                .map(WorkspaceOperationResult::CompletedWithSummary)
        }
        WorkspaceOperationKind::PublishDocumentation => {
            vapor_client_application::publish_documentation_with_context(executable, root, &context)
                .map(WorkspaceOperationResult::CompletedWithSummary)
        }
        WorkspaceOperationKind::RunTool(operation, project_selection, target_triples) => {
            context.set_phase(application::OperationPhase::Building);
            if fixture_mode {
                let operation = match operation {
                    WorkspaceToolOperation::Format => vapor_core::DevelopmentOperation::Fmt,
                    WorkspaceToolOperation::Check => vapor_core::DevelopmentOperation::Check,
                    WorkspaceToolOperation::Test => vapor_core::DevelopmentOperation::Test,
                    WorkspaceToolOperation::Build => vapor_core::DevelopmentOperation::Build,
                    WorkspaceToolOperation::BuildRelease | WorkspaceToolOperation::Validate => {
                        return Err("This operation is unavailable in fixture mode.".to_owned());
                    }
                };
                vapor_core::run_workspace_operation_at_with_context(root, operation, &context)
                    .map(|_| WorkspaceOperationResult::Completed)
                    .map_err(|error| error.to_string())
            } else {
                vapor_client_application::run_workspace_tool_with_context(
                    executable,
                    root,
                    operation,
                    &project_selection,
                    &target_triples,
                    &context,
                )
                .map(|_| WorkspaceOperationResult::Completed)
            }
        }
        WorkspaceOperationKind::RunConfiguration { name, profiling } => {
            context.set_phase(application::OperationPhase::Resolving);
            let workspace = vapor_core::VaporWorkspace::discover_from(root)
                .map_err(|error| error.to_string())?;
            vapor_core::run_workspace_configuration_with_context(
                &workspace, &name, profiling, &context,
            )
            .map(|_| WorkspaceOperationResult::Completed)
            .map_err(|error| error.to_string())
        }
        WorkspaceOperationKind::DeployApplication {
            target_triples,
            skip_documentation,
        } => vapor_client_application::deploy_application_with_context(
            executable,
            root,
            &target_triples,
            skip_documentation,
            &context,
        )
        .map(WorkspaceOperationResult::CompletedWithSummary),
        WorkspaceOperationKind::PackageApplication { target_triples } => {
            vapor_client_application::package_application_with_context(
                executable,
                root,
                &target_triples,
                &context,
            )
            .map(WorkspaceOperationResult::CompletedWithSummary)
        }
        WorkspaceOperationKind::Clean => vapor_core::run_workspace_operation_at_with_context(
            root,
            vapor_core::DevelopmentOperation::Clean,
            &context,
        )
        .map(|_| WorkspaceOperationResult::Completed)
        .map_err(|error| error.to_string()),
        WorkspaceOperationKind::Remember => vapor_client_application::remember_workspace(root)
            .map(WorkspaceOperationResult::Remembered),
        WorkspaceOperationKind::AdoptProject(path) => {
            vapor_client_application::adopt_workspace_content_project(executable, root, &path)
                .map(|_| WorkspaceOperationResult::Completed)
        }
        WorkspaceOperationKind::UnregisterProject(id) => {
            vapor_client_application::unregister_workspace_content_project(executable, root, &id)
                .map(|(_, _, identity)| WorkspaceOperationResult::Unregistered(identity))
        }
    }
}

pub(super) fn is_isolated_fixture_workspace(root: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let Ok(temporary_root) = std::env::temp_dir().canonicalize() else {
        return false;
    };
    root.starts_with(temporary_root)
}

fn workspace_result_outcome(
    result: &Result<WorkspaceOperationResult, String>,
) -> Result<String, String> {
    match result {
        Ok(WorkspaceOperationResult::SourceRepositories(repositories)) => {
            Ok(format!("Found {} source repositories.", repositories.len()))
        }
        Ok(WorkspaceOperationResult::AcquiredSourceRepositories(repositories)) => {
            Ok(format!("Added {} source repositories.", repositories.len()))
        }
        Ok(WorkspaceOperationResult::Listed(files)) => {
            Ok(format!("Found {} workspace files.", files.len()))
        }
        Ok(WorkspaceOperationResult::Opened(opened)) => {
            Ok(format!("Opened {}.", opened.path.display()))
        }
        Ok(WorkspaceOperationResult::Saved { path, .. }) => {
            Ok(format!("Saved {}.", path.display()))
        }
        Ok(WorkspaceOperationResult::Created(path)) => Ok(format!("Created {}.", path.display())),
        Ok(WorkspaceOperationResult::Completed) => Ok("Workspace operation completed.".to_owned()),
        Ok(WorkspaceOperationResult::CompletedWithSummary(summary)) => Ok(summary.clone()),
        Ok(WorkspaceOperationResult::Remembered(paths)) => Ok(format!(
            "Registered {} workspace{}.",
            paths.len(),
            if paths.len() == 1 { "" } else { "s" }
        )),
        Ok(WorkspaceOperationResult::Unregistered(identity)) => {
            Ok(format!("Unregistered {}.", identity.id))
        }
        Err(error) => Err(error.clone()),
    }
}

fn is_content_manifest_path(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == "Vapor.toml" || name.to_string_lossy().ends_with(".vapor.toml"))
}

#[cfg(test)]
mod tests {
    use super::validate_clean_confirmation;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn clean_confirmation_must_name_the_open_workspace() {
        let root = std::env::temp_dir().join(format!(
            "vapor-clean-confirmation-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        let other = root.with_extension("other");
        std::fs::create_dir_all(&root).expect("create active workspace");
        std::fs::create_dir_all(&other).expect("create another workspace");
        let root = std::fs::canonicalize(root).expect("canonicalize active workspace");
        let other = std::fs::canonicalize(other).expect("canonicalize other workspace");

        assert!(validate_clean_confirmation(&root, &root.display().to_string()).is_ok());
        let error = validate_clean_confirmation(&root, &other.display().to_string())
            .expect_err("a confirmation for another workspace must fail");
        assert!(error.message.contains("does not match"));

        std::fs::remove_dir_all(&root).expect("remove active workspace");
        std::fs::remove_dir_all(&other).expect("remove other workspace");
    }
}
