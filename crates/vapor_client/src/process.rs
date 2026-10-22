use crate::{
    local_socket,
    transport::{encode_frame, read_frame},
};
#[cfg(not(windows))]
use interprocess::local_socket::traits::RecvHalf as _;
use interprocess::local_socket::traits::Stream as _;
use std::{
    collections::VecDeque,
    env,
    ffi::OsString,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use vapor_client_protocol::{
    ClientAction, ClientEvent, ClientRequest, ClientRevision, ClientSnapshot, LegacyStorageSeed,
    PROTOCOL_VERSION, ProtocolFrame, RequestId,
};

// The first production connection may need to initialize durable state and
// project the local Superworkspace before its initial snapshot is available.
// Keep that bootstrap bounded, but allow a cold start more than a fixed
// eight-second network-style handshake window.
const START_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const CONNECT_RETRY: Duration = Duration::from_millis(40);
const SERVICE_LOG_LIMIT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_HANDSHAKE_BUFFERED_EVENTS: usize = 16;

#[derive(Debug, Clone)]
pub struct ClientHostConfig {
    pub application_executable: PathBuf,
    pub workshop_app_id: Option<u32>,
    pub workspace_root: Option<PathBuf>,
    pub active_composition_id: Option<String>,
    pub fixture_mode: bool,
    pub legacy_storage: LegacyStorageSeed,
    pub state_directory: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    Spawn(String),
    Protocol(String),
    ServiceExited(Option<i32>),
    Timeout(&'static str),
    ServiceStartupTimeout(Vec<String>),
    Io(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(message) => {
                write!(formatter, "could not start Vapor Client service: {message}")
            }
            Self::Protocol(message) => {
                write!(formatter, "Vapor Client service protocol error: {message}")
            }
            Self::ServiceExited(code) => write!(
                formatter,
                "Vapor Client service exited unexpectedly ({code:?})"
            ),
            Self::Timeout(phase) => {
                write!(formatter, "Vapor Client service timed out during {phase}")
            }
            Self::ServiceStartupTimeout(lines) => {
                write!(
                    formatter,
                    "Vapor Client service timed out during shared service startup"
                )?;
                if !lines.is_empty() {
                    write!(formatter, "; recent service output: {}", lines.join(" | "))?;
                }
                Ok(())
            }
            Self::Io(message) => write!(formatter, "Vapor Client service I/O failed: {message}"),
        }
    }
}

impl std::error::Error for ClientError {}

pub struct Client {
    child: Option<Child>,
    writer: Option<BufWriter<Box<dyn Write + Send>>>,
    incoming: Receiver<Result<ProtocolFrame, ClientError>>,
    next_request: u64,
    request_prefix: String,
    latest_snapshot: ClientSnapshot,
    shared_service: Option<(PathBuf, ClientHostConfig)>,
}

#[derive(Default)]
struct ServiceHandshake {
    ready: bool,
    snapshot: Option<ClientSnapshot>,
    require_snapshot: bool,
    pending_events: VecDeque<ProtocolFrame>,
}

impl ServiceHandshake {
    fn new(require_snapshot: bool) -> Self {
        Self {
            ready: false,
            snapshot: None,
            require_snapshot,
            pending_events: VecDeque::new(),
        }
    }

    fn accept(&mut self, frame: ProtocolFrame) -> Result<Option<ClientSnapshot>, ClientError> {
        match frame {
            ProtocolFrame::Event(ClientEvent::ServiceReady { protocol_version }) => {
                if protocol_version != PROTOCOL_VERSION {
                    return Err(ClientError::Protocol(format!(
                        "service protocol {protocol_version} does not match client protocol {PROTOCOL_VERSION}"
                    )));
                }
                self.ready = true;
            }
            ProtocolFrame::Event(ClientEvent::Snapshot(snapshot)) => {
                self.snapshot = Some(*snapshot);
            }
            ProtocolFrame::Event(ClientEvent::ProtocolError(error)) => {
                return Err(ClientError::Protocol(error.message));
            }
            ProtocolFrame::Event(ClientEvent::ServiceStopping) => {
                return Err(ClientError::ServiceExited(None));
            }
            frame @ ProtocolFrame::Event(_) => {
                if self.pending_events.len() == MAX_HANDSHAKE_BUFFERED_EVENTS {
                    return Err(ClientError::Protocol(
                        "service sent too many events before completing connection".into(),
                    ));
                }
                self.pending_events.push_back(frame);
            }
            _ => {
                return Err(ClientError::Protocol(
                    "service sent a non-event frame during connection".into(),
                ));
            }
        }
        Ok(
            if self.ready && (self.snapshot.is_some() || !self.require_snapshot) {
                Some(self.snapshot.take().unwrap_or_else(empty_snapshot))
            } else {
                None
            },
        )
    }
}

impl Client {
    /// Connects to this installation's shared service, starting it once if needed.
    pub fn connect_or_start(
        service_path: &Path,
        bootstrap: ClientHostConfig,
    ) -> Result<Self, ClientError> {
        if bootstrap.fixture_mode {
            return Err(ClientError::Protocol(
                "fixture clients cannot attach to the shared production service".into(),
            ));
        }
        let context = resolve_host_context(&bootstrap)?;
        let endpoint =
            local_socket::endpoint_name(context.app_root.as_deref(), &context.state_directory);
        match Self::connect_existing(&endpoint, Some(bootstrap.legacy_storage.clone())) {
            Ok(mut client) => {
                client.shared_service = Some((service_path.to_path_buf(), bootstrap));
                Ok(client)
            }
            Err(error @ ClientError::Protocol(_)) => Err(error),
            Err(_) => {
                ensure_service(service_path, &bootstrap, &context, &endpoint)?;
                let mut client =
                    Self::connect_existing(&endpoint, Some(bootstrap.legacy_storage.clone()))?;
                client.shared_service = Some((service_path.to_path_buf(), bootstrap));
                Ok(client)
            }
        }
    }

    /// Starts or finds the shared service and waits until it accepts a local connection.
    pub fn ensure_service(
        service_path: &Path,
        bootstrap: ClientHostConfig,
    ) -> Result<(), ClientError> {
        if bootstrap.fixture_mode {
            return Err(ClientError::Protocol(
                "fixture mode cannot start the shared production service".into(),
            ));
        }
        let context = resolve_host_context(&bootstrap)?;
        let endpoint =
            local_socket::endpoint_name(context.app_root.as_deref(), &context.state_directory);
        ensure_service(service_path, &bootstrap, &context, &endpoint)
    }

    /// Reconnects after service loss and refreshes this client's snapshot.
    /// Pending requests are never replayed because their prior outcome may be unknown.
    pub fn reconnect(&mut self) -> Result<&ClientSnapshot, ClientError> {
        let (service_path, bootstrap) = self.shared_service.clone().ok_or_else(|| {
            ClientError::Protocol("this client is not attached to a shared service".into())
        })?;
        let mut replacement = Self::connect_or_start(&service_path, bootstrap)?;
        self.child = replacement.child.take();
        self.writer = replacement.writer.take();
        let (_, disconnected) = mpsc::sync_channel(1);
        self.incoming = std::mem::replace(&mut replacement.incoming, disconnected);
        self.next_request = replacement.next_request;
        self.request_prefix = replacement.request_prefix.clone();
        self.latest_snapshot = replacement.latest_snapshot.clone();
        self.shared_service = replacement.shared_service.take();
        Ok(&self.latest_snapshot)
    }

    /// Starts the isolated stdio endpoint used only by fixture simulations.
    pub fn spawn(service_path: &Path, bootstrap: ClientHostConfig) -> Result<Self, ClientError> {
        let mut command = Command::new(service_path);
        configure_background_process(&mut command);
        command
            .arg("--stdio")
            .arg("--application-executable")
            .arg(&bootstrap.application_executable)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(app_id) = bootstrap.workshop_app_id {
            command.arg("--workshop-app-id").arg(app_id.to_string());
        }
        if let Some(workspace) = bootstrap.workspace_root.as_ref() {
            command.arg("--workspace").arg(workspace);
        }
        if let Some(composition_id) = bootstrap.active_composition_id.as_ref() {
            command.arg("--active-composition").arg(composition_id);
        }
        if let Some(state_directory) = bootstrap.state_directory.as_ref() {
            command.arg("--state-dir").arg(state_directory);
        }
        if bootstrap.fixture_mode {
            command.arg("--fixture");
        }
        let mut child = command
            .spawn()
            .map_err(|error| ClientError::Spawn(error.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ClientError::Spawn("service stdin was not piped".into()))?;
        let mut writer = BufWriter::new(stdin);
        if let Err(error) = write_frame(
            &mut writer,
            &ProtocolFrame::Initialize {
                protocol_version: PROTOCOL_VERSION,
                legacy_storage: bootstrap.legacy_storage,
            },
        ) {
            // A child that exits before accepting its first frame must not be
            // left running or unreaped when initialization fails.
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ClientError::Spawn("service stdout was not piped".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ClientError::Spawn("service stderr was not piped".into()))?;
        thread::Builder::new()
            .name("client-service-stderr".into())
            .spawn(move || drain_stderr(stderr))
            .map_err(|error| ClientError::Spawn(error.to_string()))?;
        let (sender, incoming) = mpsc::sync_channel(64);
        thread::Builder::new()
            .name("client-service-protocol".into())
            .spawn(move || read_protocol(stdout, sender))
            .map_err(|error| ClientError::Spawn(error.to_string()))?;
        let mut client = Self::new(Some(child), Box::new(writer), incoming);
        client.await_ready()?;
        Ok(client)
    }

    fn connect_existing(
        endpoint: &str,
        legacy_storage: Option<LegacyStorageSeed>,
    ) -> Result<Self, ClientError> {
        let stream =
            local_socket::connect(endpoint).map_err(|error| ClientError::Io(error.to_string()))?;
        Self::connect_stream(stream, legacy_storage.is_some(), legacy_storage)
    }

    fn connect_stream(
        stream: local_socket::Stream,
        require_snapshot: bool,
        legacy_storage: Option<LegacyStorageSeed>,
    ) -> Result<Self, ClientError> {
        // Windows named pipes do not support socket-style read timeouts. In
        // particular, interprocess returns Unsupported here before the client
        // can send its protocol handshake, which made startup retry until the
        // outer 30-second deadline. The service sends its readiness frame as
        // soon as it receives Connect; use the blocking named-pipe handshake on
        // Windows and keep the bounded socket timeout on Unix.
        #[cfg(not(windows))]
        stream
            .set_recv_timeout(Some(START_TIMEOUT))
            .map_err(|error| ClientError::Io(error.to_string()))?;
        let (reader, writer) = stream.split();
        let mut writer = BufWriter::new(Box::new(writer) as Box<dyn Write + Send>);
        write_frame(
            &mut writer,
            &ProtocolFrame::Connect {
                protocol_version: PROTOCOL_VERSION,
                legacy_storage,
            },
        )?;
        let mut reader = BufReader::new(reader);
        let mut handshake = ServiceHandshake::new(require_snapshot);
        loop {
            let frame = read_frame(&mut reader)
                .map_err(ClientError::Protocol)?
                .ok_or(ClientError::ServiceExited(None))?;
            if let Some(snapshot) = handshake.accept(frame)? {
                #[cfg(not(windows))]
                reader
                    .get_ref()
                    .set_timeout(None)
                    .map_err(|error| ClientError::Io(error.to_string()))?;
                let read = reader.into_inner();
                let (sender, incoming) = mpsc::sync_channel(64);
                for frame in std::mem::take(&mut handshake.pending_events) {
                    sender
                        .send(Ok(frame))
                        .map_err(|_| ClientError::ServiceExited(None))?;
                }
                thread::Builder::new()
                    .name("client-service-protocol".into())
                    .spawn(move || read_protocol(read, sender))
                    .map_err(|error| ClientError::Spawn(error.to_string()))?;
                let mut client = Self::new(None, Box::new(writer), incoming);
                client.latest_snapshot = snapshot;
                return Ok(client);
            }
        }
    }

    fn new(
        child: Option<Child>,
        writer: Box<dyn Write + Send>,
        incoming: Receiver<Result<ProtocolFrame, ClientError>>,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self {
            child,
            writer: Some(BufWriter::new(writer)),
            incoming,
            next_request: 1,
            request_prefix: format!("client-{}-{now}", std::process::id()),
            latest_snapshot: empty_snapshot(),
            shared_service: None,
        }
    }

    pub fn latest_snapshot(&self) -> &ClientSnapshot {
        &self.latest_snapshot
    }

    pub fn send(
        &mut self,
        action: ClientAction,
        expected_revision: Option<ClientRevision>,
    ) -> Result<RequestId, ClientError> {
        let request_id = RequestId(format!("{}-{}", self.request_prefix, self.next_request));
        self.next_request = self.next_request.saturating_add(1);
        let frame = ProtocolFrame::Request(ClientRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.clone(),
            expected_revision,
            action,
        });
        self.write_frame(&frame)?;
        Ok(request_id)
    }

    /// Send one JSON-encoded LSP message without creating an app-state operation.
    pub fn send_language_server_message(&mut self, message: String) -> Result<(), ClientError> {
        self.write_frame(&ProtocolFrame::LanguageServerMessage(message))
    }

    /// Polls for one frame without blocking or reconnecting.
    pub fn try_recv(&mut self) -> Result<Option<ProtocolFrame>, ClientError> {
        match self.incoming.try_recv() {
            Ok(frame) => self.accept_frame(frame).map(Some),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                let status = self
                    .child
                    .as_mut()
                    .and_then(|child| child.try_wait().ok().flatten())
                    .and_then(|status| status.code());
                Err(ClientError::ServiceExited(status))
            }
        }
    }

    /// Waits for one frame up to `timeout`; reconnecting is an explicit operation.
    pub fn recv_timeout(&mut self, timeout: Duration) -> Result<ProtocolFrame, ClientError> {
        match self.incoming.recv_timeout(timeout) {
            Ok(frame) => self.accept_frame(frame),
            Err(RecvTimeoutError::Timeout) => Err(ClientError::Timeout("waiting for a response")),
            Err(RecvTimeoutError::Disconnected) => {
                let status = self
                    .child
                    .as_mut()
                    .and_then(|child| child.try_wait().ok().flatten())
                    .and_then(|status| status.code());
                Err(ClientError::ServiceExited(status))
            }
        }
    }

    pub fn shutdown(mut self, timeout: Duration) -> Result<(), ClientError> {
        self.writer.take();
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        let deadline = Instant::now() + timeout.min(SHUTDOWN_TIMEOUT);
        loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| ClientError::Io(error.to_string()))?
            {
                return if status.success() {
                    Ok(())
                } else {
                    Err(ClientError::ServiceExited(status.code()))
                };
            }
            if Instant::now() >= deadline {
                child
                    .kill()
                    .map_err(|error| ClientError::Io(error.to_string()))?;
                let _ = child.wait();
                return Err(ClientError::Timeout("fixture service shutdown"));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn await_ready(&mut self) -> Result<(), ClientError> {
        let deadline = Instant::now() + START_TIMEOUT;
        let mut handshake = ServiceHandshake::new(true);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(ClientError::Timeout("service startup"));
            }
            let frame = self.recv_timeout(remaining)?;
            if let Some(snapshot) = handshake.accept(frame)? {
                self.latest_snapshot = snapshot;
                return Ok(());
            }
        }
    }

    fn accept_frame(
        &mut self,
        frame: Result<ProtocolFrame, ClientError>,
    ) -> Result<ProtocolFrame, ClientError> {
        let frame = frame?;
        if let ProtocolFrame::Event(ClientEvent::Snapshot(snapshot)) = &frame {
            self.latest_snapshot = snapshot.as_ref().clone();
        }
        if let ProtocolFrame::Response(response) = &frame {
            self.latest_snapshot = response.snapshot.clone();
        }
        Ok(frame)
    }

    fn write_frame(&mut self, frame: &ProtocolFrame) -> Result<(), ClientError> {
        let writer = self
            .writer
            .as_mut()
            .ok_or(ClientError::ServiceExited(None))?;
        write_frame(writer, frame)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(writer) = self.writer.as_mut() {
            let _ = write_frame(writer, &ProtocolFrame::Disconnect);
        }
        self.writer.take();
        if let Some(child) = self.child.as_mut()
            && child.try_wait().ok().flatten().is_none()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn resolve_host_context(
    bootstrap: &ClientHostConfig,
) -> Result<crate::ClientHostContext, ClientError> {
    let environment = env::vars_os().collect();
    crate::ClientHostContext::resolve(
        crate::ClientHostArguments {
            application_executable: bootstrap.application_executable.clone(),
            workspace_root: bootstrap.workspace_root.clone(),
            state_directory: bootstrap.state_directory.clone(),
        },
        &environment,
        crate::HostPlatform::current(),
    )
    .map_err(ClientError::Io)
}

fn ensure_service(
    service_path: &Path,
    bootstrap: &ClientHostConfig,
    context: &crate::ClientHostContext,
    endpoint: &str,
) -> Result<(), ClientError> {
    ensure_service_with_timeout(service_path, bootstrap, context, endpoint, START_TIMEOUT)
}

fn ensure_service_with_timeout(
    service_path: &Path,
    bootstrap: &ClientHostConfig,
    context: &crate::ClientHostContext,
    endpoint: &str,
    startup_timeout: Duration,
) -> Result<(), ClientError> {
    let lock = local_socket::bootstrap_lock(&context.state_directory)
        .map_err(|error| ClientError::Io(error.to_string()))?;
    match Client::connect_existing(endpoint, None) {
        Ok(client) => {
            drop(client);
            drop(lock);
            return Ok(());
        }
        Err(error @ ClientError::Protocol(_)) => {
            drop(lock);
            return Err(error);
        }
        Err(_) => {}
    }
    if !endpoint_unavailable(endpoint) {
        drop(lock);
        return Err(ClientError::Io(
            "the local service endpoint exists but is not connectable".into(),
        ));
    }
    local_socket::remove_stale_endpoint(endpoint)
        .map_err(|error| ClientError::Io(error.to_string()))?;
    let mut spawned_service = Some(spawn_shared_service(
        service_path,
        bootstrap,
        &context.state_directory,
    )?);

    let deadline = Instant::now() + startup_timeout;
    loop {
        match Client::connect_existing(endpoint, None) {
            Ok(client) => {
                drop(client);
                if let Some(service) = spawned_service.take() {
                    service.detach()?;
                }
                return Ok(());
            }
            Err(error @ ClientError::Protocol(_)) => return Err(error),
            Err(error) if Instant::now() >= deadline => {
                return Err(match error {
                    ClientError::Io(_) | ClientError::ServiceExited(_) => {
                        let lines = spawned_service
                            .as_ref()
                            .map(|service| startup_diagnostics(&service.log_path))
                            .unwrap_or_default();
                        drop(spawned_service.take());
                        ClientError::ServiceStartupTimeout(lines)
                    }
                    other => other,
                });
            }
            Err(_) => {
                if let Some(child) = spawned_service.as_mut()
                    && let Some(status) = child
                        .child
                        .as_mut()
                        .expect("attached service child is present")
                        .try_wait()
                        .map_err(|error| ClientError::Io(error.to_string()))?
                {
                    return Err(ClientError::ServiceExited(status.code()));
                }
                thread::sleep(CONNECT_RETRY);
            }
        }
    }
}

fn endpoint_unavailable(endpoint: &str) -> bool {
    match local_socket::connect(endpoint) {
        Ok(stream) => {
            drop(stream);
            false
        }
        Err(error) => matches!(
            error.kind(),
            std::io::ErrorKind::NotFound
                | std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::AddrNotAvailable
                | std::io::ErrorKind::WouldBlock
        ),
    }
}

fn spawn_shared_service(
    service_path: &Path,
    bootstrap: &ClientHostConfig,
    state_directory: &Path,
) -> Result<SpawnedService, ClientError> {
    let log_path = service_log_path(state_directory);
    prepare_service_log(&log_path).map_err(|error| {
        ClientError::Spawn(format!(
            "could not prepare the Vapor Client service log: {error}"
        ))
    })?;
    let service_log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| {
            ClientError::Spawn(format!(
                "could not open the Vapor Client service log '{}': {error}",
                log_path.display()
            ))
        })?;
    let mut command = Command::new(service_path);
    configure_background_process(&mut command);
    command
        .arg("--serve")
        .arg("--application-executable")
        .arg(&bootstrap.application_executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(service_log));
    if let Some(app_id) = bootstrap.workshop_app_id {
        command.arg("--workshop-app-id").arg(app_id.to_string());
    }
    if let Some(workspace) = bootstrap.workspace_root.as_ref() {
        command.arg("--workspace").arg(workspace);
    }
    if let Some(composition_id) = bootstrap.active_composition_id.as_ref() {
        command.arg("--active-composition").arg(composition_id);
    }
    if let Some(state_directory) = bootstrap.state_directory.as_ref() {
        command.arg("--state-dir").arg(state_directory);
    }
    let child = command
        .spawn()
        .map_err(|error| ClientError::Spawn(error.to_string()))?;
    Ok(SpawnedService {
        child: Some(child),
        log_path,
        detached: false,
    })
}

struct SpawnedService {
    child: Option<Child>,
    log_path: PathBuf,
    detached: bool,
}

impl SpawnedService {
    fn detach(mut self) -> Result<(), ClientError> {
        let child = self
            .child
            .take()
            .expect("attached service child is present");
        let child = Arc::new(Mutex::new(Some(child)));
        let reaper_child = Arc::clone(&child);
        let reaper = thread::Builder::new()
            .name("client-service-reaper".into())
            .spawn(move || {
                if let Some(mut child) = reaper_child
                    .lock()
                    .expect("service reaper child mutex is not poisoned")
                    .take()
                {
                    let _ = child.wait();
                }
            });
        if let Err(error) = reaper {
            if let Some(mut child) = child
                .lock()
                .expect("service reaper child mutex is not poisoned")
                .take()
            {
                let _ = child.kill();
                let _ = child.wait();
            }
            return Err(ClientError::Spawn(format!(
                "could not supervise the local service process: {error}"
            )));
        }
        self.detached = true;
        Ok(())
    }
}

impl Drop for SpawnedService {
    fn drop(&mut self) {
        if !self.detached
            && let Some(child) = self.child.as_mut()
            && child.try_wait().ok().flatten().is_none()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn service_log_path(state_directory: &Path) -> PathBuf {
    state_directory.join("logs").join("client-service.log")
}

fn prepare_service_log(path: &Path) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("service log path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() >= SERVICE_LOG_LIMIT_BYTES) {
        let previous = path.with_extension("log.1");
        let _ = std::fs::remove_file(&previous);
        std::fs::rename(path, previous)?;
    }
    Ok(())
}

fn startup_diagnostics(path: &Path) -> Vec<String> {
    const MAX_LINES: usize = 24;
    std::fs::read_to_string(path)
        .map(|source| {
            source
                .lines()
                .rev()
                .take(MAX_LINES)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(windows)]
fn configure_background_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}

#[cfg(not(windows))]
fn configure_background_process(_command: &mut Command) {}

fn write_frame(writer: &mut impl Write, frame: &ProtocolFrame) -> Result<(), ClientError> {
    let bytes = encode_frame(frame).map_err(ClientError::Protocol)?;
    writer
        .write_all(&bytes)
        .and_then(|()| writer.flush())
        .map_err(|error| ClientError::Io(error.to_string()))
}

pub fn service_sibling_path(gui_executable: &Path) -> PathBuf {
    service_sibling_path_for(gui_executable, cfg!(windows))
}

fn service_sibling_path_for(gui_executable: &Path, windows: bool) -> PathBuf {
    let mut filename: OsString = "vapor-client-service".into();
    if windows {
        filename.push(".exe");
    }
    gui_executable
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(filename)
}

fn read_protocol(input: impl Read, sender: SyncSender<Result<ProtocolFrame, ClientError>>) {
    let mut input = BufReader::new(input);
    loop {
        match read_frame(&mut input) {
            Ok(Some(frame)) => {
                if sender.send(Ok(frame)).is_err() {
                    return;
                }
            }
            Ok(None) => return,
            Err(error) => {
                let _ = sender.send(Err(ClientError::Protocol(error)));
                return;
            }
        }
    }
}

fn drain_stderr(stderr: impl Read) {
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    while reader.read_line(&mut line).is_ok_and(|size| size > 0) {
        eprintln!("[vapor-client-service] {}", line.trim_end());
        line.clear();
    }
}

fn empty_snapshot() -> ClientSnapshot {
    use vapor_client_protocol::{ClientRevision, HostSummary};
    ClientSnapshot {
        revision: ClientRevision(0),
        host: HostSummary {
            app_root: None,
            workspace: None,
            mode: "starting".into(),
            platform: String::new(),
            steam_client_available: false,
        },
        root_distribution: vapor_client_protocol::RootDistributionIdentitySnapshot::default(),
        active_composition_id: None,
        durable_state: vapor_client_protocol::DurableStateSnapshot::default(),
        active_operation: None,
        active_operations: Vec::new(),
        operation_history: Vec::new(),
        composition_preview: None,
        catalog: Vec::new(),
        catalog_status: vapor_client_protocol::CatalogStatus::default(),
        catalog_search: vapor_client_protocol::CatalogSearchSnapshot::default(),
        workspace_content: vapor_client_protocol::WorkspaceContentSnapshot::default(),
        superworkspace: vapor_client_protocol::SuperworkspaceSnapshot::default(),
        workshop: vapor_client_protocol::WorkshopSnapshot::default(),
        root_publishing: None,
        application_release: None,
        profile_traces: Vec::new(),
        profile_trace_error: None,
        installation_role: vapor_client_protocol::InstallationRoleSnapshot::default(),
        selected_content_id: None,
        available_actions: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Client, ClientError, ClientHostConfig, ServiceHandshake, empty_snapshot, service_log_path,
        service_sibling_path_for, startup_diagnostics,
    };
    #[cfg(unix)]
    use super::{ensure_service_with_timeout, resolve_host_context};
    use std::path::Path;
    use vapor_client_protocol::{ClientEvent, ClientRevision, PROTOCOL_VERSION, ProtocolFrame};

    fn bootstrap() -> ClientHostConfig {
        ClientHostConfig {
            application_executable: Path::new("/app/bin/vapor-gui").to_path_buf(),
            workshop_app_id: None,
            workspace_root: None,
            active_composition_id: Some("default".to_owned()),
            fixture_mode: true,
            legacy_storage: vapor_client_protocol::LegacyStorageSeed::default(),
            state_directory: None,
        }
    }

    #[test]
    fn service_is_resolved_as_gui_sibling_on_each_target() {
        let linux = service_sibling_path_for(Path::new("/app/bin/vapor-gui"), false);
        assert_eq!(linux.file_name().unwrap(), "vapor-client-service");
        assert_eq!(linux.parent().unwrap(), Path::new("/app/bin"));
        let windows = service_sibling_path_for(Path::new("C:/app/bin/vapor-gui.exe"), true);
        assert_eq!(windows.file_name().unwrap(), "vapor-client-service.exe");
        assert_eq!(windows.parent().unwrap(), Path::new("C:/app/bin"));
    }

    #[cfg(unix)]
    #[test]
    fn dropping_a_client_sends_an_explicit_disconnect_frame() {
        use super::read_frame;
        use std::io::BufReader;

        let (client_stream, service_stream) =
            std::os::unix::net::UnixStream::pair().expect("create local socket pair");
        let (_, incoming) = std::sync::mpsc::channel();
        let client = Client::new(None, Box::new(client_stream), incoming);

        drop(client);

        let mut service_stream = BufReader::new(service_stream);
        assert_eq!(
            read_frame(&mut service_stream).expect("read disconnect frame"),
            Some(ProtocolFrame::Disconnect)
        );
        assert_eq!(
            read_frame(&mut service_stream).expect("read client close"),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn detached_service_process_is_reaped_after_exit() {
        use super::SpawnedService;
        use std::{
            process::Command,
            thread,
            time::{Duration, Instant},
        };

        let child = Command::new("sh")
            .args(["-c", "sleep 0.1"])
            .spawn()
            .expect("start child fixture");
        let pid = child.id();
        SpawnedService {
            child: Some(child),
            log_path: std::env::temp_dir().join("unused-service.log"),
            detached: false,
        }
        .detach()
        .expect("spawn child reaper");

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let state = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|stat| stat.rsplit_once(") ").map(|(_, rest)| rest.chars().next()))
                .flatten();
            match state {
                None => break,
                Some('Z') if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Some(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Some(state) => panic!("service child {pid} remained in state {state}"),
            }
        }
    }

    #[test]
    fn service_diagnostics_are_read_from_the_persistent_log_tail() {
        let path =
            std::env::temp_dir().join(format!("vapor-service-log-test-{}.log", std::process::id()));
        std::fs::write(
            &path,
            (0..30)
                .map(|line| format!("service line {line}\n"))
                .collect::<String>(),
        )
        .expect("write test service log");

        let lines = startup_diagnostics(&path);
        assert_eq!(lines.len(), 24);
        assert_eq!(lines[0], "service line 6");
        assert_eq!(lines[23], "service line 29");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn service_log_is_stored_with_other_app_state() {
        assert_eq!(
            service_log_path(Path::new("/app/.vapor/state")),
            Path::new("/app/.vapor/state/logs/client-service.log")
        );
    }

    #[cfg(unix)]
    #[test]
    fn shared_startup_timeout_reports_child_output_and_stops_the_child() {
        use std::{
            fs,
            os::unix::fs::PermissionsExt,
            time::{Duration, SystemTime},
        };

        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock follows epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "vapor-client-startup-timeout-{}-{unique}",
            std::process::id()
        ));
        let state = root.join("state");
        let app_executable = root.join("app/bin/vapor-gui");
        let service = root.join("fake-service");
        fs::create_dir_all(&state).expect("create test state directory");
        fs::write(
            &service,
            "#!/bin/sh\necho startup diagnostic >&2\nexec sleep 30\n",
        )
        .expect("write fake service");
        fs::set_permissions(&service, fs::Permissions::from_mode(0o700))
            .expect("make fake service executable");

        let mut bootstrap = bootstrap();
        bootstrap.fixture_mode = false;
        bootstrap.application_executable = app_executable;
        bootstrap.state_directory = Some(state);
        let context = resolve_host_context(&bootstrap).expect("resolve host context");
        let endpoint = super::local_socket::endpoint_name(
            context.app_root.as_deref(),
            &context.state_directory,
        );

        let error = ensure_service_with_timeout(
            &service,
            &bootstrap,
            &context,
            &endpoint,
            Duration::from_millis(250),
        )
        .expect_err("fake service never claims the local endpoint");

        match error {
            ClientError::ServiceStartupTimeout(lines) => {
                assert!(lines.iter().any(|line| line.contains("startup diagnostic")));
            }
            other => panic!("unexpected service startup error: {other:?}"),
        }
        fs::remove_dir_all(root).expect("remove test files");
    }

    #[test]
    fn shared_service_handshake_waits_for_initial_snapshot_after_ready() {
        let mut handshake = ServiceHandshake::new(true);
        assert!(
            handshake
                .accept(ProtocolFrame::Event(ClientEvent::ServiceReady {
                    protocol_version: PROTOCOL_VERSION,
                }))
                .unwrap()
                .is_none()
        );

        let mut snapshot = empty_snapshot();
        snapshot.revision = ClientRevision(5);
        let connected = handshake
            .accept(ProtocolFrame::Event(ClientEvent::Snapshot(Box::new(
                snapshot,
            ))))
            .unwrap()
            .expect("snapshot completes the handshake");
        assert_eq!(connected.revision, ClientRevision(5));
    }

    #[test]
    fn shared_service_handshake_accepts_snapshot_before_ready() {
        let mut handshake = ServiceHandshake::new(true);
        let mut snapshot = empty_snapshot();
        snapshot.revision = ClientRevision(7);
        assert!(
            handshake
                .accept(ProtocolFrame::Event(ClientEvent::Snapshot(Box::new(
                    snapshot
                ))))
                .unwrap()
                .is_none()
        );
        let connected = handshake
            .accept(ProtocolFrame::Event(ClientEvent::ServiceReady {
                protocol_version: PROTOCOL_VERSION,
            }))
            .unwrap()
            .expect("ready completes the handshake");
        assert_eq!(connected.revision, ClientRevision(7));
    }

    #[test]
    fn shared_service_handshake_buffers_live_events_until_the_snapshot_arrives() {
        let mut handshake = ServiceHandshake::new(true);
        let operation = ProtocolFrame::Event(ClientEvent::OperationChanged(
            vapor_client_protocol::OperationEvent {
                revision: ClientRevision(4),
                id: "operation-4".into(),
                composition_id: None,
                action: "authoring.list-registered-sources".into(),
                phase: vapor_client_protocol::OperationPhase::Resolving,
                status: vapor_client_protocol::OperationStatus::Running,
                progress: None,
                message: None,
            },
        ));

        assert!(handshake.accept(operation.clone()).unwrap().is_none());
        assert!(
            handshake
                .accept(ProtocolFrame::Event(ClientEvent::ServiceReady {
                    protocol_version: PROTOCOL_VERSION,
                }))
                .unwrap()
                .is_none()
        );
        let snapshot = handshake
            .accept(ProtocolFrame::Event(ClientEvent::Snapshot(Box::new(
                empty_snapshot(),
            ))))
            .unwrap()
            .expect("initial snapshot completes the handshake");

        assert_eq!(snapshot.revision, ClientRevision(0));
        assert_eq!(handshake.pending_events.len(), 1);
        assert_eq!(handshake.pending_events.front(), Some(&operation));
    }

    #[test]
    fn service_readiness_probe_does_not_wait_for_an_uninitialized_snapshot() {
        let mut handshake = ServiceHandshake::new(false);
        let snapshot = handshake
            .accept(ProtocolFrame::Event(ClientEvent::ServiceReady {
                protocol_version: PROTOCOL_VERSION,
            }))
            .unwrap()
            .expect("readiness probe completes without a session snapshot");
        assert_eq!(snapshot.revision, ClientRevision(0));
    }

    #[test]
    fn disconnected_shared_client_polling_reports_exit_without_reconnecting() {
        use std::{
            sync::mpsc,
            time::{Duration, SystemTime},
        };

        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock follows epoch")
            .as_nanos();
        let state_directory = std::env::temp_dir().join(format!(
            "vapor-client-disconnected-poll-{}-{unique}",
            std::process::id()
        ));
        let (sender, incoming) = mpsc::channel();
        drop(sender);
        let mut bootstrap = bootstrap();
        bootstrap.fixture_mode = false;
        bootstrap.state_directory = Some(state_directory.clone());
        let mut client = Client {
            child: None,
            writer: None,
            incoming,
            next_request: 1,
            request_prefix: "disconnected-test".into(),
            latest_snapshot: empty_snapshot(),
            shared_service: Some((
                std::env::temp_dir().join("missing-vapor-client-service"),
                bootstrap,
            )),
        };

        assert!(matches!(
            client.try_recv(),
            Err(ClientError::ServiceExited(None))
        ));
        assert!(matches!(
            client.recv_timeout(Duration::from_millis(1)),
            Err(ClientError::ServiceExited(None))
        ));
        let _ = std::fs::remove_dir_all(state_directory);
    }

    #[test]
    fn missing_sibling_is_reported_as_a_spawn_error() {
        let missing = std::env::temp_dir().join(format!(
            "vapor-client-service-missing-{}",
            std::process::id()
        ));
        assert!(matches!(
            Client::spawn(&missing, bootstrap()),
            Err(ClientError::Spawn(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn early_exit_and_malformed_output_are_bounded_startup_errors() {
        use std::{fs, os::unix::fs::PermissionsExt, time::SystemTime};

        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock follows epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "vapor-studio-client-process-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create temporary test directory");
        let exiting = root.join("exit-service");
        fs::write(&exiting, "#!/bin/sh\nexit 0\n").expect("write exiting child");
        fs::set_permissions(&exiting, fs::Permissions::from_mode(0o700))
            .expect("make child executable");
        let exited = Client::spawn(&exiting, bootstrap())
            .err()
            .expect("early exit fails");
        assert!(
            matches!(exited, ClientError::ServiceExited(_) | ClientError::Io(_)),
            "unexpected early-exit error: {exited:?}"
        );

        let malformed = root.join("malformed-service");
        fs::write(
            &malformed,
            "#!/bin/sh\nIFS= read -r initialization || exit 2\nprintf '{broken json}\\n'\n",
        )
        .expect("write malformed child");
        fs::set_permissions(&malformed, fs::Permissions::from_mode(0o700))
            .expect("make child executable");
        let error = Client::spawn(&malformed, bootstrap())
            .err()
            .expect("malformed service output fails startup");
        assert!(
            matches!(&error, ClientError::Protocol(message) if message.contains("malformed protocol data")),
            "got {error:?}"
        );
        fs::remove_dir_all(root).expect("clean temporary test directory");
    }
}
