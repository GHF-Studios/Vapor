//! Shared operation identity and lifecycle contracts for Vapor frontends.
//!
//! Frontends own presentation and scheduling. The core contract gives them a
//! common operation identity, lifecycle stream, and cooperative progress API.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static NEXT_OPERATION_ID: AtomicU64 = AtomicU64::new(1);

/// Process-local identity for one operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OperationId(pub u64);

impl OperationId {
    /// Allocate a new operation identity.
    pub fn fresh() -> Self {
        Self(NEXT_OPERATION_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// Coarse execution phase shared by CLI, GUI, and future hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperationPhase {
    Queued,
    Resolving,
    Acquiring,
    Installing,
    Verifying,
    Building,
    Running,
    Staging,
    Publishing,
    Finalizing,
}

/// Terminal or active operation state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum OperationStatus {
    Running,
    Succeeded { summary: String },
    Failed { message: String },
    Cancelled,
}

/// Optional count-based progress for an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// Completed work units.
    pub completed: u64,
    /// Total work units when known.
    pub total: Option<u64>,
    /// Human-readable work unit.
    pub unit: String,
}

/// Serializable event emitted by the shared operation runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationEvent {
    /// Stable identity for this run.
    pub id: OperationId,
    /// Product area that owns the operation (for example `workspace` or `content`).
    pub domain: String,
    /// Frontend-independent action name.
    pub action: String,
    /// Current coarse execution phase.
    pub phase: OperationPhase,
    /// Optional progress update.
    pub progress: Option<Progress>,
    /// Current status.
    pub status: OperationStatus,
}

/// Request metadata supplied by a frontend before dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRequest {
    /// Stable identity for this run.
    pub id: OperationId,
    /// Product area that owns the operation.
    pub domain: String,
    /// Frontend-independent action name.
    pub action: String,
}

/// Callback used by a host to consume operation lifecycle events.
pub type OperationEventSink = Arc<dyn Fn(OperationEvent) + Send + Sync + 'static>;

/// Cooperative reporting context passed into one operation body.
#[derive(Clone)]
pub struct OperationContext {
    request: OperationRequest,
    sink: OperationEventSink,
    state: Arc<Mutex<(OperationPhase, Option<Progress>)>>,
}

impl OperationContext {
    fn new(request: OperationRequest, sink: OperationEventSink) -> Self {
        Self {
            request,
            sink,
            state: Arc::new(Mutex::new((OperationPhase::Running, None))),
        }
    }

    /// The identity of the executing operation.
    pub fn id(&self) -> OperationId {
        self.request.id
    }

    /// Update the coarse phase and notify the host.
    pub fn set_phase(&self, phase: OperationPhase) {
        if let Ok(mut state) = self.state.lock() {
            state.0 = phase;
        }
        self.emit(OperationStatus::Running);
    }

    /// Report count-based progress and notify the host.
    pub fn report_progress(&self, progress: Progress) {
        if let Ok(mut state) = self.state.lock() {
            state.1 = Some(progress);
        }
        self.emit(OperationStatus::Running);
    }

    fn emit(&self, status: OperationStatus) {
        let (phase, progress) = self
            .state
            .lock()
            .map(|state| (state.0, state.1.clone()))
            .unwrap_or((OperationPhase::Running, None));
        (self.sink)(OperationEvent {
            id: self.request.id,
            domain: self.request.domain.clone(),
            action: self.request.action.clone(),
            phase,
            progress,
            status,
        });
    }
}

/// Stateless lifecycle runner. Hosts choose whether and where work is scheduled.
pub struct OperationService;

impl OperationService {
    /// Run a synchronous operation body while emitting queued, running, and terminal events.
    pub fn run<T, E>(
        request: OperationRequest,
        sink: OperationEventSink,
        body: impl FnOnce(OperationContext) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: fmt::Display,
    {
        Self::run_observed(request, sink, body, |result| match result {
            Ok(_) => Ok("Completed".to_owned()),
            Err(error) => Err(error.to_string()),
        })
    }

    /// Run a body whose result is not itself a `Result`, while letting the host
    /// classify its domain outcome for the shared lifecycle stream.
    pub fn run_observed<T>(
        request: OperationRequest,
        sink: OperationEventSink,
        body: impl FnOnce(OperationContext) -> T,
        outcome: impl FnOnce(&T) -> Result<String, String>,
    ) -> T {
        sink(OperationEvent {
            id: request.id,
            domain: request.domain.clone(),
            action: request.action.clone(),
            phase: OperationPhase::Queued,
            progress: None,
            status: OperationStatus::Running,
        });
        let context = OperationContext::new(request, Arc::clone(&sink));
        context.emit(OperationStatus::Running);
        let result = body(context.clone());
        context.set_phase(OperationPhase::Finalizing);
        match outcome(&result) {
            Ok(summary) => context.emit(OperationStatus::Succeeded { summary }),
            Err(message) => context.emit(OperationStatus::Failed { message }),
        }
        result
    }
}
