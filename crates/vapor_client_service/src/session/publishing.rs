//! Shared-service orchestration for publishing workflows.

use super::*;

impl ClientSession {
    pub(super) fn start_application_release_operation(
        &mut self,
        action: PublishingAction,
    ) -> Result<(), ServiceError> {
        if self.bootstrap.fixture_mode {
            return Err(invalid_client_action_request(
                "Steam application releases are unavailable in fixture mode.",
            ));
        }
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
            } => {
                self.application_release_review = None;
                self.application_release_snapshot = None;
                (
                    ApplicationReleaseTask::Stage {
                        account,
                        branch,
                        description,
                        runtime_targets,
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
                if !review.payload_reviewed {
                    return Err(invalid_client_action_request(
                        "A real Steam upload requires a service-reviewed package payload. Stage a fresh review.",
                    ));
                }
                if confirmation.trim() != phrase {
                    return Err(invalid_client_action_request(&format!(
                        "Type {phrase} exactly to authorize this SteamPipe upload."
                    )));
                }
                (ApplicationReleaseTask::Publish(review), false, review_id)
            }
            PublishingAction::DiscardApplicationReview => unreachable!("handled by dispatch"),
        };
        let action_id = if staged {
            "publishing.stage-application"
        } else {
            "publishing.publish-application"
        };
        self.spawn_background_operation(
            BackgroundOperationSpec {
                domain: "application-release",
                action: action_id,
                thread_prefix: "application-release",
                failure_label: "Steam application release",
                composition_id: None,
                phase: OperationPhase::Publishing,
            },
            move |context| match task {
                ApplicationReleaseTask::Stage {
                    account,
                    branch,
                    description,
                    runtime_targets,
                } => {
                    application::validate_application_release_request(
                        &executable,
                        &workspace_root,
                        &runtime_targets,
                        branch.as_deref(),
                        &description,
                        &account,
                    )?;
                    context.set_phase(application::OperationPhase::Staging);
                    application::stage_application_release(
                        &executable,
                        &workspace_root,
                        &runtime_targets,
                        branch.as_deref(),
                        &description,
                        &account,
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
        if self.operation_registry.get(&completion.id).is_none() {
            return;
        }
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
        let terminal_status = if succeeded {
            OperationStatus::Succeeded
        } else {
            OperationStatus::Failed {
                message: message.clone(),
            }
        };
        let Some(active) = self
            .operation_registry
            .get(&completion.id)
            .map(|op| op.summary.clone())
        else {
            return;
        };
        self.finish_operation(&completion.id, terminal_status.clone());
        self.revision.0 = self.revision.0.saturating_add(1);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: active.id,
                composition_id: None,
                action: active.action,
                phase: OperationPhase::Complete,
                status: terminal_status,
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
        payload_reviewed: review.payload_reviewed,
        package_root: review.package_root.display().to_string(),
        build_script: review.build_script.display().to_string(),
        files: review.files,
        fingerprint: review.fingerprint.clone(),
        succeeded,
        status: succeeded.then(|| "SteamCMD upload completed".to_owned()),
    }
}
