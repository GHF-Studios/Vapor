//! Bounded stdout/stderr forwarding for managed child processes.

use crate::OperationOutputStream;
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    process::{Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

const MAX_OPERATION_OUTPUT_LINE_BYTES: usize = 8 * 1024;

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
    run_cancellable_with_started(command, is_cancelled, |_| Ok(()), report_output)
}

/// Run a managed child and notify the owner as soon as its process identity exists.
/// If setup fails, terminate the complete child tree before returning.
pub fn run_cancellable_with_started(
    command: &mut Command,
    mut is_cancelled: impl FnMut() -> bool,
    on_started: impl FnOnce(u32) -> Result<(), String>,
    mut report_output: impl FnMut(OperationOutputStream, &str),
) -> Result<ExitStatus, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
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
    let stdout_reader =
        thread::spawn(move || forward_output(stdout, OperationOutputStream::Stdout, stdout_sender));
    let stderr_reader =
        thread::spawn(move || forward_output(stderr, OperationOutputStream::Stderr, sender));
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
        let status = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
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

fn forward_output(
    reader: impl Read,
    stream: OperationOutputStream,
    sender: mpsc::SyncSender<(OperationOutputStream, String)>,
) -> Result<(), String> {
    let mut reader = BufReader::new(reader);
    let mut bytes = Vec::new();
    loop {
        let Some(truncated) = read_bounded_line(&mut reader, &mut bytes)? else {
            return Ok(());
        };
        let text = String::from_utf8_lossy(&bytes);
        let text = format_output_line(&text, truncated);
        sender
            .send((stream, text))
            .map_err(|_| "Vapor Client stopped receiving managed child output".to_owned())?;
    }
}

fn read_bounded_line(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
) -> Result<Option<bool>, String> {
    line.clear();
    let mut truncated = false;
    loop {
        let available = reader
            .fill_buf()
            .map_err(|error| format!("failed to read managed child output: {error}"))?;
        if available.is_empty() {
            return Ok((!line.is_empty()).then_some(truncated));
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let content_length = newline.unwrap_or(available.len());
        let retained_length = content_length.min(MAX_OPERATION_OUTPUT_LINE_BYTES - line.len());
        line.extend_from_slice(&available[..retained_length]);
        truncated |= retained_length < content_length;

        let consumed = newline.map_or(available.len(), |index| index + 1);
        let completed = newline.is_some();
        reader.consume(consumed);
        if completed {
            if matches!(line.last(), Some(b'\r')) {
                line.pop();
            }
            return Ok(Some(truncated));
        }
    }
}

fn format_output_line(line: &str, truncated: bool) -> String {
    if !truncated && line.len() <= MAX_OPERATION_OUTPUT_LINE_BYTES {
        return line.to_owned();
    }
    let suffix = " … [truncated]";
    let mut end = MAX_OPERATION_OUTPUT_LINE_BYTES.saturating_sub(suffix.len());
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &line[..end])
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
    use super::{
        MAX_OPERATION_OUTPUT_LINE_BYTES, format_output_line, forward_output, read_bounded_line,
    };
    use crate::OperationOutputStream;
    use std::{
        io::{BufReader, Cursor},
        process::Command,
    };

    #[test]
    fn output_lines_are_bounded_without_splitting_utf8() {
        let line = format!("{}🌌", "x".repeat(MAX_OPERATION_OUTPUT_LINE_BYTES));
        let bounded = format_output_line(&line, true);
        assert!(bounded.len() <= MAX_OPERATION_OUTPUT_LINE_BYTES);
        assert!(bounded.is_char_boundary(bounded.len()));
        assert!(bounded.ends_with("[truncated]"));
    }

    #[test]
    fn oversized_output_is_drained_before_the_next_line() {
        let source = format!(
            "{}tail\nnext line\n",
            "x".repeat(MAX_OPERATION_OUTPUT_LINE_BYTES * 2)
        );
        let mut reader = BufReader::new(Cursor::new(source));
        let mut line = Vec::new();

        assert!(read_bounded_line(&mut reader, &mut line).unwrap().unwrap());
        assert!(line.len() <= MAX_OPERATION_OUTPUT_LINE_BYTES);
        assert_eq!(
            String::from_utf8_lossy(&line),
            "x".repeat(MAX_OPERATION_OUTPUT_LINE_BYTES)
        );
        assert!(!read_bounded_line(&mut reader, &mut line).unwrap().unwrap());
        assert_eq!(line, b"next line");
        assert!(read_bounded_line(&mut reader, &mut line).unwrap().is_none());
    }

    #[test]
    fn child_output_keeps_its_original_stream() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(4);
        forward_output(
            Cursor::new(b"stdout line\n".to_vec()),
            OperationOutputStream::Stdout,
            sender,
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
