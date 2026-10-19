use crate::{
    ClientHostConfig, ClientSession, host::ClientHostContext, persistence::ClientStateStore,
};
use std::{
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    sync::mpsc::{self, RecvTimeoutError, SyncSender},
    thread,
    time::{Duration, Instant},
};
use vapor_client_protocol::{
    ClientAction, ClientEvent, DeveloperAction, LegacyStorageSeed, MAX_PROTOCOL_FRAME_BYTES,
    PROTOCOL_VERSION, ProtocolFrame, ServiceError, ServiceErrorCode,
};

const MAX_FRAME_BYTES: usize = MAX_PROTOCOL_FRAME_BYTES;

const SERVICE_IDLE_TIMEOUT: Duration = Duration::from_secs(20);

// Protocol frames carry heap-backed request payloads; keeping this local event
// value inline avoids one allocation for every message on the service channel.
#[allow(clippy::large_enum_variant)]
enum LocalInput {
    Accepted(vapor_client::local_socket::Stream),
    Frame(u64, Result<ProtocolFrame, String>),
    Disconnected(u64),
    ListenerFailed(String),
}

/// Runs the singleton local service until all clients leave and its idle grace ends.
pub fn run_local_service(
    bootstrap: ClientHostConfig,
    host_context: ClientHostContext,
) -> Result<(), String> {
    if bootstrap.fixture_mode {
        return Err("fixture mode is restricted to the isolated stdio simulator".to_owned());
    }
    let endpoint = vapor_client::endpoint_name(
        host_context.app_root.as_deref(),
        &host_context.state_directory,
    );
    let lock = vapor_client::local_socket::startup_lock(&host_context.state_directory)
        .map_err(|error| format!("could not lock Vapor Client startup: {error}"))?;
    if vapor_client::local_socket::connect(&endpoint).is_ok() {
        return Ok(());
    }
    vapor_client::local_socket::remove_stale_endpoint(&endpoint)
        .map_err(|error| format!("could not recover a stale Vapor Client endpoint: {error}"))?;
    let listener = vapor_client::local_socket::bind(&endpoint)
        .map_err(|error| format!("could not claim the Vapor Client service endpoint: {error}"))?;
    drop(lock);

    serve_local(listener, bootstrap, host_context)
}

fn serve_local(
    listener: vapor_client::local_socket::Listener,
    bootstrap: ClientHostConfig,
    host_context: ClientHostContext,
) -> Result<(), String> {
    use interprocess::local_socket::traits::{Listener as _, Stream as _};
    use std::sync::mpsc::{self, SyncSender};

    let (sender, receiver) = mpsc::channel();
    let listener = std::sync::Arc::new(listener);
    let accept_sender = sender.clone();
    let accept_listener = listener.clone();
    thread::Builder::new()
        .name("vapor-client-service-accept".to_owned())
        .spawn(move || {
            loop {
                match accept_listener.accept() {
                    Ok(stream) => {
                        if accept_sender.send(LocalInput::Accepted(stream)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = accept_sender.send(LocalInput::ListenerFailed(error.to_string()));
                        return;
                    }
                }
            }
        })
        .map_err(|error| format!("could not start local service acceptor: {error}"))?;

    let mut clients = std::collections::HashMap::<u64, SyncSender<ProtocolFrame>>::new();
    let mut authenticated = std::collections::HashSet::<u64>::new();
    let mut telemetry_subscribers = std::collections::HashSet::<u64>::new();
    let mut latest_development_status = None;
    let mut session: Option<ClientSession> = None;
    let telemetry_events = crate::development_telemetry::spawn();
    let mut development_status = None;
    let mut next_client_id = 1_u64;
    let mut idle_since = None;

    loop {
        match receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(LocalInput::Accepted(stream)) => {
                let client_id = next_client_id;
                next_client_id = next_client_id.saturating_add(1);
                let (reader, writer) = stream.split();
                let (outbound, outbound_receiver) = mpsc::sync_channel(64);
                clients.insert(client_id, outbound);

                let writer_sender = sender.clone();
                thread::Builder::new()
                    .name(format!("vapor-client-service-write-{client_id}"))
                    .spawn(move || {
                        let mut writer = writer;
                        while let Ok(frame) = outbound_receiver.recv() {
                            if let Err(error) = write_frame(&mut writer, &frame) {
                                let _ =
                                    writer_sender.send(LocalInput::Frame(client_id, Err(error)));
                                break;
                            }
                        }
                    })
                    .map_err(|error| format!("could not start local client writer: {error}"))?;

                let reader_sender = sender.clone();
                thread::Builder::new()
                    .name(format!("vapor-client-service-read-{client_id}"))
                    .spawn(move || {
                        read_local_frames(reader, client_id, reader_sender.clone());
                        let _ = reader_sender.send(LocalInput::Disconnected(client_id));
                    })
                    .map_err(|error| format!("could not start local client reader: {error}"))?;
                idle_since = None;
            }
            Ok(LocalInput::Frame(
                client_id,
                Ok(ProtocolFrame::Connect {
                    protocol_version,
                    legacy_storage,
                }),
            )) => {
                if !clients.contains_key(&client_id) || authenticated.contains(&client_id) {
                    continue;
                }
                let Some(client) = clients.get(&client_id) else {
                    continue;
                };
                let _ = client.try_send(ProtocolFrame::Event(ClientEvent::ServiceReady {
                    protocol_version: PROTOCOL_VERSION,
                }));
                if protocol_version != PROTOCOL_VERSION {
                    let _ = client.try_send(ProtocolFrame::Event(ClientEvent::ProtocolError(ServiceError {
                        code: ServiceErrorCode::UnsupportedProtocol,
                        message: format!("client protocol {protocol_version} is unsupported; service protocol is {PROTOCOL_VERSION}"),
                        retryable: false,
                    })));
                    clients.remove(&client_id);
                    authenticated.remove(&client_id);
                    continue;
                }
                if session.is_none()
                    && let Some(legacy_storage) = legacy_storage
                {
                    match ClientSession::with_persistent_state(
                        bootstrap.clone(),
                        host_context.clone(),
                        ClientStateStore::new(host_context.state_directory.clone()),
                        Some(&legacy_storage),
                    ) {
                        Ok(initialized) => session = Some(initialized),
                        Err(error) => {
                            let _ = client.try_send(ProtocolFrame::Event(
                                ClientEvent::ProtocolError(ServiceError {
                                    code: ServiceErrorCode::PersistenceFailure,
                                    message: error,
                                    retryable: true,
                                }),
                            ));
                            clients.remove(&client_id);
                            continue;
                        }
                    }
                }
                let Some(session) = session.as_ref() else {
                    // Entrypoint health checks may start the process before a
                    // frontend is ready to provide the one-time legacy seed.
                    continue;
                };
                authenticated.insert(client_id);
                let _ = client.try_send(ProtocolFrame::Event(ClientEvent::Snapshot(Box::new(
                    session.snapshot(),
                ))));
            }
            Ok(LocalInput::Frame(client_id, Ok(ProtocolFrame::Request(request)))) => {
                if !authenticated.contains(&client_id) {
                    send_protocol_error(
                        &clients,
                        client_id,
                        "client must negotiate the service protocol before sending requests",
                    );
                    continue;
                }
                let Some(session) = session.as_mut() else {
                    continue;
                };
                let telemetry_subscription = match &request.action {
                    ClientAction::Developer(DeveloperAction::SubscribeTelemetry { enabled }) => {
                        Some(*enabled)
                    }
                    _ => None,
                };
                let response = if request.protocol_version != PROTOCOL_VERSION {
                    vapor_client_protocol::ClientResponse {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: request.request_id,
                        revision: session.snapshot().revision,
                        kind: vapor_client_protocol::ClientResponseKind::Rejected,
                        snapshot: session.snapshot(),
                        error: Some(ServiceError {
                            code: ServiceErrorCode::UnsupportedProtocol,
                            message: format!(
                                "client protocol {} is unsupported; service protocol is {}",
                                request.protocol_version, PROTOCOL_VERSION
                            ),
                            retryable: false,
                        }),
                    }
                } else {
                    session.dispatch(request)
                };
                if response.kind == vapor_client_protocol::ClientResponseKind::Applied
                    && let Some(enabled) = telemetry_subscription
                {
                    if enabled {
                        telemetry_subscribers.insert(client_id);
                    } else {
                        telemetry_subscribers.remove(&client_id);
                    }
                }
                if let Some(client) = clients.get(&client_id) {
                    let _ = client.try_send(ProtocolFrame::Response(Box::new(response)));
                }
                if telemetry_subscription == Some(true)
                    && let Some(event) = development_status.clone()
                    && let Some(client) = clients.get(&client_id)
                {
                    let _ = client.try_send(ProtocolFrame::Event(event));
                }
                broadcast(
                    &clients,
                    ProtocolFrame::Event(ClientEvent::Snapshot(Box::new(session.snapshot()))),
                );
            }
            Ok(LocalInput::Frame(client_id, Ok(_))) => {
                send_protocol_error(
                    &clients,
                    client_id,
                    "service accepts Connect and Request frames only",
                );
            }
            Ok(LocalInput::Frame(client_id, Err(message))) => {
                send_protocol_error(&clients, client_id, &message);
                clients.remove(&client_id);
                authenticated.remove(&client_id);
            }
            Ok(LocalInput::Disconnected(client_id)) => {
                clients.remove(&client_id);
                authenticated.remove(&client_id);
                telemetry_subscribers.remove(&client_id);
                if clients.is_empty() {
                    idle_since = Some(Instant::now());
                }
            }
            Ok(LocalInput::ListenerFailed(error)) => {
                return Err(format!("local service listener failed: {error}"));
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
            Err(RecvTimeoutError::Timeout) => {}
        }

        if let Some(session) = session.as_mut() {
            let events = session.poll_background_operations();
            if !events.is_empty() {
                for event in events {
                    if matches!(&event, ClientEvent::ProfileAnalysis(_)) {
                        broadcast_development_event(
                            &clients,
                            &telemetry_subscribers,
                            event,
                            &mut latest_development_status,
                        );
                    } else {
                        broadcast(&clients, ProtocolFrame::Event(event));
                    }
                }
                broadcast(
                    &clients,
                    ProtocolFrame::Event(ClientEvent::Snapshot(Box::new(session.snapshot()))),
                );
            }
        }
        while let Ok(event) = telemetry_events.try_recv() {
            broadcast_development_event(
                &clients,
                &telemetry_subscribers,
                event,
                &mut development_status,
            );
        }
        if clients.is_empty()
            && session
                .as_ref()
                .is_none_or(|session| !session.has_active_operation())
        {
            let idle = idle_since.get_or_insert_with(Instant::now);
            if idle.elapsed() >= SERVICE_IDLE_TIMEOUT {
                return Ok(());
            }
        } else {
            idle_since = None;
        }
    }
}

fn read_local_frames(
    reader: impl Read,
    client_id: u64,
    sender: std::sync::mpsc::Sender<LocalInput>,
) {
    let mut reader = BufReader::new(reader);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        let read = match reader
            .by_ref()
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut buffer)
        {
            Ok(read) => read,
            Err(error) => {
                let _ = sender.send(LocalInput::Frame(client_id, Err(error.to_string())));
                return;
            }
        };
        if read == 0 {
            return;
        }
        if buffer.len() > MAX_FRAME_BYTES {
            let _ = sender.send(LocalInput::Frame(
                client_id,
                Err("protocol frame exceeds the 32 MiB limit".to_owned()),
            ));
            return;
        }
        if buffer.last() == Some(&b'\n') {
            buffer.pop();
        }
        if buffer.last() == Some(&b'\r') {
            buffer.pop();
        }
        match serde_json::from_slice(&buffer) {
            Ok(frame) => {
                if sender
                    .send(LocalInput::Frame(client_id, Ok(frame)))
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(LocalInput::Frame(
                    client_id,
                    Err(format!("invalid protocol frame: {error}")),
                ));
                return;
            }
        }
    }
}

fn send_protocol_error(
    clients: &std::collections::HashMap<u64, std::sync::mpsc::SyncSender<ProtocolFrame>>,
    client_id: u64,
    message: &str,
) {
    if let Some(client) = clients.get(&client_id) {
        let _ = client.try_send(ProtocolFrame::Event(ClientEvent::ProtocolError(
            ServiceError {
                code: ServiceErrorCode::InvalidRequest,
                message: message.to_owned(),
                retryable: false,
            },
        )));
    }
}

fn broadcast(
    clients: &std::collections::HashMap<u64, std::sync::mpsc::SyncSender<ProtocolFrame>>,
    frame: ProtocolFrame,
) {
    for client in clients.values() {
        let _ = client.try_send(frame.clone());
    }
}

fn broadcast_development_event(
    clients: &std::collections::HashMap<u64, SyncSender<ProtocolFrame>>,
    subscribers: &std::collections::HashSet<u64>,
    event: ClientEvent,
    latest_status: &mut Option<ClientEvent>,
) {
    if matches!(&event, ClientEvent::DevelopmentSessionChanged { .. }) {
        *latest_status = Some(event.clone());
    } else if !matches!(
        &event,
        ClientEvent::Telemetry(_) | ClientEvent::ProfileAnalysis(_)
    ) {
        return;
    }
    let frame = ProtocolFrame::Event(event);
    for client_id in subscribers {
        if let Some(client) = clients.get(client_id) {
            let _ = client.try_send(frame.clone());
        }
    }
}

pub fn run_stdio_service(
    bootstrap: ClientHostConfig,
    host_context: ClientHostContext,
) -> Result<(), String> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = BufReader::new(stdin);
    let seed = read_initialization(&mut input)?;
    let session = if bootstrap.fixture_mode {
        ClientSession::new(bootstrap)
    } else {
        ClientSession::with_persistent_state(
            bootstrap,
            host_context.clone(),
            ClientStateStore::new(host_context.state_directory.clone()),
            Some(&seed),
        )?
    };
    serve(input, BufWriter::new(stdout.lock()), session)
}

fn read_initialization(input: &mut impl BufRead) -> Result<LegacyStorageSeed, String> {
    let mut buffer = Vec::new();
    let read = input
        .by_ref()
        .take((MAX_FRAME_BYTES + 1) as u64)
        .read_until(b'\n', &mut buffer)
        .map_err(|error| format!("failed reading service initialization: {error}"))?;
    if read == 0 {
        return Err("client closed before service initialization".to_owned());
    }
    if buffer.len() > MAX_FRAME_BYTES {
        return Err("service initialization exceeds the 32 MiB limit".to_owned());
    }
    if buffer.last() == Some(&b'\n') {
        buffer.pop();
    }
    if buffer.last() == Some(&b'\r') {
        buffer.pop();
    }
    let frame: ProtocolFrame = serde_json::from_slice(&buffer)
        .map_err(|error| format!("invalid service initialization: {error}"))?;
    match frame {
        ProtocolFrame::Initialize {
            protocol_version,
            legacy_storage,
        } if protocol_version == PROTOCOL_VERSION => Ok(legacy_storage),
        ProtocolFrame::Initialize {
            protocol_version, ..
        } => Err(format!(
            "client protocol {protocol_version} is unsupported; service protocol is {PROTOCOL_VERSION}"
        )),
        _ => Err("expected a service initialization frame".to_owned()),
    }
}

fn serve(
    input: impl BufRead + Send + 'static,
    mut output: impl Write,
    mut session: ClientSession,
) -> Result<(), String> {
    write_frame(
        &mut output,
        &ProtocolFrame::Event(ClientEvent::ServiceReady {
            protocol_version: PROTOCOL_VERSION,
        }),
    )?;
    write_frame(
        &mut output,
        &ProtocolFrame::Event(ClientEvent::Snapshot(Box::new(session.snapshot()))),
    )?;

    let (sender, receiver) = mpsc::sync_channel(8);
    thread::Builder::new()
        .name("client-service-protocol-input".to_owned())
        .spawn(move || read_requests(input, sender))
        .map_err(|error| format!("could not start service input reader: {error}"))?;

    loop {
        match receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(Ok(Some(mut buffer))) => {
                if buffer.last() == Some(&b'\n') {
                    buffer.pop();
                }
                if buffer.last() == Some(&b'\r') {
                    buffer.pop();
                }
                if buffer.is_empty() {
                    continue;
                }
                process_frame(&buffer, &mut output, &mut session)?;
            }
            Ok(Ok(None)) => return Ok(()),
            Ok(Err(message)) => {
                let limit_exceeded = message == "protocol frame limit exceeded";
                write_frame(
                    &mut output,
                    &ProtocolFrame::Event(ClientEvent::ProtocolError(ServiceError {
                        code: ServiceErrorCode::InvalidRequest,
                        message: if limit_exceeded {
                            "protocol frame exceeds the 32 MiB limit".to_owned()
                        } else {
                            message.clone()
                        },
                        retryable: false,
                    })),
                )?;
                return Err(message);
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
            Err(RecvTimeoutError::Timeout) => {}
        }

        write_background_updates(&mut output, &mut session)?;
    }
}

fn read_requests(input: impl BufRead, sender: SyncSender<Result<Option<Vec<u8>>, String>>) {
    let mut input = input;
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        let read = match input
            .by_ref()
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut buffer)
        {
            Ok(read) => read,
            Err(error) => {
                let _ = sender.send(Err(format!("failed reading protocol input: {error}")));
                return;
            }
        };
        if read == 0 {
            let _ = sender.send(Ok(None));
            return;
        }
        if buffer.len() > MAX_FRAME_BYTES {
            let _ = sender.send(Err("protocol frame limit exceeded".to_owned()));
            return;
        }
        if sender.send(Ok(Some(buffer.clone()))).is_err() {
            return;
        }
    }
}

fn process_frame(
    buffer: &[u8],
    output: &mut impl Write,
    session: &mut ClientSession,
) -> Result<(), String> {
    match serde_json::from_slice::<ProtocolFrame>(buffer) {
        Ok(ProtocolFrame::Initialize { .. } | ProtocolFrame::Connect { .. }) => {
            let error = ServiceError {
                code: ServiceErrorCode::InvalidRequest,
                message: "service initialization is only valid as the first frame".to_owned(),
                retryable: false,
            };
            write_frame(
                output,
                &ProtocolFrame::Event(ClientEvent::ProtocolError(error)),
            )?;
        }
        Ok(ProtocolFrame::Request(request)) => {
            let response = if request.protocol_version != PROTOCOL_VERSION {
                vapor_client_protocol::ClientResponse {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: request.request_id,
                    revision: session.snapshot().revision,
                    kind: vapor_client_protocol::ClientResponseKind::Rejected,
                    snapshot: session.snapshot(),
                    error: Some(ServiceError {
                        code: ServiceErrorCode::UnsupportedProtocol,
                        message: format!(
                            "client protocol {} is unsupported; service protocol is {}",
                            request.protocol_version, PROTOCOL_VERSION
                        ),
                        retryable: false,
                    }),
                }
            } else {
                session.dispatch(request)
            };
            write_frame(output, &ProtocolFrame::Response(Box::new(response)))?;
        }
        Ok(_) => {
            write_frame(
                output,
                &ProtocolFrame::Event(ClientEvent::ProtocolError(ServiceError {
                    code: ServiceErrorCode::InvalidRequest,
                    message: "service accepts request frames only".to_owned(),
                    retryable: false,
                })),
            )?;
        }
        Err(error) => {
            write_frame(
                output,
                &ProtocolFrame::Event(ClientEvent::ProtocolError(ServiceError {
                    code: ServiceErrorCode::InvalidRequest,
                    message: format!("invalid protocol frame: {error}"),
                    retryable: false,
                })),
            )?;
        }
    }
    write_background_updates(output, session)?;
    Ok(())
}

fn write_background_updates(
    output: &mut impl Write,
    session: &mut ClientSession,
) -> Result<(), String> {
    let events = session.poll_background_operations();
    if events.is_empty() {
        return Ok(());
    }
    for event in events {
        write_frame(output, &ProtocolFrame::Event(event))?;
    }
    write_frame(
        output,
        &ProtocolFrame::Event(ClientEvent::Snapshot(Box::new(session.snapshot()))),
    )
}

fn write_frame(writer: &mut impl Write, frame: &ProtocolFrame) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(frame)
        .map_err(|error| format!("failed encoding protocol response: {error}"))?;
    if bytes.len() + 1 > MAX_FRAME_BYTES {
        return Err(format!(
            "service protocol response exceeds the {MAX_FRAME_BYTES} byte limit"
        ));
    }
    bytes.push(b'\n');
    writer
        .write_all(&bytes)
        .and_then(|()| writer.flush())
        .map_err(|error| format!("failed writing protocol response: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, path::PathBuf};
    use vapor_client_protocol::{CatalogAction, ClientAction, ClientRequest, ProtocolFrame};

    fn fixture_session() -> ClientSession {
        ClientSession::new(ClientHostConfig {
            application_executable: PathBuf::from("/app/bin/vapor-gui"),
            workshop_app_id: None,
            workspace_root: None,
            active_composition_id: Some("default".to_owned()),
            fixture_mode: true,
            legacy_storage: vapor_client_protocol::LegacyStorageSeed::default(),
            state_directory: None,
        })
    }

    fn frames(output: &[u8]) -> Vec<ProtocolFrame> {
        output
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).expect("server emits valid JSON frames"))
            .collect()
    }

    #[test]
    fn server_handshakes_and_correlates_requests() {
        let request = ProtocolFrame::Request(ClientRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: vapor_client_protocol::RequestId("test-1".into()),
            expected_revision: None,
            action: ClientAction::Catalog(CatalogAction::Refresh),
        });
        let input = format!("{}\n", serde_json::to_string(&request).unwrap());
        let mut output = Vec::new();
        serve(Cursor::new(input), &mut output, fixture_session()).unwrap();
        let frames = frames(&output);
        assert!(matches!(
            frames[0],
            ProtocolFrame::Event(ClientEvent::ServiceReady { .. })
        ));
        assert!(matches!(
            frames[1],
            ProtocolFrame::Event(ClientEvent::Snapshot(_))
        ));
        let ProtocolFrame::Response(response) = &frames[2] else {
            panic!("expected correlated response")
        };
        assert_eq!(response.request_id.0, "test-1");
        assert_eq!(response.revision.0, 1);
    }

    #[test]
    fn unsupported_versions_are_rejected_and_malformed_frames_do_not_kill_the_session() {
        let previous = ProtocolFrame::Request(ClientRequest {
            protocol_version: PROTOCOL_VERSION - 1,
            request_id: vapor_client_protocol::RequestId("previous-client".into()),
            expected_revision: None,
            action: ClientAction::Catalog(CatalogAction::Refresh),
        });
        let future = ProtocolFrame::Request(ClientRequest {
            protocol_version: PROTOCOL_VERSION + 1,
            request_id: vapor_client_protocol::RequestId("future-client".into()),
            expected_revision: None,
            action: ClientAction::Catalog(CatalogAction::Refresh),
        });
        let valid = ProtocolFrame::Request(ClientRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: vapor_client_protocol::RequestId("current-client".into()),
            expected_revision: None,
            action: ClientAction::Catalog(CatalogAction::Refresh),
        });
        let input = format!(
            "{}\n{}\nnot-json\n{}\n",
            serde_json::to_string(&previous).unwrap(),
            serde_json::to_string(&future).unwrap(),
            serde_json::to_string(&valid).unwrap()
        );
        let mut output = Vec::new();
        serve(Cursor::new(input), &mut output, fixture_session()).unwrap();
        let frames = frames(&output);
        assert!(matches!(
            &frames[2],
            ProtocolFrame::Response(response)
                if response.error.as_ref().is_some_and(|error| error.code == ServiceErrorCode::UnsupportedProtocol)
        ));
        assert!(matches!(
            &frames[3],
            ProtocolFrame::Response(response)
                if response.error.as_ref().is_some_and(|error| error.code == ServiceErrorCode::UnsupportedProtocol)
        ));
        assert!(matches!(
            &frames[4],
            ProtocolFrame::Event(ClientEvent::ProtocolError(error))
                if error.code == ServiceErrorCode::InvalidRequest
        ));
        assert!(matches!(
            &frames[5],
            ProtocolFrame::Response(response) if response.revision.0 == 1
        ));
    }

    #[test]
    fn oversized_input_is_rejected_with_a_bounded_protocol_error() {
        let mut input = vec![b'x'; MAX_FRAME_BYTES + 1];
        input.push(b'\n');
        let mut output = Vec::new();
        assert!(serve(Cursor::new(input), &mut output, fixture_session()).is_err());
        assert!(matches!(
            frames(&output).last(),
            Some(ProtocolFrame::Event(ClientEvent::ProtocolError(error)))
                if error.message.contains("32 MiB limit")
        ));
    }

    #[test]
    fn development_telemetry_only_reaches_subscribed_clients() {
        let (first_sender, first_receiver) = std::sync::mpsc::sync_channel(1);
        let (second_sender, second_receiver) = std::sync::mpsc::sync_channel(1);
        let clients = std::collections::HashMap::from([(1, first_sender), (2, second_sender)]);
        let subscribers = std::collections::HashSet::from([2]);
        let mut latest_status = None;

        broadcast_development_event(
            &clients,
            &subscribers,
            ClientEvent::Telemetry(vapor_telemetry::TelemetryEvent::new(
                "engine",
                vapor_telemetry::TelemetryPayload::Lifecycle {
                    state: "running".into(),
                    detail: None,
                },
            )),
            &mut latest_status,
        );

        assert!(first_receiver.try_recv().is_err());
        assert!(matches!(
            second_receiver.try_recv(),
            Ok(ProtocolFrame::Event(ClientEvent::Telemetry(_)))
        ));

        broadcast_development_event(
            &clients,
            &subscribers,
            ClientEvent::ProfileAnalysis(Err("trace analysis failed".to_owned())),
            &mut latest_status,
        );
        assert!(first_receiver.try_recv().is_err());
        assert!(matches!(
            second_receiver.try_recv(),
            Ok(ProtocolFrame::Event(ClientEvent::ProfileAnalysis(Err(_))))
        ));
    }
}
