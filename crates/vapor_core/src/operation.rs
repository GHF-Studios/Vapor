//! Shared operation identity and lifecycle contracts for Vapor frontends.
//!
//! Frontends own presentation and scheduling. The core contract gives them a
//! common operation identity, lifecycle stream, and cooperative progress API.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const MAX_OPERATION_OUTPUT_BYTES: usize = 8 * 1024;

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
    Cancelling,
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
    /// Most recently completed unit of work, when the runner can identify it.
    #[serde(default)]
    pub message: Option<String>,
}

/// Which child-process pipe produced one line of operation output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperationOutputStream {
    Stdout,
    Stderr,
}

/// One bounded line emitted by an operation's child process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationOutput {
    pub stream: OperationOutputStream,
    pub text: String,
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
    /// Optional child-process output line.
    pub output: Option<OperationOutput>,
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

/// Shared cancellation request for one operation and all of its worker contexts.
#[derive(Debug, Clone, Default)]
pub struct OperationCancellation {
    cancelled: Arc<AtomicBool>,
}

impl OperationCancellation {
    /// Request that the operation stop at its next cancellation checkpoint.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Whether the host has requested cancellation.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Cooperative reporting context passed into one operation body.
#[derive(Clone)]
pub struct OperationContext {
    request: OperationRequest,
    sink: OperationEventSink,
    cancellation: OperationCancellation,
    state: Arc<Mutex<(OperationPhase, Option<Progress>)>>,
}

impl OperationContext {
    fn new(
        request: OperationRequest,
        sink: OperationEventSink,
        cancellation: OperationCancellation,
    ) -> Self {
        Self {
            request,
            sink,
            cancellation,
            state: Arc::new(Mutex::new((OperationPhase::Running, None))),
        }
    }

    /// The identity of the executing operation.
    pub fn id(&self) -> OperationId {
        self.request.id
    }

    /// Whether the host has requested cancellation of this operation.
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
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

    /// Forward one child-process output line to subscribed frontends.
    pub fn report_output(&self, mut output: OperationOutput) {
        if output.text.len() > MAX_OPERATION_OUTPUT_BYTES {
            let suffix = " … [truncated]";
            let mut end = MAX_OPERATION_OUTPUT_BYTES.saturating_sub(suffix.len());
            while !output.text.is_char_boundary(end) {
                end -= 1;
            }
            output.text.truncate(end);
            output.text.push_str(suffix);
        }
        self.emit_with_output(OperationStatus::Running, Some(output));
    }

    fn emit(&self, status: OperationStatus) {
        self.emit_with_output(status, None);
    }

    fn emit_with_output(&self, status: OperationStatus, output: Option<OperationOutput>) {
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
            output,
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
        Self::run_observed_cancellable(
            request,
            sink,
            OperationCancellation::default(),
            body,
            outcome,
        )
    }

    /// Run an observed operation with a host-owned cancellation request.
    pub fn run_observed_cancellable<T>(
        request: OperationRequest,
        sink: OperationEventSink,
        cancellation: OperationCancellation,
        body: impl FnOnce(OperationContext) -> T,
        outcome: impl FnOnce(&T) -> Result<String, String>,
    ) -> T {
        sink(OperationEvent {
            id: request.id,
            domain: request.domain.clone(),
            action: request.action.clone(),
            phase: OperationPhase::Queued,
            progress: None,
            output: None,
            status: OperationStatus::Running,
        });
        let context = OperationContext::new(request, Arc::clone(&sink), cancellation.clone());
        context.emit(OperationStatus::Running);
        let result = body(context.clone());
        context.set_phase(OperationPhase::Finalizing);
        if cancellation.is_cancelled() {
            context.emit(OperationStatus::Cancelled);
        } else {
            match outcome(&result) {
                Ok(summary) => context.emit(OperationStatus::Succeeded { summary }),
                Err(message) => context.emit(OperationStatus::Failed { message }),
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    #[test]
    fn cancellation_is_shared_across_operation_context_clones() {
        let cancellation = OperationCancellation::default();
        let worker = cancellation.clone();

        assert!(!worker.is_cancelled());
        cancellation.cancel();
        assert!(worker.is_cancelled());
    }

    #[test]
    fn cancelled_operation_emits_cancelled_terminal_status() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&events);
        let sink: OperationEventSink = Arc::new(move |event| {
            observed.lock().unwrap().push(event);
        });
        let request = OperationRequest {
            id: OperationId::fresh(),
            domain: "workspace".to_owned(),
            action: "build".to_owned(),
        };
        let cancellation = OperationCancellation::default();
        let worker_cancellation = cancellation.clone();

        OperationService::run_observed_cancellable(
            request,
            sink,
            cancellation,
            move |_| {
                worker_cancellation.cancel();
            },
            |_| Ok("must not be reported as successful".to_owned()),
        );

        assert!(matches!(
            events.lock().unwrap().last().map(|event| &event.status),
            Some(OperationStatus::Cancelled)
        ));
    }

    #[test]
    fn operation_context_forwards_child_output_with_its_stream() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&events);
        let sink: OperationEventSink = Arc::new(move |event| {
            observed.lock().unwrap().push(event);
        });
        let request = OperationRequest {
            id: OperationId::fresh(),
            domain: "workspace".to_owned(),
            action: "test".to_owned(),
        };
        let _ = OperationService::run(request, sink, |context| {
            context.report_output(OperationOutput {
                stream: OperationOutputStream::Stderr,
                text: "failure detail".to_owned(),
            });
            Ok::<_, String>(())
        });
        let events: MutexGuard<'_, Vec<OperationEvent>> = events.lock().unwrap();
        assert!(events.iter().any(|event| {
            event.output
                == Some(OperationOutput {
                    stream: OperationOutputStream::Stderr,
                    text: "failure detail".to_owned(),
                })
        }));
    }

    #[test]
    fn operation_context_bounds_oversized_output_without_splitting_utf8() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&events);
        let sink: OperationEventSink = Arc::new(move |event| {
            observed.lock().unwrap().push(event);
        });
        let request = OperationRequest {
            id: OperationId::fresh(),
            domain: "workspace".to_owned(),
            action: "build".to_owned(),
        };
        let _ = OperationService::run(request, sink, |context| {
            context.report_output(OperationOutput {
                stream: OperationOutputStream::Stdout,
                text: format!("{}🌌", "x".repeat(MAX_OPERATION_OUTPUT_BYTES)),
            });
            Ok::<_, String>(())
        });

        let events = events.lock().unwrap();
        let output = events
            .iter()
            .find_map(|event| event.output.as_ref())
            .expect("operation output was forwarded");
        assert!(output.text.len() <= MAX_OPERATION_OUTPUT_BYTES);
        assert!(output.text.is_char_boundary(output.text.len()));
        assert!(output.text.ends_with("[truncated]"));
    }
}
