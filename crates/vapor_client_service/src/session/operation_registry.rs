use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
};

use vapor_client_application::OperationCancellation;
use vapor_client_protocol::{OperationPhase, OperationStatus, OperationSummary};

const OPERATION_HISTORY_LIMIT: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum OperationResourceKey {
    CatalogRefresh,
    WorkspaceScan(PathBuf),
    WorkspaceEdit(PathBuf),
    WorkshopAccount,
    Composition(String),
    Workspace(PathBuf),
    Publishing,
    InstallationTools,
    Profiling,
    Exclusive(String),
}

impl OperationResourceKey {
    fn conflicts_with(&self, other: &Self) -> bool {
        if self == other {
            return true;
        }
        matches!(
            (self, other),
            (Self::WorkspaceScan(scan), Self::WorkspaceEdit(edit))
                | (Self::WorkspaceEdit(edit), Self::WorkspaceScan(scan))
                if scan == edit
        ) || matches!(
            (self, other),
            (Self::Workspace(workspace), Self::WorkspaceEdit(edit))
                | (Self::WorkspaceEdit(edit), Self::Workspace(workspace))
                if workspace == edit
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OperationReservationError {
    Conflict { existing_operation_id: String },
    DuplicateOperationId,
}

#[derive(Debug)]
pub(super) struct RegisteredOperation {
    pub(super) summary: OperationSummary,
    cancellation: Option<OperationCancellation>,
    cancellable: bool,
    resources: Vec<OperationResourceKey>,
}

#[derive(Debug, Default)]
pub(super) struct OperationRegistry {
    active: BTreeMap<String, RegisteredOperation>,
    order: VecDeque<String>,
    history: VecDeque<OperationSummary>,
}

impl OperationRegistry {
    pub(super) fn start(
        &mut self,
        summary: OperationSummary,
        resources: impl IntoIterator<Item = OperationResourceKey>,
        cancellation: Option<OperationCancellation>,
        cancellable: bool,
    ) -> Result<(), OperationReservationError> {
        if self.active.contains_key(&summary.id) {
            return Err(OperationReservationError::DuplicateOperationId);
        }
        let resources = resources.into_iter().collect::<Vec<_>>();
        if let Some(existing_operation_id) = self.active.iter().find_map(|(id, active)| {
            resources
                .iter()
                .any(|requested| {
                    active
                        .resources
                        .iter()
                        .any(|reserved| requested.conflicts_with(reserved))
                })
                .then(|| id.clone())
        }) {
            return Err(OperationReservationError::Conflict {
                existing_operation_id,
            });
        }
        self.order.push_back(summary.id.clone());
        self.active.insert(
            summary.id.clone(),
            RegisteredOperation {
                summary,
                cancellation,
                cancellable,
                resources,
            },
        );
        Ok(())
    }

    pub(super) fn get(&self, id: &str) -> Option<&RegisteredOperation> {
        self.active.get(id)
    }

    pub(super) fn get_mut(&mut self, id: &str) -> Option<&mut RegisteredOperation> {
        self.active.get_mut(id)
    }

    pub(super) fn primary(&self) -> Option<&OperationSummary> {
        self.order
            .front()
            .and_then(|id| self.active.get(id))
            .map(|operation| &operation.summary)
    }

    pub(super) fn active_summaries(&self) -> Vec<OperationSummary> {
        self.order
            .iter()
            .filter_map(|id| self.active.get(id))
            .map(|operation| operation.summary.clone())
            .collect()
    }

    pub(super) fn has_action(&self, action: &str) -> bool {
        self.active
            .values()
            .any(|operation| operation.summary.action == action)
    }

    pub(super) fn history(&self) -> &VecDeque<OperationSummary> {
        &self.history
    }

    pub(super) fn cancellation(&self, id: &str) -> Option<OperationCancellation> {
        self.active
            .get(id)
            .and_then(|operation| operation.cancellation.clone())
    }

    pub(super) fn is_cancellable(&self, id: &str) -> bool {
        self.active
            .get(id)
            .is_some_and(|operation| operation.cancellable)
    }

    pub(super) fn set_status(&mut self, id: &str, status: OperationStatus) -> bool {
        let Some(operation) = self.active.get_mut(id) else {
            return false;
        };
        operation.summary.status = status;
        true
    }

    pub(super) fn remove(&mut self, id: &str) -> Option<RegisteredOperation> {
        self.order.retain(|active_id| active_id != id);
        self.active.remove(id)
    }

    pub(super) fn complete(
        &mut self,
        id: &str,
        status: OperationStatus,
    ) -> Option<OperationSummary> {
        let mut operation = self.remove(id)?;
        operation.summary.phase = OperationPhase::Complete;
        operation.summary.progress = match status {
            OperationStatus::Succeeded => Some(1.0),
            _ => operation.summary.progress,
        };
        operation.summary.status = status;
        let summary = operation.summary;
        self.history.push_back(summary.clone());
        while self.history.len() > OPERATION_HISTORY_LIMIT {
            self.history.pop_front();
        }
        Some(summary)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    pub(super) fn conflicts_with(
        &self,
        resources: &[OperationResourceKey],
    ) -> Option<&OperationSummary> {
        self.order
            .iter()
            .filter_map(|id| self.active.get(id))
            .find_map(|active| {
                resources
                    .iter()
                    .any(|requested| {
                        active
                            .resources
                            .iter()
                            .any(|reserved| requested.conflicts_with(reserved))
                    })
                    .then_some(&active.summary)
            })
    }
}
