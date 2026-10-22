//! Shared-service orchestration for Library lifecycle and Auto-Install policy.

use super::{
    BackgroundOperationMessage, BackgroundOperationSpec, ClientSession, LibraryOperationCompletion,
};
use std::path::PathBuf;
use vapor_client_application as application;
use vapor_client_protocol::{
    CatalogLocationKind, ClientAction, ClientEvent, ContentReference, LibraryAction,
    OperationEvent, OperationPhase, OperationStatus, ServiceError, ServiceErrorCode,
};

impl ClientSession {
    pub(super) fn start_library_operation(
        &mut self,
        action: LibraryAction,
    ) -> Result<(), ServiceError> {
        self.reject_if_operation_in_progress()?;

        let selector = library_action_selector(&action);
        if library_action_can_invalidate_composition(&action) {
            let dependent = if let Some(selector) = selector {
                let id = self
                    .catalog
                    .entries()
                    .iter()
                    .find(|entry| {
                        entry.id == selector
                            || entry.published_file_id.as_deref() == Some(selector)
                            || entry.id == selector.split_once('@').map_or(selector, |(id, _)| id)
                    })
                    .map(|entry| entry.id.as_str())
                    .unwrap_or_else(|| selector.split_once('@').map_or(selector, |(id, _)| id));
                let content_id =
                    vapor_client_protocol::ContentReference::parse(id.to_owned(), None)
                        .and_then(|reference| {
                            reference.vapor_id().map_err(|error| error.to_string())
                        })
                        .map_err(|message| ServiceError {
                            code: ServiceErrorCode::InvalidRequest,
                            message: format!("invalid Library content ID `{id}`: {message}"),
                            retryable: false,
                        })?;
                vapor_client_protocol::dependent_composition_id(
                    &self.durable_state.compositions,
                    &content_id,
                )
            } else {
                self.durable_state
                    .compositions
                    .compositions
                    .iter()
                    .find(|composition| {
                        composition.root_package.is_some()
                            || composition
                                .resolved_graph
                                .as_ref()
                                .is_some_and(|graph| !graph.root.id.is_empty())
                    })
                    .map(|composition| composition.id.as_str())
            };
            if let Some(composition_id) = dependent {
                return Err(ServiceError {
                    code: ServiceErrorCode::ActionUnavailable,
                    message: format!(
                        "Content used by composition '{composition_id}' must be changed through that composition's install, update, or repair workflow."
                    ),
                    retryable: false,
                });
            }
        }

        let action_id = library_action_id(&action);
        let bootstrap = self.bootstrap.clone();
        let worker_action = action;
        let completion_action = worker_action.clone();
        self.spawn_background_operation(
            BackgroundOperationSpec {
                domain: "vapor-library",
                action: action_id,
                thread_prefix: "vapor-library",
                failure_label: "Library operation",
                composition_id: None,
                phase: library_action_phase(&worker_action),
            },
            move |context| {
                context.set_phase(application_library_phase(&worker_action));
                let workspace_root =
                    library_action_workspace_root(&worker_action).map(PathBuf::from);
                application::run_library_operation(
                    &bootstrap.application_executable,
                    workspace_root.as_deref(),
                    &worker_action,
                )
            },
            library_result_outcome,
            move |id, result| {
                BackgroundOperationMessage::CompleteLibrary(Box::new(LibraryOperationCompletion {
                    id,
                    action: completion_action,
                    result,
                }))
            },
        )
    }

    pub(super) fn validate_auto_install_target(&self, content_id: &str) -> Result<(), String> {
        let entry = self
            .catalog
            .entries()
            .iter()
            .find(|entry| entry.id == content_id)
            .ok_or_else(|| "Refresh the Library before enabling automatic updates.".to_owned())?;
        let is_cached = entry
            .locations
            .iter()
            .any(|location| location.kind == CatalogLocationKind::Cache);
        if !application::can_auto_install_library_item(
            &entry.kind,
            entry.published_file_id.is_some(),
            is_cached,
            entry.conflicts.is_empty(),
        ) {
            return Err(
                "Automatic updates require an unambiguous cached runtime item published on Workshop."
                    .to_owned(),
            );
        }
        let reference = ContentReference::parse(content_id.to_owned(), None)
            .map_err(|error| format!("invalid Library content ID: {error}"))?;
        let content_id = reference
            .vapor_id()
            .map_err(|error| format!("invalid Library content ID: {error}"))?;
        let policy_has_packagepack_owner = self
            .durable_state
            .library_policies
            .iter()
            .find(|policy| policy.content_id == content_id)
            .is_some_and(|policy| !policy.packagepack_owners.is_empty());
        let installed_composition_owns_item = self
            .durable_state
            .compositions
            .compositions
            .iter()
            .filter(|composition| {
                matches!(
                    &composition.installation,
                    vapor_client_protocol::InstallationState::Installed { .. }
                        | vapor_client_protocol::InstallationState::OutOfDate { .. }
                        | vapor_client_protocol::InstallationState::Partial { .. }
                        | vapor_client_protocol::InstallationState::NeedsRepair { .. }
                )
            })
            .filter_map(|composition| {
                let root = composition.root_package.as_ref()?;
                let root_entry = self
                    .catalog
                    .entries()
                    .iter()
                    .find(|entry| entry.id == root.id)?;
                (root_entry.kind == "packagepack").then_some(composition)
            })
            .any(|composition| {
                composition.resolved_graph.as_ref().is_some_and(|graph| {
                    graph.nodes.iter().any(|node| {
                        node.content.id == content_id.as_str()
                            || node
                                .dependencies
                                .iter()
                                .any(|dependency| dependency.content.id == content_id.as_str())
                    })
                })
            });
        if !policy_has_packagepack_owner && !installed_composition_owns_item {
            return Err(
                "Auto-Install is available only for content owned by a subscribed or installed Packagepack."
                    .to_owned(),
            );
        }
        Ok(())
    }

    pub(super) fn apply_library_completion(&mut self, completion: LibraryOperationCompletion) {
        let (succeeded, message) = match completion.result {
            Ok(message) => {
                self.refresh_catalog();
                (true, message)
            }
            Err(message) => (false, message),
        };
        self.clear_active_operation();
        self.revision.0 = self.revision.0.saturating_add(1);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: completion.id,
                composition_id: None,
                action: library_action_id(&completion.action).to_owned(),
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
    }
}

fn library_action_id(action: &LibraryAction) -> &'static str {
    ClientAction::Library(action.clone()).id()
}

fn library_action_selector(action: &LibraryAction) -> Option<&str> {
    match action {
        LibraryAction::Install { selector, .. }
        | LibraryAction::Disable { selector }
        | LibraryAction::Enable { selector }
        | LibraryAction::Uninstall { selector } => Some(selector),
        LibraryAction::Acquire { .. } => None,
        LibraryAction::Update { selector, .. }
        | LibraryAction::ValidateArtifacts { selector, .. }
        | LibraryAction::Verify { selector }
        | LibraryAction::Repair { selector, .. } => selector.as_deref(),
        LibraryAction::SetAutoInstall { .. } => None,
    }
}

fn library_action_workspace_root(action: &LibraryAction) -> Option<&str> {
    match action {
        LibraryAction::Install { workspace_root, .. }
        | LibraryAction::Acquire { workspace_root, .. }
        | LibraryAction::ValidateArtifacts { workspace_root, .. }
        | LibraryAction::Update { workspace_root, .. }
        | LibraryAction::Repair { workspace_root, .. } => workspace_root.as_deref(),
        _ => None,
    }
}

pub(super) fn library_action_can_invalidate_composition(action: &LibraryAction) -> bool {
    matches!(
        action,
        LibraryAction::Install { .. }
            | LibraryAction::Update { .. }
            | LibraryAction::Repair { .. }
            | LibraryAction::Disable { .. }
            | LibraryAction::Uninstall { .. }
    )
}

fn library_action_phase(action: &LibraryAction) -> OperationPhase {
    match action {
        LibraryAction::Acquire { .. } => OperationPhase::Acquiring,
        LibraryAction::Verify { .. } => OperationPhase::Verifying,
        LibraryAction::ValidateArtifacts { .. } => OperationPhase::Resolving,
        _ => OperationPhase::Installing,
    }
}

fn application_library_phase(action: &LibraryAction) -> application::OperationPhase {
    match action {
        LibraryAction::Acquire { .. } => application::OperationPhase::Acquiring,
        LibraryAction::Verify { .. } => application::OperationPhase::Verifying,
        LibraryAction::ValidateArtifacts { .. } => application::OperationPhase::Resolving,
        _ => application::OperationPhase::Installing,
    }
}

fn library_result_outcome(result: &Result<String, String>) -> Result<String, String> {
    result.clone()
}
