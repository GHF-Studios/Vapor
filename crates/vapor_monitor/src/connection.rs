use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use vapor_core::active_development_session;
use vapor_telemetry::TelemetryConsumer;

use crate::model::{MonitorMessage, SessionInfo};

pub fn spawn_connection_worker(explicit_address: Option<String>, sender: Sender<MonitorMessage>) {
    thread::spawn(move || {
        let mut last_waiting = String::new();
        loop {
            let (address, session) = match explicit_address.as_ref() {
                Some(address) => (address.clone(), None),
                None => match active_development_session() {
                    Ok(Some(session)) => {
                        let info = SessionInfo {
                            id: session.id.clone(),
                            configuration: session.configuration.clone(),
                            workspace: session.workspace.display().to_string(),
                            address: session.address.clone(),
                        };
                        (session.address, Some(info))
                    }
                    Ok(None) => {
                        wait(
                            &sender,
                            &mut last_waiting,
                            "Waiting for `vapor run`…".to_owned(),
                            Duration::from_millis(500),
                        );
                        continue;
                    }
                    Err(error) => {
                        wait(
                            &sender,
                            &mut last_waiting,
                            format!("Waiting for Vapor session state: {error}"),
                            Duration::from_secs(1),
                        );
                        continue;
                    }
                },
            };

            match TelemetryConsumer::connect(&address) {
                Ok(mut consumer) => {
                    last_waiting.clear();
                    if sender.send(MonitorMessage::Connected(session)).is_err() {
                        return;
                    }
                    loop {
                        match consumer.recv() {
                            Ok(Some(event)) => {
                                if sender.send(MonitorMessage::Event(event)).is_err() {
                                    return;
                                }
                            }
                            Ok(None) => break,
                            Err(error) => {
                                let _ = sender.send(MonitorMessage::Disconnected(format!(
                                    "Telemetry disconnected: {error}"
                                )));
                                break;
                            }
                        }
                    }
                }
                Err(error) => {
                    wait(
                        &sender,
                        &mut last_waiting,
                        format!("Waiting for telemetry at {address}: {error}"),
                        Duration::from_millis(500),
                    );
                    continue;
                }
            }

            thread::sleep(Duration::from_millis(500));
        }
    });
}

fn wait(
    sender: &Sender<MonitorMessage>,
    last_waiting: &mut String,
    message: String,
    duration: Duration,
) {
    if message != *last_waiting {
        let _ = sender.send(MonitorMessage::Waiting(message.clone()));
        *last_waiting = message;
    }
    thread::sleep(duration);
}
