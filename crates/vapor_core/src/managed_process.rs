//! Bounded stdout/stderr forwarding for managed child processes.

use crate::OperationOutputStream;
use std::{
    ffi::OsStr,
    io::{self, BufRead, BufReader, Read, Write},
    process::{Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

const MAX_OPERATION_OUTPUT_LINE_BYTES: usize = 8 * 1024;

/// Construct a child-process command that never opens a separate Windows console.
///
/// Use this for tools Vapor manages in the background. Commands that deliberately
/// open a user terminal should keep using `Command::new` and request that window.
pub fn background_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    configure_background_process(&mut command);
    command
}

/// Run a managed child while forwarding bounded stdout and stderr lines.
pub fn run(
    command: &mut Command,
    report_output: impl FnMut(OperationOutputStream, &str),
) -> Result<ExitStatus, String> {
    run_cancellable(command, || false, report_output)
}

/// Run a managed child while forwarding output and polling for cancellation.
pub fn run_cancellable(
    command: &mut Command,
    is_cancelled: impl FnMut() -> bool,
    report_output: impl FnMut(OperationOutputStream, &str),
) -> Result<ExitStatus, String> {
    run_cancellable_with_line_limit(command, is_cancelled, MAX_OPERATION_OUTPUT_LINE_BYTES, report_output)
}

/// Run a managed child to completion while allowing a bounded machine-readable line size.
pub fn run_with_line_limit(
    command: &mut Command,
    max_line_bytes: usize,
    report_output: impl FnMut(OperationOutputStream, &str),
) -> Result<ExitStatus, String> {
    run_cancellable_with_line_limit(command, || false, max_line_bytes, report_output)
}

/// Run a managed child with a caller-selected bounded line size.
///
/// Structured machine-readable tools can emit one JSON record larger than the
/// ordinary 8 KiB console line limit. This variant keeps that exceptional
/// allowance explicit at the process call site.
pub fn run_cancellable_with_line_limit(
    command: &mut Command,
    is_cancelled: impl FnMut() -> bool,
    max_line_bytes: usize,
    report_output: impl FnMut(OperationOutputStream, &str),
) -> Result<ExitStatus, String> {
    if max_line_bytes == 0 {
        return Err("managed child output line limit must be positive".to_owned());
    }
    run_cancellable_with_started_and_line_limit(
        command, is_cancelled, |_| Ok(()), max_line_bytes, report_output,
    )
}

/// Run a managed child and notify the owner as soon as its process identity exists.
/// If setup fails, terminate the complete child tree before returning.
pub fn run_cancellable_with_started(
    command: &mut Command,
    is_cancelled: impl FnMut() -> bool,
    on_started: impl FnOnce(u32) -> Result<(), String>,
    report_output: impl FnMut(OperationOutputStream, &str),
) -> Result<ExitStatus, String> {
    run_cancellable_with_started_and_line_limit(
        command,
        is_cancelled,
        on_started,
        MAX_OPERATION_OUTPUT_LINE_BYTES,
        report_output,
    )
}

fn run_cancellable_with_started_and_line_limit(
    command: &mut Command,
    mut is_cancelled: impl FnMut() -> bool,
    on_started: impl FnOnce(u32) -> Result<(), String>,
    max_line_bytes: usize,
    mut report_output: impl FnMut(OperationOutputStream, &str),
) -> Result<ExitStatus, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    configure_background_process(command);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to start managed child process: {error}"))?;
    if let Err(error) = on_started(child.id()) {
        let _ = terminate_child_tree(&mut child);
        let _ = child.wait();
        return Err(error);
    }
    let stdout = child.stdout.take().ok_or_else(|| {
        let _ = child.kill();
        let _ = child.wait();
        "managed child stdout pipe was not available".to_owned()
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        let _ = child.kill();
        let _ = child.wait();
        "managed child stderr pipe was not available".to_owned()
    })?;
    let (sender, receiver) = mpsc::sync_channel(128);
    let stdout_sender = sender.clone();
    let stdout_reader = thread::spawn(move || {
        forward_output(stdout, OperationOutputStream::Stdout, stdout_sender, max_line_bytes)
    });
    let stderr_reader = thread::spawn(move || {
        forward_output(stderr, OperationOutputStream::Stderr, sender, max_line_bytes)
    });
    let mut cancellation_sent = false;
    loop {
        if !cancellation_sent && is_cancelled() {
            cancellation_sent = true;
            if let Err(error) = terminate_child_tree(&mut child)
                && child.try_wait().ok().flatten().is_none()
            {
                return Err(format!("failed to stop managed child process: {error}"));
            }
        }
        match receiver.recv_timeout(Duration::from_millis(40)) {
            Ok((stream, text)) => report_output(stream, &text),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancellation_sent {
                    // The pipe readers finish after the child exits; continue draining their
                    // bounded output so neither pipe can block the child during shutdown.
                    match child.try_wait() {
                        Ok(Some(_)) => {}
                        Ok(None) => continue,
                        Err(error) => {
                            return Err(format!(
                                "failed waiting for managed child process: {error}"
                            ));
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child
        .wait()
        .map_err(|error| format!("failed waiting for managed child process: {error}"))?;
    stdout_reader
        .join()
        .map_err(|_| "managed child stdout reader panicked".to_owned())??;
    stderr_reader
        .join()
        .map_err(|_| "managed child stderr reader panicked".to_owned())??;
    if cancellation_sent {
        Err("operation cancelled".to_owned())
    } else {
        Ok(status)
    }
}

fn terminate_child_tree(child: &mut std::process::Child) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        let process_group = format!("-{}", child.id());
        // The child starts in a fresh process group so Cargo and its compiler
        // descendants are stopped together, rather than leaving builds behind.
        let result = Command::new("kill")
            .args(["-KILL", "--", &process_group])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if matches!(result, Ok(status) if status.success()) {
            return Ok(());
        }
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        Err(result.err().unwrap_or_else(|| {
            std::io::Error::other("could not terminate managed child process group")
        }))
    }
    #[cfg(windows)]
    {
        let mut command = background_command("taskkill");
        command
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = command.status();
        if matches!(status, Ok(status) if status.success()) {
            return Ok(());
        }
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        child.kill()
    }
    #[cfg(not(any(unix, windows)))]
    {
        child.kill()
    }
}

#[cfg(windows)]
fn configure_background_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}

#[cfg(not(windows))]
fn configure_background_process(_command: &mut Command) {}

fn forward_output(
    reader: impl Read,
    stream: OperationOutputStream,
    sender: mpsc::SyncSender<(OperationOutputStream, String)>,
    max_line_bytes: usize,
) -> Result<(), String> {
    let mut reader = BufReader::new(reader);
    let mut bytes = Vec::new();
    let mut carry = Vec::new();
    loop {
        let Some(_continued) = read_bounded_line(&mut reader, &mut bytes, &mut carry, max_line_bytes)? else {
            return Ok(());
        };
        let text = String::from_utf8_lossy(&bytes);
        sender
            .send((stream, text.into_owned()))
            .map_err(|_| "Vapor Client stopped receiving managed child output".to_owned())?;
    }
}

fn read_bounded_line(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
    carry: &mut Vec<u8>,
    max_line_bytes: usize,
) -> Result<Option<bool>, String> {
    line.clear();
    line.append(carry);
    loop {
        let available = reader
            .fill_buf()
            .map_err(|error| format!("failed to read managed child output: {error}"))?;
        if available.is_empty() {
            return Ok((!line.is_empty()).then_some(false));
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let content_length = newline.unwrap_or(available.len());
        let remaining = max_line_bytes.saturating_sub(line.len());
        if newline.is_some() && content_length <= remaining {
            line.extend_from_slice(&available[..content_length]);
            reader.consume(content_length + 1);
            if matches!(line.last(), Some(b'\r')) {
                line.pop();
            }
            return Ok(Some(false));
        }

        let candidate_length = content_length.min(remaining);
        if candidate_length == 0 {
            preserve_utf8_tail(line, carry);
            return Ok(Some(true));
        }
        let mut retained_length = candidate_length;
        if let Err(error) = std::str::from_utf8(&available[..candidate_length])
            && error.error_len().is_none()
        {
            retained_length = error.valid_up_to();
        }
        if retained_length == 0 {
            if line.is_empty() {
                retained_length = candidate_length;
            } else {
                preserve_utf8_tail(line, carry);
                return Ok(Some(true));
            }
        }
        line.extend_from_slice(&available[..retained_length]);
        reader.consume(retained_length);
        if retained_length < content_length || line.len() >= max_line_bytes {
            preserve_utf8_tail(line, carry);
            return Ok(Some(true));
        }
    }
}

fn preserve_utf8_tail(line: &mut Vec<u8>, carry: &mut Vec<u8>) {
    if let Err(error) = std::str::from_utf8(line)
        && error.error_len().is_none()
    {
        *carry = line.split_off(error.valid_up_to());
    }
}

/// Preserve child output when a managed provider is called synchronously.
pub fn write_to_terminal(stream: OperationOutputStream, text: &str) {
    match stream {
        OperationOutputStream::Stdout => {
            let _ = writeln!(io::stdout().lock(), "{text}");
        }
        OperationOutputStream::Stderr => {
            let _ = writeln!(io::stderr().lock(), "{text}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_OPERATION_OUTPUT_LINE_BYTES, forward_output, read_bounded_line};
    use crate::OperationOutputStream;
    use std::{
        io::{BufReader, Cursor},
        process::Command,
    };

    #[test]
    fn long_utf8_output_is_chunked_without_data_loss() {
        let source = format!(
            "{}🌌tail\n",
            "x".repeat(MAX_OPERATION_OUTPUT_LINE_BYTES * 2)
        );
        let (sender, receiver) = std::sync::mpsc::sync_channel(4);
        forward_output(
            Cursor::new(source.as_bytes().to_vec()),
            OperationOutputStream::Stdout,
            sender,
            MAX_OPERATION_OUTPUT_LINE_BYTES,
        )
        .unwrap();
        let chunks = receiver
            .into_iter()
            .map(|(_, text)| text)
            .collect::<Vec<_>>();
        assert!(chunks.len() >= 3);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.len() <= MAX_OPERATION_OUTPUT_LINE_BYTES)
        );
        assert_eq!(chunks.concat(), source.trim_end_matches('\n'));
    }

    #[test]
    fn oversized_output_is_drained_before_the_next_line() {
        let source = format!(
            "{}tail\nnext line\n",
            "x".repeat(MAX_OPERATION_OUTPUT_LINE_BYTES * 2)
        );
        let mut reader = BufReader::new(Cursor::new(source));
        let mut line = Vec::new();
        let mut carry = Vec::new();

        let mut continued = true;
        let mut chunks = Vec::new();
        while continued {
            continued = read_bounded_line(&mut reader, &mut line, &mut carry, MAX_OPERATION_OUTPUT_LINE_BYTES)
                .unwrap()
                .unwrap();
            assert!(line.len() <= MAX_OPERATION_OUTPUT_LINE_BYTES);
            chunks.push(line.clone());
        }
        assert_eq!(
            String::from_utf8_lossy(&chunks.concat()),
            format!("{}tail", "x".repeat(MAX_OPERATION_OUTPUT_LINE_BYTES * 2))
        );
        assert!(
            !read_bounded_line(&mut reader, &mut line, &mut carry, MAX_OPERATION_OUTPUT_LINE_BYTES)
                .unwrap()
                .unwrap()
        );
        assert_eq!(line, b"next line");
        assert!(
            read_bounded_line(&mut reader, &mut line, &mut carry, MAX_OPERATION_OUTPUT_LINE_BYTES)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn child_output_keeps_its_original_stream() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(4);
        forward_output(
            Cursor::new(b"stdout line\n".to_vec()),
            OperationOutputStream::Stdout,
            sender,
            MAX_OPERATION_OUTPUT_LINE_BYTES,
        )
        .unwrap();
        let event = receiver.recv().unwrap();
        assert_eq!(event.0, OperationOutputStream::Stdout);
        assert_eq!(event.1, "stdout line");
    }

    #[test]
    fn child_process_output_fixture() {
        let Some(mode) = std::env::var_os("VAPOR_PROCESS_OUTPUT_TEST_CHILD") else {
            return;
        };
        if std::env::var_os("VAPOR_PROCESS_OUTPUT_LONG_LINE").is_some() {
            println!("{}", "j".repeat(32 * 1024));
            return;
        }
        for index in 0..128 {
            println!("stdout marker {index}");
            eprintln!("stderr marker {index}");
        }
        if mode == "failure" {
            panic!("child failure marker");
        }
    }

    #[test]
    fn cancellation_child_fixture() {
        if std::env::var_os("VAPOR_PROCESS_OUTPUT_CANCEL_GRANDCHILD").is_some() {
            std::thread::sleep(std::time::Duration::from_secs(30));
        } else if std::env::var_os("VAPOR_PROCESS_OUTPUT_CANCEL_CHILD").is_some() {
            let executable = std::env::current_exe().expect("test executable path");
            let mut grandchild = Command::new(executable)
                .arg("--exact")
                .arg("managed_process::tests::cancellation_child_fixture")
                .arg("--nocapture")
                .env("VAPOR_PROCESS_OUTPUT_CANCEL_GRANDCHILD", "1")
                .spawn()
                .expect("spawn the descendant fixture");
            let _ = grandchild.wait();
        }
    }

    #[test]
    fn cancellation_stops_a_managed_child_process() {
        let executable = std::env::current_exe().expect("test executable path");
        let mut command = Command::new(executable);
        command
            .arg("--exact")
            .arg("managed_process::tests::cancellation_child_fixture")
            .arg("--nocapture")
            .env("VAPOR_PROCESS_OUTPUT_CANCEL_CHILD", "1");
        let started = std::time::Instant::now();
        let result = super::run_cancellable(
            &mut command,
            || started.elapsed() >= std::time::Duration::from_millis(100),
            |_, _| {},
        );
        assert_eq!(result.unwrap_err(), "operation cancelled");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "cancellation should stop descendants that inherited the output pipes"
        );
    }

    #[test]
    fn caller_selected_limit_preserves_large_machine_readable_records() {
        let executable = std::env::current_exe().expect("test executable path");
        let mut command = Command::new(&executable);
        command
            .arg("--exact")
            .arg("managed_process::tests::child_process_output_fixture")
            .arg("--nocapture")
            .env("VAPOR_PROCESS_OUTPUT_TEST_CHILD", "success")
            .env("VAPOR_PROCESS_OUTPUT_LONG_LINE", "1");
        let mut output = Vec::new();
        let status = super::run_cancellable_with_line_limit(
            &mut command,
            || false,
            64 * 1024,
            |stream, text| output.push((stream, text.to_owned())),
        )
        .expect("spawn the child test process");

        assert!(status.success());
        // The test harness emits its own progress lines even with `--exact`;
        // assert on the machine-readable fixture record rather than assuming
        // the child's entire stdout contains only that record.
        let stdout = output
            .iter()
            .filter(|(stream, _)| *stream == OperationOutputStream::Stdout)
            .map(|(_, text)| text.as_str())
            .collect::<String>();
        let record = "j".repeat(32 * 1024);
        assert_eq!(stdout.matches(&record).count(), 1);
    }

    #[test]
    fn child_process_output_drains_both_bounded_pipes_and_preserves_failure_text() {
        let executable = std::env::current_exe().expect("test executable path");
        let fixture = "managed_process::tests::child_process_output_fixture";

        for (mode, should_succeed) in [("success", true), ("failure", false)] {
            let mut command = Command::new(&executable);
            command
                .arg("--exact")
                .arg(fixture)
                .arg("--nocapture")
                .env("VAPOR_PROCESS_OUTPUT_TEST_CHILD", mode);
            let mut output = Vec::new();
            let status = super::run(&mut command, |stream, text| {
                output.push((stream, text.to_owned()));
            })
            .expect("spawn the child test process");

            assert_eq!(status.success(), should_succeed);
            assert_eq!(
                output
                    .iter()
                    .filter(|(stream, text)| {
                        *stream == OperationOutputStream::Stdout
                            && text.starts_with("stdout marker ")
                    })
                    .count(),
                128
            );
            assert_eq!(
                output
                    .iter()
                    .filter(|(stream, text)| {
                        *stream == OperationOutputStream::Stderr
                            && text.starts_with("stderr marker ")
                    })
                    .count(),
                128
            );
            if !should_succeed {
                assert!(output.iter().any(|(stream, text)| {
                    *stream == OperationOutputStream::Stderr
                        && text.contains("child failure marker")
                }));
            }
        }
    }
}
