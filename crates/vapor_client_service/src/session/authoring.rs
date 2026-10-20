//! Shared-service orchestration for authoring workflows.

use super::*;

impl ClientSession {
    pub(super) fn start_authoring_operation(
        &mut self,
        mut action: AuthoringAction,
    ) -> Result<(), ServiceError> {
        if self.bootstrap.fixture_mode {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "Project authoring is unavailable in fixture mode; no real workspace, toolchain, or source registry will be accessed.".to_owned(),
                retryable: false,
            });
        }
        self.reject_if_operation_in_progress()?;
        let workspace = self.bootstrap.workspace_root.clone();
        let executable = self.bootstrap.application_executable.clone();
        if let AuthoringAction::PackageContent {
            selector,
            target_triples,
            ..
        } = &mut action
        {
            if selector.trim().is_empty() || selector.len() > 512 {
                return Err(invalid_client_action_request(
                    "Choose a content identity of 1 to 512 bytes before packaging.",
                ));
            }
            let workspace_root = workspace.as_deref().ok_or_else(|| {
                invalid_client_action_request("Open a workspace before packaging content.")
            })?;
            *target_triples = vapor_client_application::resolve_workspace_runtime_targets(
                &executable,
                workspace_root,
                target_triples,
            )
            .map_err(|message| ServiceError {
                code: ServiceErrorCode::ProviderFailure,
                message,
                retryable: false,
            })?;
            super::validate_runtime_target_matrix(target_triples)?;
        }
        let workspace_bound = !matches!(
            &action,
            AuthoringAction::CreateWorkspace { .. }
                | AuthoringAction::ListRegisteredSources
                | AuthoringAction::RememberSource { .. }
                | AuthoringAction::OpenSource { .. }
                | AuthoringAction::ForgetSource { .. }
                | AuthoringAction::CloseSource
                | AuthoringAction::InitializeSourceWorkspace { .. }
                | AuthoringAction::RepairSourceMetadata { .. }
        );
        let action_id = authoring_action_id(&action);
        let phase = authoring_action_phase(&action);
        let completion_workspace = workspace.clone();
        self.spawn_background_operation(
            BackgroundOperationSpec {
                domain: "studio-authoring",
                action: action_id,
                thread_prefix: "studio-authoring",
                failure_label: "authoring operation",
                composition_id: None,
                phase,
            },
            move |context| {
                execute_authoring_operation(&executable, workspace.as_deref(), action, &context)
            },
            authoring_result_outcome,
            move |id, result| {
                BackgroundOperationMessage::CompleteAuthoring(Box::new(
                    AuthoringOperationCompletion {
                        id,
                        action: action_id.to_owned(),
                        workspace: completion_workspace,
                        workspace_bound,
                        result,
                    },
                ))
            },
        )
    }

    pub(super) fn apply_authoring_completion(&mut self, completion: AuthoringOperationCompletion) {
        let mut result = completion.result;
        if completion.workspace_bound
            && self.bootstrap.workspace_root.as_deref() != completion.workspace.as_deref()
        {
            result = Err("The active workspace changed while authoring was in progress.".into());
        }
        let message = result
            .as_ref()
            .map(authoring_result_message)
            .unwrap_or_else(|error| error.clone());
        let succeeded = result.is_ok();
        self.clear_active_operation();
        self.revision.0 = self.revision.0.saturating_add(1);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: completion.id.clone(),
                composition_id: None,
                action: completion.action,
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
        if let Ok(result) = result {
            match &result {
                AuthoringResultKind::RegisteredSources { .. }
                | AuthoringResultKind::ContentValidated { .. }
                | AuthoringResultKind::ContentPackaged { .. } => {}
                AuthoringResultKind::Composition { .. }
                | AuthoringResultKind::WorkshopMetadata { .. }
                | AuthoringResultKind::RuntimeOutputs { .. } => self.refresh_catalog(),
                AuthoringResultKind::ContentCreated { .. } => self.refresh_catalog(),
                AuthoringResultKind::SourceWorkspaceCreated { .. } => self.refresh_catalog(),
                AuthoringResultKind::SourceMetadataRepaired { .. } => self.refresh_catalog(),
                AuthoringResultKind::WorkspaceCreated { .. } => {}
            }
            self.operation_events
                .push_back(ClientEvent::AuthoringResult(Box::new(AuthoringResult {
                    operation_id: completion.id,
                    result,
                })));
        }
    }
}

fn authoring_action_id(action: &AuthoringAction) -> &'static str {
    match action {
        AuthoringAction::LoadComposition { .. } => "authoring.load-composition",
        AuthoringAction::ListRegisteredSources => "authoring.list-registered-sources",
        AuthoringAction::RememberSource { .. } => "authoring.remember-source",
        AuthoringAction::OpenSource { .. } => "authoring.open-source",
        AuthoringAction::ForgetSource { .. } => "authoring.forget-source",
        AuthoringAction::CloseSource => "authoring.close-source",
        AuthoringAction::SaveComposition { .. } => "authoring.save-composition",
        AuthoringAction::LoadWorkshopMetadata { .. } => "authoring.load-workshop-metadata",
        AuthoringAction::SaveWorkshopMetadata { .. } => "authoring.save-workshop-metadata",
        AuthoringAction::LoadRuntimeOutputs { .. } => "authoring.load-runtime-outputs",
        AuthoringAction::ValidateContent { .. } => "authoring.validate-content",
        AuthoringAction::PackageContent { .. } => "authoring.package-content",
        AuthoringAction::SaveRuntimeOutputs { .. } => "authoring.save-runtime-outputs",
        AuthoringAction::CreateContent { .. } => "authoring.create-content",
        AuthoringAction::ForkContent { .. } => "authoring.fork-content",
        AuthoringAction::CreateWorkspace { .. } => "authoring.create-workspace",
        AuthoringAction::InitializeSourceWorkspace { .. } => {
            "authoring.initialize-source-workspace"
        }
        AuthoringAction::RepairSourceMetadata { .. } => "authoring.repair-source-metadata",
    }
}

fn authoring_action_phase(action: &AuthoringAction) -> OperationPhase {
    match action {
        AuthoringAction::PackageContent { .. } => OperationPhase::Building,
        _ => OperationPhase::Resolving,
    }
}

fn execute_authoring_operation(
    executable: &Path,
    active_workspace: Option<&Path>,
    action: AuthoringAction,
    context: &vapor_core::OperationContext,
) -> Result<AuthoringResultKind, String> {
    use AuthoringAction as Action;
    match action {
        Action::ListRegisteredSources => {
            let installation =
                vapor_core::VaporInstallation::discover().map_err(|error| error.to_string())?;
            let state =
                vapor_core::source_state(&installation).map_err(|error| error.to_string())?;
            Ok(registered_sources_result(state, active_workspace))
        }
        Action::RememberSource { root } => {
            let installation =
                vapor_core::VaporInstallation::discover().map_err(|error| error.to_string())?;
            let state = vapor_core::remember_source(&installation, Path::new(&root))
                .map_err(|error| error.to_string())?;
            Ok(registered_sources_result(state, active_workspace))
        }
        Action::OpenSource { root } => {
            let installation =
                vapor_core::VaporInstallation::discover().map_err(|error| error.to_string())?;
            let state = vapor_core::open_source(&installation, Path::new(&root))
                .map_err(|error| error.to_string())?;
            Ok(registered_sources_result(state, active_workspace))
        }
        Action::ForgetSource { root } => {
            let installation =
                vapor_core::VaporInstallation::discover().map_err(|error| error.to_string())?;
            let state = vapor_core::forget_source(&installation, Path::new(&root))
                .map_err(|error| error.to_string())?;
            Ok(registered_sources_result(state, active_workspace))
        }
        Action::CloseSource => {
            let installation =
                vapor_core::VaporInstallation::discover().map_err(|error| error.to_string())?;
            let state =
                vapor_core::close_source(&installation).map_err(|error| error.to_string())?;
            Ok(registered_sources_result(state, active_workspace))
        }
        Action::LoadComposition { project_id } => {
            let workspace = active_workspace.ok_or("Open a workspace before editing content.")?;
            vapor_client_application::load_source_composition(executable, workspace, &project_id)
                .map(|value| AuthoringResultKind::Composition {
                    id: value.id,
                    version: value.version,
                    kind: value.kind,
                    manifest: value.manifest.to_string_lossy().into_owned(),
                    source: value.source,
                    links: value
                        .links
                        .into_iter()
                        .map(|link| AuthoringCompositionLink {
                            id: link.id,
                            relationship: link.relationship,
                            optional: link.optional,
                            version: link.version,
                            workshop_id: link.workshop_id,
                            reason: link.reason,
                        })
                        .collect(),
                })
        }
        Action::SaveComposition {
            project_id,
            expected_fingerprint,
            dependencies,
        } => {
            let workspace = active_workspace.ok_or("Open a workspace before editing content.")?;
            let links = dependencies
                .into_iter()
                .map(|link| vapor_client_application::SourceCompositionLink {
                    id: link.project_id,
                    relationship: link.relationship,
                    optional: link.optional,
                    version: link.version_requirement,
                    workshop_id: link.workshop_id,
                    reason: link.reason,
                })
                .collect::<Vec<_>>();
            vapor_client_application::save_source_composition(
                executable,
                workspace,
                &project_id,
                &expected_fingerprint,
                &links,
            )
            .map(|value| AuthoringResultKind::Composition {
                id: value.id,
                version: value.version,
                kind: value.kind,
                manifest: value.manifest.to_string_lossy().into_owned(),
                source: value.source,
                links: value
                    .links
                    .into_iter()
                    .map(|link| AuthoringCompositionLink {
                        id: link.id,
                        relationship: link.relationship,
                        optional: link.optional,
                        version: link.version,
                        workshop_id: link.workshop_id,
                        reason: link.reason,
                    })
                    .collect(),
            })
        }
        Action::LoadWorkshopMetadata { project_id } => {
            let workspace = active_workspace.ok_or("Open a workspace before editing content.")?;
            vapor_client_application::load_source_workshop_metadata(
                executable,
                workspace,
                &project_id,
            )
            .map(|value| AuthoringResultKind::WorkshopMetadata {
                id: value.id,
                version: value.version,
                kind: value.kind,
                manifest: value.manifest.to_string_lossy().into_owned(),
                source: value.source,
                policy: workshop_policy_to_protocol(value.policy),
            })
        }
        Action::SaveWorkshopMetadata {
            project_id,
            expected_fingerprint,
            app_id,
            published_file_id,
            visibility,
            title,
            description,
            tags,
            change_note,
        } => {
            let workspace = active_workspace.ok_or("Open a workspace before editing content.")?;
            vapor_client_application::save_source_workshop_metadata(
                executable,
                workspace,
                &project_id,
                &expected_fingerprint,
                &vapor_client_application::WorkshopPolicyDraft {
                    app_id,
                    published_file_id,
                    visibility,
                    title,
                    description,
                    tags,
                    change_note,
                },
            )
            .map(|value| AuthoringResultKind::WorkshopMetadata {
                id: value.id,
                version: value.version,
                kind: value.kind,
                manifest: value.manifest.to_string_lossy().into_owned(),
                source: value.source,
                policy: workshop_policy_to_protocol(value.policy),
            })
        }
        Action::LoadRuntimeOutputs { project_id } => {
            let workspace = active_workspace.ok_or("Open a workspace before editing content.")?;
            vapor_client_application::load_source_runtime_outputs(
                executable,
                workspace,
                &project_id,
            )
            .map(|value| AuthoringResultKind::RuntimeOutputs {
                id: value.id,
                version: value.version,
                kind: value.kind,
                manifest: value.manifest.to_string_lossy().into_owned(),
                source: value.source,
                binaries: value.binaries,
                libraries: value.libraries,
            })
        }
        Action::ValidateContent { project_id } => {
            let workspace =
                active_workspace.ok_or("Open a workspace before validating content.")?;
            vapor_client_application::validate_composition_source(
                executable,
                workspace,
                &project_id,
            )
            .map(|summary| AuthoringResultKind::ContentValidated {
                id: project_id,
                summary,
            })
        }
        Action::PackageContent {
            selector,
            target_triples,
            dry_run,
        } => {
            let workspace = active_workspace.ok_or("Open a workspace before packaging content.")?;
            vapor_client_application::build_and_package_authored_content_with_context(
                executable,
                workspace,
                &selector,
                &target_triples,
                dry_run,
                context,
            )
            .map(|report| AuthoringResultKind::ContentPackaged {
                id: report.artifact_id().to_owned(),
                root: report.root().to_string_lossy().into_owned(),
                target: report.runtime_target().to_owned(),
                fingerprint: format!(
                    "{}:{}",
                    report.fingerprint().algorithm(),
                    report.fingerprint().hash()
                ),
                files: report.fingerprint().files(),
                bytes: report.fingerprint().bytes(),
                dry_run: report.dry_run(),
                receipt: report
                    .receipt()
                    .map(|path| path.to_string_lossy().into_owned()),
            })
        }
        Action::SaveRuntimeOutputs {
            project_id,
            expected_fingerprint,
            binaries,
            libraries,
        } => {
            let workspace = active_workspace.ok_or("Open a workspace before editing content.")?;
            vapor_client_application::save_source_runtime_outputs(
                executable,
                workspace,
                &project_id,
                &expected_fingerprint,
                &binaries,
                &libraries,
            )
            .map(|value| AuthoringResultKind::RuntimeOutputs {
                id: value.id,
                version: value.version,
                kind: value.kind,
                manifest: value.manifest.to_string_lossy().into_owned(),
                source: value.source,
                binaries: value.binaries,
                libraries: value.libraries,
            })
        }
        Action::CreateContent {
            workspace_root,
            kind,
            id,
            version,
            dependencies,
            optional_dependencies,
            workshop,
        } => {
            let active = active_workspace.ok_or("Open a workspace before creating content.")?;
            let requested = PathBuf::from(workspace_root)
                .canonicalize()
                .map_err(|error| error.to_string())?;
            let active = active.canonicalize().map_err(|error| error.to_string())?;
            if requested != active {
                return Err("Content creation must target the currently open workspace.".to_owned());
            }
            let kind = parse_authoring_content_kind(&kind)?;
            let policy = vapor_client_application::WorkshopPolicyDraft {
                app_id: workshop.app_id,
                published_file_id: workshop.published_file_id,
                visibility: workshop.visibility,
                title: workshop.title,
                description: workshop.description,
                tags: workshop.tags,
                change_note: workshop.change_note,
            };
            let manifest =
                vapor_client_application::create_content_workspace_with_dependency_policies(
                    executable,
                    &active,
                    &id,
                    &version,
                    kind,
                    vapor_client_application::ContentDependencyDraft {
                        required: dependencies,
                        optional: optional_dependencies,
                    },
                    &policy,
                )?;
            let contents = std::fs::read_to_string(&manifest)
                .map_err(|error| format!("{}: {error}", manifest.display()))?;
            let files = vapor_client_application::discover_workspace_files(&active)?
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            Ok(AuthoringResultKind::ContentCreated {
                workspace_root: active.to_string_lossy().into_owned(),
                manifest: manifest
                    .strip_prefix(&active)
                    .unwrap_or(&manifest)
                    .to_string_lossy()
                    .into_owned(),
                manifest_contents: contents,
                files,
            })
        }
        Action::ForkContent {
            workspace_root,
            content_id,
            name,
        } => {
            let active = active_workspace.ok_or("Open a workspace before forking content.")?;
            let requested = PathBuf::from(workspace_root)
                .canonicalize()
                .map_err(|error| error.to_string())?;
            let active = active.canonicalize().map_err(|error| error.to_string())?;
            if requested != active {
                return Err("Content forks must target the currently open workspace.".to_owned());
            }
            if !valid_workspace_slug(&name) {
                return Err(
                    "Fork name must use lowercase letters, digits, and internal hyphens."
                        .to_owned(),
                );
            }
            let manifest = vapor_client_application::fork_acquired_content(
                executable,
                &active,
                &content_id,
                &name,
            )?;
            let contents = std::fs::read_to_string(&manifest)
                .map_err(|error| format!("{}: {error}", manifest.display()))?;
            let files = vapor_client_application::discover_workspace_files(&active)?
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            Ok(AuthoringResultKind::ContentCreated {
                workspace_root: active.to_string_lossy().into_owned(),
                manifest: manifest
                    .strip_prefix(&active)
                    .unwrap_or(&manifest)
                    .to_string_lossy()
                    .into_owned(),
                manifest_contents: contents,
                files,
            })
        }
        Action::CreateWorkspace {
            parent,
            organization,
            name,
        } => {
            if !valid_workspace_slug(&organization) || !valid_workspace_slug(&name) {
                return Err(
                    "Workspace organization and name must use lowercase letters, digits, and internal hyphens."
                        .to_owned(),
                );
            }
            let parent = PathBuf::from(parent);
            let parent = parent
                .canonicalize()
                .map_err(|error| format!("{}: {error}", parent.display()))?;
            if !parent.is_dir() {
                return Err(format!("{} is not a directory", parent.display()));
            }
            let destination = parent.join(&name);
            let root = vapor_client_application::create_basic_content_workspace(
                executable,
                &destination,
                &organization,
                &name,
            )?;
            let files = vapor_client_application::discover_workspace_files(&root)?
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            Ok(AuthoringResultKind::WorkspaceCreated {
                root: root.to_string_lossy().into_owned(),
                files,
            })
        }
        Action::InitializeSourceWorkspace {
            path,
            organization,
            name,
            app_id,
        } => {
            if !valid_workspace_slug(&organization) || !valid_workspace_slug(&name) {
                return Err(
                    "Workspace organization and name must use lowercase letters, digits, and internal hyphens."
                        .to_owned(),
                );
            }
            let destination = PathBuf::from(&path);
            let destination = if destination.is_absolute() {
                destination
            } else {
                std::env::current_dir()
                    .map_err(|error| format!("failed to read invocation directory: {error}"))?
                    .join(destination)
            };
            let report = vapor_client_application::initialize_source_workspace(
                executable,
                &destination,
                &organization,
                &name,
                app_id,
            )?;
            let installation =
                vapor_core::VaporInstallation::discover().map_err(|error| error.to_string())?;
            vapor_core::open_source(&installation, &report.root)
                .map_err(|error| error.to_string())?;
            let files = vapor_client_application::discover_workspace_files(&report.root)?
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            Ok(AuthoringResultKind::SourceWorkspaceCreated {
                root: report.root.to_string_lossy().into_owned(),
                workspace_id: report.workspace_id,
                packagepack_id: report.packagepack_id,
                files,
            })
        }
        Action::RepairSourceMetadata {
            workspace_root,
            write_changes,
        } => {
            let report = vapor_client_application::repair_source_metadata(
                executable,
                Path::new(&workspace_root),
                write_changes,
            )?;
            Ok(AuthoringResultKind::SourceMetadataRepaired {
                write_changes,
                actions: report
                    .actions
                    .into_iter()
                    .map(|action| SourceMetadataRepairAction {
                        manifest: action.manifest.to_string_lossy().into_owned(),
                        section: action.section,
                        reference_id: action.reference_id,
                        workshop_id: action.workshop_id,
                    })
                    .collect(),
            })
        }
    }
}

fn valid_workspace_slug(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value
            .bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn workshop_policy_to_protocol(
    value: vapor_client_application::WorkshopPolicyDraft,
) -> WorkshopMetadataDraft {
    WorkshopMetadataDraft {
        app_id: value.app_id,
        published_file_id: value.published_file_id,
        visibility: value.visibility,
        title: value.title,
        description: value.description,
        tags: value.tags,
        change_note: value.change_note,
    }
}

fn parse_authoring_content_kind(kind: &str) -> Result<vapor_core::ContentKind, String> {
    Ok(match kind {
        "engine" => vapor_core::ContentKind::Engine,
        "game" => vapor_core::ContentKind::Game,
        "packagepack" => vapor_core::ContentKind::Packagepack,
        "enginepack" => vapor_core::ContentKind::Enginepack,
        "gamepack" => vapor_core::ContentKind::Gamepack,
        "modpack" => vapor_core::ContentKind::Modpack,
        "engine-mod" => vapor_core::ContentKind::EngineMod,
        "game-mod" => vapor_core::ContentKind::GameMod,
        "extension-mod" => vapor_core::ContentKind::ExtensionMod,
        "crate" => vapor_core::ContentKind::Crate,
        _ => return Err(format!("Unsupported source content role: {kind}")),
    })
}

fn authoring_result_outcome(
    result: &Result<AuthoringResultKind, String>,
) -> Result<String, String> {
    result
        .as_ref()
        .map(authoring_result_message)
        .map_err(Clone::clone)
}

fn authoring_result_message(result: &AuthoringResultKind) -> String {
    match result {
        AuthoringResultKind::Composition { id, .. } => format!("Loaded composition for {id}."),
        AuthoringResultKind::RegisteredSources { roots, .. } => {
            format!(
                "Loaded {} registered workspace{}.",
                roots.len(),
                if roots.len() == 1 { "" } else { "s" }
            )
        }
        AuthoringResultKind::WorkshopMetadata { id, .. } => {
            format!("Loaded Workshop metadata for {id}.")
        }
        AuthoringResultKind::RuntimeOutputs { id, .. } => {
            format!("Loaded runtime outputs for {id}.")
        }
        AuthoringResultKind::ContentValidated { id, summary } => {
            format!("Validated {id}: {summary}")
        }
        AuthoringResultKind::ContentPackaged {
            id,
            files,
            bytes,
            dry_run,
            ..
        } => format!(
            "{} {id} ({files} files, {bytes} bytes).",
            if *dry_run {
                "Package preview ready for"
            } else {
                "Package staged for"
            }
        ),
        AuthoringResultKind::ContentCreated { manifest, .. } => {
            format!("Created content at {manifest}.")
        }
        AuthoringResultKind::WorkspaceCreated { root, .. } => {
            format!("Created workspace at {root}.")
        }
        AuthoringResultKind::SourceWorkspaceCreated { root, .. } => {
            format!("Created source workspace at {root}.")
        }
        AuthoringResultKind::SourceMetadataRepaired {
            write_changes,
            actions,
        } => format!(
            "{} {} source metadata repair{}.",
            if *write_changes { "Applied" } else { "Found" },
            actions.len(),
            if actions.len() == 1 { "" } else { "s" }
        ),
    }
}

fn registered_sources_result(
    state: vapor_core::SourceState,
    workspace: Option<&Path>,
) -> AuthoringResultKind {
    let mut roots = state.known.into_iter().collect::<BTreeSet<_>>();
    let discovery_start = state
        .superworkspace
        .as_deref()
        .or(workspace)
        .or(state.active.as_deref());
    if let Some(start) = discovery_start
        && let Ok(superworkspace) = vapor_core::VaporSuperworkspace::discover_from(start)
    {
        roots.extend(
            superworkspace
                .repositories
                .into_iter()
                .map(|repository| repository.root),
        );
    }
    AuthoringResultKind::RegisteredSources {
        roots: roots
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        active: state.active.map(|path| path.to_string_lossy().into_owned()),
    }
}
