//! Shared-service orchestration for publishing workflows.

use super::*;

impl ClientSession {
    pub(super) fn start_root_publish_operation(
        &mut self,
        action: PublishingAction,
    ) -> Result<(), ServiceError> {
        if self.bootstrap.fixture_mode {
            return Err(invalid_client_action_request(
                "Steam root publishing is unavailable in Vapor Client fixture mode.",
            ));
        }
        self.reject_if_operation_in_progress()?;
        let (mut request, review_token, staged) = match action {
            PublishingAction::StageRoot {
                account,
                description,
                set_live,
            } => {
                let account = account.trim();
                let description = description.trim();
                if account.is_empty() || account.len() > 128 {
                    return Err(invalid_client_action_request(
                        "Enter a Steamworks account name of 1 to 128 bytes.",
                    ));
                }
                if description.is_empty() || description.len() > 256 {
                    return Err(invalid_client_action_request(
                        "Enter a release description of 1 to 256 bytes.",
                    ));
                }
                let set_live = set_live
                    .map(|branch| branch.trim().to_owned())
                    .filter(|branch| !branch.is_empty());
                if set_live.as_deref().is_some_and(|branch| {
                    branch.len() > 64
                        || !branch
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
                }) {
                    return Err(invalid_client_action_request(
                        "Beta branch names may contain only letters, numbers, hyphens, underscores, and periods.",
                    ));
                }
                self.root_publish_review = None;
                (
                    vapor_sdk_core::root::types::RootPublishRequest {
                        plan: true,
                        app_id: vapor_sdk_core::root::types::ROOT_STEAM_APP_ID,
                        depot_id: vapor_sdk_core::root::types::ROOT_STEAM_DEPOT_ID,
                        description: description.to_owned(),
                        account: account.to_owned(),
                        steamcmd: PathBuf::from("steamcmd"),
                        set_live,
                        expected_fingerprint: None,
                    },
                    String::new(),
                    true,
                )
            }
            PublishingAction::PublishRoot {
                review_id,
                confirmation,
            } => {
                let review = self
                    .root_publish_review
                    .as_ref()
                    .filter(|review| review.snapshot.review_id == review_id && review.snapshot.planned)
                    .ok_or_else(|| invalid_client_action_request(
                        "This Steam root review is no longer current. Stage a fresh release plan.",
                    ))?;
                let phrase = match review.snapshot.set_live.as_deref() {
                    Some(branch) => format!(
                        "UPLOAD ROOT {} AND SET LIVE {branch}",
                        vapor_sdk_core::root::types::ROOT_STEAM_APP_ID
                    ),
                    None => format!(
                        "UPLOAD ROOT {}",
                        vapor_sdk_core::root::types::ROOT_STEAM_APP_ID
                    ),
                };
                if confirmation.trim() != phrase {
                    return Err(invalid_client_action_request(&format!(
                        "Type {phrase} exactly to authorize this SteamPipe upload."
                    )));
                }
                let mut request = review.request.clone();
                request.plan = false;
                request.expected_fingerprint = Some(review.snapshot.fingerprint.clone());
                (request, review_id, false)
            }
            PublishingAction::StageApplication { .. }
            | PublishingAction::PublishApplication { .. }
            | PublishingAction::DiscardRootReview => unreachable!("handled by dispatch"),
        };
        request.plan = staged;
        let action_id = if staged {
            "publishing.stage-root"
        } else {
            "publishing.publish-root"
        };
        self.spawn_background_operation(
            BackgroundOperationSpec {
                domain: "studio-root-publishing",
                action: action_id,
                thread_prefix: "studio-root-publish",
                failure_label: "Steam root operation",
                composition_id: None,
                phase: OperationPhase::Publishing,
            },
            move |context| {
                context.set_phase(application::OperationPhase::Publishing);
                let result = vapor_sdk_core::root::publish::root_publish(&request)
                    .map_err(|error| error.to_string());
                (request, result)
            },
            |(_, result)| match result {
                Ok(report)
                    if report
                        .status
                        .as_ref()
                        .is_some_and(|status| status.success()) =>
                {
                    Ok("Steam root operation completed".to_owned())
                }
                Ok(report) => Err(report
                    .status
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "Steam root operation failed".to_owned())),
                Err(error) => Err(error.clone()),
            },
            move |id, (request, result)| {
                BackgroundOperationMessage::CompleteRootPublish(Box::new(
                    RootPublishOperationCompletion {
                        id,
                        review_id: review_token,
                        staged,
                        request,
                        result,
                    },
                ))
            },
        )
    }

    pub(super) fn start_application_release_operation(
        &mut self,
        action: PublishingAction,
    ) -> Result<(), ServiceError> {
        if self.bootstrap.fixture_mode {
            return Err(invalid_client_action_request(
                "Steam application releases are unavailable in fixture mode.",
            ));
        }
        self.reject_if_operation_in_progress()?;
        let workspace_root = self.bootstrap.workspace_root.clone().ok_or_else(|| {
            invalid_client_action_request(
                "Open the Vapor application source workspace before staging a release.",
            )
        })?;
        let executable = self.bootstrap.application_executable.clone();
        let (task, staged, review_id) = match action {
            PublishingAction::StageApplication {
                account,
                branch,
                description,
                runtime_targets,
                build_release,
            } => {
                self.application_release_review = None;
                self.application_release_snapshot = None;
                (
                    ApplicationReleaseTask::Stage {
                        account,
                        branch,
                        description,
                        runtime_targets,
                        build_release,
                    },
                    true,
                    String::new(),
                )
            }
            PublishingAction::PublishApplication {
                review_id,
                confirmation,
            } => {
                let review = self
                    .application_release_review
                    .as_ref()
                    .filter(|_| {
                        self.application_release_snapshot
                            .as_ref()
                            .is_some_and(|snapshot| {
                                snapshot.review_id == review_id && snapshot.planned
                            })
                    })
                    .cloned()
                    .ok_or_else(|| invalid_client_action_request(
                        "This Steam app release review is no longer current. Stage a fresh review.",
                    ))?;
                let phrase = format!("PUBLISH APPLICATION {} TO {}", review.app_id, review.branch);
                if !review.release_build_validated {
                    return Err(invalid_client_action_request(
                        "A real Steam upload requires a service-validated release build. Stage with managed build enabled.",
                    ));
                }
                if confirmation.trim() != phrase {
                    return Err(invalid_client_action_request(&format!(
                        "Type {phrase} exactly to authorize this SteamPipe upload."
                    )));
                }
                (ApplicationReleaseTask::Publish(review), false, review_id)
            }
            PublishingAction::StageRoot { .. }
            | PublishingAction::PublishRoot { .. }
            | PublishingAction::DiscardRootReview => unreachable!("handled by dispatch"),
        };
        let action_id = if staged {
            "publishing.stage-application"
        } else {
            "publishing.publish-application"
        };
        let phase = match &task {
            ApplicationReleaseTask::Stage {
                build_release: true,
                ..
            } => OperationPhase::Building,
            _ => OperationPhase::Publishing,
        };
        self.spawn_background_operation(
            BackgroundOperationSpec {
                domain: "application-release",
                action: action_id,
                thread_prefix: "application-release",
                failure_label: "Steam application release",
                composition_id: None,
                phase,
            },
            move |context| match task {
                ApplicationReleaseTask::Stage {
                    account,
                    branch,
                    description,
                    runtime_targets,
                    build_release,
                } => {
                    application::validate_application_release_request(
                        &executable,
                        &workspace_root,
                        &runtime_targets,
                        branch.as_deref(),
                        &description,
                        &account,
                    )?;
                    if build_release {
                        context.set_phase(application::OperationPhase::Building);
                        let progress_context = context.clone();
                        let output_context = context.clone();
                        application::build_application_release_with_events(
                            &executable,
                            &workspace_root,
                            &runtime_targets,
                            move |completed, total, message| {
                                progress_context.report_progress(application::Progress {
                                    completed: completed as u64,
                                    total: Some(total as u64),
                                    unit: "steps".to_owned(),
                                    message: Some(message.to_owned()),
                                });
                            },
                            move |stream, text| {
                                output_context.report_output(application::OperationOutput {
                                    stream,
                                    text: text.to_owned(),
                                });
                            },
                        )?;
                    }
                    context.set_phase(application::OperationPhase::Staging);
                    application::stage_application_release(
                        &executable,
                        &workspace_root,
                        &runtime_targets,
                        branch.as_deref(),
                        &description,
                        &account,
                        build_release,
                    )
                }
                ApplicationReleaseTask::Publish(review) => {
                    context.set_phase(application::OperationPhase::Publishing);
                    application::publish_reviewed_application_release(
                        &executable,
                        &workspace_root,
                        &review,
                    )
                    .map(|result| result.review)
                }
            },
            |result| match result {
                Ok(_) => Ok("Steam application release operation completed".to_owned()),
                Err(error) => Err(error.clone()),
            },
            move |id, result| {
                BackgroundOperationMessage::CompleteApplicationRelease(Box::new(
                    ApplicationReleaseOperationCompletion {
                        id,
                        review_id,
                        staged,
                        result,
                    },
                ))
            },
        )
    }

    pub(super) fn apply_application_release_completion(
        &mut self,
        completion: ApplicationReleaseOperationCompletion,
    ) {
        let mut succeeded = false;
        let (message, snapshot) = match completion.result {
            Ok(review) => {
                succeeded = true;
                let review_id = if completion.staged {
                    completion.id.clone()
                } else {
                    completion.review_id.clone()
                };
                let snapshot = application_release_snapshot(
                    &review_id,
                    &review,
                    completion.staged,
                    !completion.staged,
                );
                if completion.staged {
                    self.application_release_review = Some(review);
                } else {
                    self.application_release_review = None;
                }
                let message = if completion.staged {
                    format!(
                        "Steam app release review ready · {} files across {} depots.",
                        snapshot.files,
                        snapshot.depot_ids.len()
                    )
                } else {
                    "Steam app release upload completed.".to_owned()
                };
                (message, Some(snapshot))
            }
            Err(error) => (error, None),
        };
        if let Some(snapshot) = snapshot {
            self.application_release_snapshot = Some(snapshot);
        }
        let Some(active) = self.active_operation.take() else {
            return;
        };
        self.revision.0 = self.revision.0.saturating_add(1);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: active.id,
                composition_id: None,
                action: active.action,
                phase: OperationPhase::Complete,
                status: if succeeded {
                    OperationStatus::Succeeded
                } else {
                    OperationStatus::Failed {
                        message: message.clone(),
                    }
                },
                progress: None,
                message: Some(message),
            }));
    }

    pub(super) fn apply_root_publish_completion(
        &mut self,
        completion: RootPublishOperationCompletion,
    ) {
        let mut success = true;
        let message = match completion.result {
            Ok(report) => {
                let upload_succeeded = report
                    .status
                    .as_ref()
                    .is_some_and(|status| status.success());
                let status = report.status.map(|status| status.to_string());
                let snapshot = RootPublishingSnapshot {
                    review_id: if completion.staged {
                        completion.id.clone()
                    } else {
                        completion.review_id.clone()
                    },
                    planned: report.planned,
                    app_id: report.package.app_id,
                    depot_id: report.package.depot_id,
                    account: report.account,
                    description: report.package.description,
                    set_live: report.set_live,
                    steamcmd: report.steamcmd.display().to_string(),
                    package_root: report.package.package_root.display().to_string(),
                    copied_files: report.package.copied_files,
                    fingerprint: report.package.fingerprint.clone(),
                    included_roots: report
                        .package
                        .included_roots
                        .into_iter()
                        .map(|path| path.display().to_string())
                        .collect(),
                    status: status.clone(),
                    succeeded: upload_succeeded,
                };
                let summary = if completion.staged {
                    format!(
                        "Steam root upload plan ready · {} files.",
                        snapshot.copied_files
                    )
                } else if upload_succeeded {
                    "SteamCMD completed the root upload.".to_owned()
                } else {
                    success = false;
                    format!(
                        "SteamCMD root upload finished with status {}.",
                        status.as_deref().unwrap_or("unknown")
                    )
                };
                self.root_publish_review = Some(RootPublishReviewState {
                    request: completion.request,
                    snapshot,
                });
                summary
            }
            Err(error) => {
                success = false;
                error
            }
        };
        let Some(active) = self.active_operation.take() else {
            return;
        };
        self.revision.0 = self.revision.0.saturating_add(1);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: active.id,
                composition_id: None,
                action: active.action,
                phase: OperationPhase::Complete,
                status: if success {
                    OperationStatus::Succeeded
                } else {
                    OperationStatus::Failed {
                        message: message.clone(),
                    }
                },
                progress: None,
                message: Some(message),
            }));
    }
}

fn application_release_snapshot(
    review_id: &str,
    review: &application::ApplicationReleaseReview,
    planned: bool,
    succeeded: bool,
) -> ApplicationReleaseSnapshot {
    ApplicationReleaseSnapshot {
        review_id: review_id.to_owned(),
        planned,
        app_id: review.app_id,
        depot_ids: review.depot_ids.clone(),
        runtime_targets: review.runtime_targets.clone(),
        branch: review.branch.clone(),
        account: review.account.clone(),
        description: review.description.clone(),
        release_build_validated: review.release_build_validated,
        package_root: review.package_root.display().to_string(),
        build_script: review.build_script.display().to_string(),
        files: review.files,
        fingerprint: review.fingerprint.clone(),
        succeeded,
        status: succeeded.then(|| "SteamCMD upload completed".to_owned()),
    }
}
