//! Bridge live Vapor Development Session telemetry into Client IPC events.

use std::{
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    thread,
    time::Duration,
};
use vapor_client_protocol::{ClientEvent, DevelopmentSessionSnapshot};
use vapor_core::active_development_session;
use vapor_telemetry::TelemetryConsumer;

const EVENT_QUEUE: usize = 256;
const RETRY_DELAY: Duration = Duration::from_millis(500);

pub fn spawn() -> Receiver<ClientEvent> {
    let (sender, receiver) = mpsc::sync_channel(EVENT_QUEUE);
    let worker_sender = sender.clone();
    if let Err(error) = thread::Builder::new()
        .name("vapor-client-development-telemetry".to_owned())
        .spawn(move || observe_sessions(worker_sender))
    {
        let _ = sender.try_send(ClientEvent::DevelopmentSessionChanged {
            session: None,
            status: format!("Could not start telemetry bridge: {error}"),
        });
    }
    receiver
}

fn observe_sessions(sender: SyncSender<ClientEvent>) {
    let mut last_status = None;
    loop {
        match active_development_session() {
            Ok(Some(session)) => {
                let snapshot = DevelopmentSessionSnapshot {
                    id: session.id,
                    configuration: session.configuration,
                    workspace: session.workspace.display().to_string(),
                    address: session.address.clone(),
                    state: session.state,
                    profiling: session.profiling,
                };
                publish_status(
                    &sender,
                    &mut last_status,
                    Some(snapshot.clone()),
                    format!("Connecting to {}", snapshot.configuration),
                );
                match TelemetryConsumer::connect(&session.address) {
                    Ok(mut consumer) => {
                        publish_status(
                            &sender,
                            &mut last_status,
                            Some(snapshot.clone()),
                            "Live".to_owned(),
                        );
                        loop {
                            match consumer.recv() {
                                Ok(Some(event)) => {
                                    if sender.try_send(ClientEvent::Telemetry(event)).is_err() {
                                        // Diagnostic history is deliberately lossy when a
                                        // frontend or the service cannot keep up.
                                    }
                                }
                                Ok(None) => break,
                                Err(error) => {
                                    publish_status(
                                        &sender,
                                        &mut last_status,
                                        Some(snapshot.clone()),
                                        format!("Telemetry disconnected: {error}"),
                                    );
                                    break;
                                }
                            }
                        }
                    }
                    Err(error) => publish_status(
                        &sender,
                        &mut last_status,
                        Some(snapshot),
                        format!("Waiting for telemetry: {error}"),
                    ),
                }
                thread::sleep(RETRY_DELAY);
            }
            Ok(None) => {
                publish_status(
                    &sender,
                    &mut last_status,
                    None,
                    "No active Vapor run".to_owned(),
                );
                thread::sleep(RETRY_DELAY);
            }
            Err(error) => {
                publish_status(
                    &sender,
                    &mut last_status,
                    None,
                    format!("Waiting for session state: {error}"),
                );
                thread::sleep(RETRY_DELAY);
            }
        }
    }
}

fn publish_status(
    sender: &SyncSender<ClientEvent>,
    last_status: &mut Option<(Option<String>, String)>,
    session: Option<DevelopmentSessionSnapshot>,
    status: String,
) {
    let identity = session.as_ref().map(|session| session.id.clone());
    if last_status.as_ref() == Some(&(identity.clone(), status.clone())) {
        return;
    }
    let status_key = status.clone();
    let event = ClientEvent::DevelopmentSessionChanged { session, status };
    match sender.try_send(event) {
        Ok(()) => *last_status = Some((identity, status_key)),
        Err(TrySendError::Full(_)) => {}
        Err(TrySendError::Disconnected(_)) => (),
    }
}
