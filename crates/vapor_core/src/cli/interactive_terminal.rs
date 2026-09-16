use std::env;
use std::ffi::OsString;
use std::io::{self, IsTerminal};
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::process::Command;

const EXTERNAL_TERMINAL_ENV: &str = "VAPOR_EXTERNAL_TERMINAL";

/// Re-run the current Vapor command in a real platform terminal when the
/// current process has no interactive stdin (for example a JetBrains run
/// console). Returns `true` when the delegated command already completed and
/// the caller should return without executing the operation again.
pub(super) fn delegate_if_needed(title: &str) -> Result<bool, String> {
    if io::stdin().is_terminal() {
        return Ok(false);
    }

    if env::var_os(EXTERNAL_TERMINAL_ENV).is_some() {
        return Err(
            "Vapor requires interactive input, but the delegated external terminal still has no interactive stdin"
                .to_owned(),
        );
    }

    let executable = env::current_exe()
        .map_err(|error| format!("failed to resolve the running Vapor executable: {error}"))?;
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();

    launch_external_terminal(title, &executable, &arguments)?;
    Ok(true)
}

#[cfg(target_os = "linux")]
fn launch_external_terminal(
    title: &str,
    executable: &Path,
    arguments: &[OsString],
) -> Result<(), String> {
    if let Some(konsole) = find_executable("konsole") {
        let status = Command::new(&konsole)
            .args(["--nofork", "--hold", "-p"])
            .arg(format!("tabtitle={title}"))
            .arg("-e")
            .arg(executable)
            .args(arguments)
            .env(EXTERNAL_TERMINAL_ENV, "1")
            .status()
            .map_err(|error| {
                format!(
                    "failed to launch external terminal `{}`: {error}",
                    konsole.display()
                )
            })?;

        return terminal_status(status);
    }

    if let Some(terminal) = find_executable("gnome-terminal") {
        let status = Command::new(&terminal)
            .arg("--wait")
            .arg(format!("--title={title}"))
            .arg("--")
            .arg(executable)
            .args(arguments)
            .env(EXTERNAL_TERMINAL_ENV, "1")
            .status()
            .map_err(|error| {
                format!(
                    "failed to launch external terminal `{}`: {error}",
                    terminal.display()
                )
            })?;

        return terminal_status(status);
    }

    if let Some(xterm) = find_executable("xterm") {
        let status = Command::new(&xterm)
            .args(["-hold", "-T", title, "-e"])
            .arg(executable)
            .args(arguments)
            .env(EXTERNAL_TERMINAL_ENV, "1")
            .status()
            .map_err(|error| {
                format!(
                    "failed to launch external terminal `{}`: {error}",
                    xterm.display()
                )
            })?;

        return terminal_status(status);
    }

    Err(
        "Vapor requires an interactive terminal, but no supported terminal emulator (Konsole, GNOME Terminal, or xterm) is available"
            .to_owned(),
    )
}

#[cfg(windows)]
fn launch_external_terminal(
    _title: &str,
    executable: &Path,
    arguments: &[OsString],
) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

    let status = Command::new(executable)
        .args(arguments)
        .env(EXTERNAL_TERMINAL_ENV, "1")
        .creation_flags(CREATE_NEW_CONSOLE)
        .status()
        .map_err(|error| format!("failed to launch Vapor in a new Windows console: {error}"))?;

    terminal_status(status)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn launch_external_terminal(
    _title: &str,
    _executable: &Path,
    _arguments: &[OsString],
) -> Result<(), String> {
    Err(format!(
        "Vapor external terminal handoff is not implemented for {}",
        env::consts::OS
    ))
}

fn terminal_status(status: std::process::ExitStatus) -> Result<(), String> {
    if status.success() {
        Ok(())
    } else {
        Err(format!("external Vapor terminal exited with {status}"))
    }
}

#[cfg(target_os = "linux")]
fn find_executable(name: &str) -> Option<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("/usr/bin").join(name),
        PathBuf::from("/bin").join(name),
    ];

    if let Some(path) = env::var_os("PATH") {
        candidates.extend(env::split_paths(&path).map(|root| root.join(name)));
    }

    candidates.into_iter().find(|candidate| candidate.is_file())
}
