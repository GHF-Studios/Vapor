use super::LocalInput;
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
};

const MAX_LSP_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_PENDING_LSP_MESSAGES: usize = 128;

struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self(Some(child))
    }

    fn child(&mut self) -> &mut Child {
        self.0.as_mut().expect("startup guard owns its child")
    }

    fn take(&mut self) -> Child {
        self.0.take().expect("startup guard owns its child")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub(super) struct Process {
    child: Child,
    messages: SyncSender<Value>,
    stopping: Arc<AtomicBool>,
}

impl Process {
    pub(super) fn send(&self, message: Value) -> Result<(), String> {
        self.messages
            .try_send(message)
            .map_err(|error| format!("Rust Analyzer message queue is unavailable: {error}"))
    }

    pub(super) fn stop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(super) fn start(
    workspace: &Path,
    client_id: u64,
    generation: u64,
    events: SyncSender<LocalInput>,
) -> Result<Process, String> {
    // The service is usually launched from Vapor's installed app root, which
    // has `App.vapor.toml` rather than a source `Workspace.vapor.toml`. Resolve
    // the pin from the workspace explicitly selected by the IDE instead of
    // inheriting the service process's working directory.
    let source = vapor_core::VaporWorkspace::discover_from(workspace).map_err(|error| {
        format!(
            "could not discover the selected source workspace at '{}': {error}",
            workspace.display()
        )
    })?;
    let toolchain = vapor_core::ManagedToolchain::for_workspace(&source).map_err(|error| {
        format!("could not prepare the selected workspace's Rust toolchain: {error}")
    })?;
    if !toolchain.rust_analyzer_path.is_file() {
        return Err(format!(
            "Vapor's pinned Rust Analyzer is unavailable at {}; repair or promote this installation's developer tools.",
            toolchain.rust_analyzer_path.display()
        ));
    }
    if !toolchain.is_installed() {
        return Err("Vapor's pinned Rust toolchain is not installed.".to_owned());
    }

    let mut command = toolchain
        .command(&toolchain.rust_analyzer_path)
        .map_err(|error| {
            format!("could not prepare Rust Analyzer's managed environment: {error}")
        })?;
    command
        .current_dir(workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_background_process(&mut command);
    let mut child = ChildGuard::new(command.spawn().map_err(|error| {
        format!(
            "could not start Vapor's pinned Rust Analyzer at {}: {error}",
            toolchain.rust_analyzer_path.display()
        )
    })?);
    let stdin = child
        .child()
        .stdin
        .take()
        .ok_or_else(|| "Rust Analyzer stdin was not available.".to_owned())?;
    let stdout = child
        .child()
        .stdout
        .take()
        .ok_or_else(|| "Rust Analyzer stdout was not available.".to_owned())?;
    let stderr = child
        .child()
        .stderr
        .take()
        .ok_or_else(|| "Rust Analyzer stderr was not available.".to_owned())?;
    let stderr_tail = Arc::new(Mutex::new(String::new()));
    let stderr_tail_worker = stderr_tail.clone();
    let stderr_worker = thread::Builder::new()
        .name(format!("vapor-rust-analyzer-stderr-{client_id}"))
        .spawn(move || drain_stderr(stderr, &stderr_tail_worker))
        .map_err(|error| format!("could not start Rust Analyzer diagnostics worker: {error}"))?;

    let (messages, receiver) = mpsc::sync_channel::<Value>(MAX_PENDING_LSP_MESSAGES);
    let writer_events = events.clone();
    thread::Builder::new()
        .name(format!("vapor-rust-analyzer-write-{client_id}"))
        .spawn(move || {
            let mut stdin = stdin;
            while let Ok(message) = receiver.recv() {
                if let Err(error) = write_message(&mut stdin, &message) {
                    let _ = writer_events.send(LocalInput::LanguageServerExited(
                        client_id,
                        generation,
                        Some(format!("Rust Analyzer input failed: {error}")),
                    ));
                    break;
                }
            }
        })
        .map_err(|error| format!("could not start Rust Analyzer input worker: {error}"))?;

    let stopping = Arc::new(AtomicBool::new(false));
    let output_stopping = stopping.clone();
    thread::Builder::new()
        .name(format!("vapor-rust-analyzer-read-{client_id}"))
        .spawn(move || {
            let mut error = read_messages(stdout, client_id, generation, &events).err();
            let _ = stderr_worker.join();
            if error.is_none() && !output_stopping.load(Ordering::Acquire) {
                error = Some("Rust Analyzer closed its output unexpectedly.".to_owned());
            }
            if let Ok(tail) = stderr_tail.lock()
                && !tail.is_empty()
            {
                error = Some(format!(
                    "{}\nRust Analyzer stderr:\n{tail}",
                    error.unwrap_or_else(|| "Rust Analyzer stopped.".to_owned())
                ));
            }
            let _ = events.send(LocalInput::LanguageServerExited(
                client_id, generation, error,
            ));
        })
        .map_err(|error| format!("could not start Rust Analyzer output worker: {error}"))?;

    Ok(Process {
        child: child.take(),
        messages,
        stopping,
    })
}

#[cfg(windows)]
fn configure_background_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}

#[cfg(not(windows))]
fn configure_background_process(_command: &mut Command) {}

fn drain_stderr(mut reader: impl Read, tail: &Mutex<String>) {
    const MAX_STDERR_BYTES: usize = 8192;
    let mut buffer = [0_u8; 1024];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        if let Ok(mut tail) = tail.lock() {
            tail.push_str(&String::from_utf8_lossy(&buffer[..read]));
            if tail.len() > MAX_STDERR_BYTES {
                let mut boundary = tail.len() - MAX_STDERR_BYTES;
                while !tail.is_char_boundary(boundary) {
                    boundary += 1;
                }
                tail.drain(..boundary);
            }
        }
    }
}

fn write_message(writer: &mut impl Write, message: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(message).map_err(std::io::Error::other)?;
    if body.len() > MAX_LSP_MESSAGE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "LSP message exceeds the 16 MiB limit",
        ));
    }
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

fn read_messages(
    reader: impl Read,
    client_id: u64,
    generation: u64,
    events: &SyncSender<LocalInput>,
) -> Result<(), String> {
    let mut reader = BufReader::new(reader);
    loop {
        let Some(length) = read_content_length(&mut reader)? else {
            return Ok(());
        };
        if length > MAX_LSP_MESSAGE_BYTES {
            return Err("Rust Analyzer sent an LSP message larger than 16 MiB.".to_owned());
        }
        let mut body = vec![0; length];
        reader
            .read_exact(&mut body)
            .map_err(|error| format!("could not read Rust Analyzer message: {error}"))?;
        let message = serde_json::from_slice(&body)
            .map_err(|error| format!("Rust Analyzer sent invalid JSON: {error}"))?;
        events
            .send(LocalInput::LanguageServerMessage(
                client_id, generation, message,
            ))
            .map_err(|_| {
                "Vapor Client service stopped while Rust Analyzer was running.".to_owned()
            })?;
    }
}

fn read_content_length(reader: &mut impl BufRead) -> Result<Option<usize>, String> {
    let mut content_length = None;
    let mut header_bytes = 0;
    loop {
        let mut line = Vec::new();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("could not read Rust Analyzer header: {error}"))?;
        if read == 0 {
            if header_bytes == 0 {
                return Ok(None);
            }
            return Err("Rust Analyzer closed during an LSP header.".to_owned());
        }
        header_bytes += read;
        if header_bytes > 8192 {
            return Err("Rust Analyzer sent an LSP header larger than 8 KiB.".to_owned());
        }
        if line == b"\r\n" || line == b"\n" {
            return content_length
                .map(Some)
                .ok_or_else(|| "Rust Analyzer omitted the LSP Content-Length header.".to_owned());
        }
        let line = std::str::from_utf8(&line)
            .map_err(|_| "Rust Analyzer sent a non-UTF8 LSP header.".to_owned())?;
        if let Some((name, value)) = line.trim_end().split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            if content_length.is_some() {
                return Err("Rust Analyzer sent duplicate LSP Content-Length headers.".to_owned());
            }
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| "Rust Analyzer sent an invalid LSP Content-Length.".to_owned())?,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_LSP_MESSAGE_BYTES, read_content_length, write_message};
    use serde_json::json;

    #[test]
    fn lsp_message_uses_content_length_framing() {
        let message = json!({"jsonrpc":"2.0","method":"initialized"});
        let body = serde_json::to_vec(&message).expect("serialize message");
        let mut expected = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        expected.extend(body);
        let mut actual = Vec::new();
        write_message(&mut actual, &message).expect("write LSP frame");
        assert_eq!(actual, expected);
    }

    #[test]
    fn lsp_header_rejects_duplicate_or_oversized_content_lengths() {
        let duplicate = b"Content-Length: 0\r\ncontent-length: 0\r\n\r\n";
        assert!(read_content_length(&mut duplicate.as_slice()).is_err());
        let excessive = format!("Content-Length: {}\r\n\r\n", MAX_LSP_MESSAGE_BYTES + 1);
        let length = read_content_length(&mut excessive.as_bytes())
            .expect("parse header")
            .expect("message length");
        assert!(length > MAX_LSP_MESSAGE_BYTES);
    }

    #[cfg(unix)]
    #[test]
    fn aborted_analyzer_startup_reaps_the_spawned_child() {
        use super::ChildGuard;
        use std::process::Command;

        let child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn child fixture");
        let pid = child.id();
        drop(ChildGuard::new(child));

        let still_running = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .expect("probe child process")
            .success();
        assert!(!still_running, "startup failure must reap the child");
    }
}
