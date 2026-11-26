use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

const CARGO_LINUX_TARGET: &str = "x86_64-unknown-linux-gnu";
const ZIG_LINUX_TARGET: &str = "x86_64-linux-gnu";

fn main() -> ExitCode {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();

    if is_cc_family_probe(&arguments) {
        println!("#pragma message \"gcc\"");
        return ExitCode::SUCCESS;
    }

    let long_app_root = match application_root() {
        Ok(path) => path,
        Err(message) => {
            eprintln!("vapor-linux-cc: {message}");
            return ExitCode::FAILURE;
        }
    };
    #[cfg(windows)]
    let app_root = match windows_short_path(&long_app_root) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("vapor-linux-cc: {message}");
            return ExitCode::FAILURE;
        }
    };
    #[cfg(not(windows))]
    let app_root = long_app_root.clone();
    let zig = app_root
        .join("tools")
        .join("zig")
        .join(format!("zig{}", env::consts::EXE_SUFFIX));

    let arguments = match normalize_windows_linker_paths(arguments, &long_app_root, &app_root) {
        Ok(arguments) => arguments,
        Err(message) => {
            eprintln!("vapor-linux-cc: {message}");
            return ExitCode::FAILURE;
        }
    };

    let mut command = Command::new(&zig);
    command.args(["cc", "-target", ZIG_LINUX_TARGET]);
    command.env(
        "ZIG_LIB_DIR",
        app_root.join("tools").join("zig").join("lib"),
    );
    command.args(arguments.iter().filter(|argument| {
        let argument = argument.to_string_lossy();
        !argument.starts_with(&format!("--target={CARGO_LINUX_TARGET}"))
            && argument != "-march=x86-64"
    }));

    match command.status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(1, 255) as u8),
        Err(error) => {
            eprintln!("vapor-linux-cc: failed to run {}: {error}", zig.display());
            ExitCode::FAILURE
        }
    }
}

fn normalize_windows_linker_paths(
    arguments: Vec<OsString>,
    long_app_root: &Path,
    short_app_root: &Path,
) -> Result<Vec<OsString>, String> {
    #[cfg(not(windows))]
    {
        let _ = (long_app_root, short_app_root);
        Ok(arguments)
    }

    #[cfg(windows)]
    {
        let long = long_app_root.to_string_lossy();
        let short = short_app_root.to_string_lossy();
        let mut normalized = Vec::with_capacity(arguments.len());
        for argument in arguments {
            let text = argument.to_string_lossy();
            if let Some(response_path) = response_file_path(&text, long_app_root) {
                if response_path.is_file() {
                    let contents = std::fs::read_to_string(&response_path).map_err(|error| {
                        format!(
                            "could not read Cargo linker response file '{}': {error}",
                            response_path.display()
                        )
                    })?;
                    let normalized_contents = replace_app_root_paths(&contents, &long, &short)
                        .replace(&escape_response_path(&long), &escape_response_path(&short));
                    if normalized_contents != contents {
                        std::fs::write(&response_path, normalized_contents).map_err(|error| {
                            format!(
                                "could not normalize Cargo linker response file '{}': {error}",
                                response_path.display()
                            )
                        })?;
                    }
                }
            }
            let value = replace_app_root_paths(&text, &long, &short);
            normalized.push(OsString::from(value));
        }
        Ok(normalized)
    }
}

#[cfg(windows)]
fn response_file_path(argument: &str, app_root: &Path) -> Option<PathBuf> {
    let response = argument.strip_prefix('@')?.trim_matches('"');
    let long_root = app_root.to_string_lossy();
    let relative = response.strip_prefix(long_root.as_ref()).or_else(|| {
        let extended_root = format!(r"\\?\{long_root}");
        response.strip_prefix(&extended_root)
    })?;
    let relative = relative.trim_start_matches(|character| character == '\\' || character == '/');
    let path = app_root.join(relative);
    path.file_name()
        .is_some_and(|name| name == "linker-arguments")
        .then_some(path)
}

#[cfg(windows)]
fn replace_app_root_paths(text: &str, long: &str, short: &str) -> String {
    let extended_root = format!(r"\\?\{long}");
    let normalized = text.replace(&extended_root, short).replace(long, short);
    normalized.replace(&long.replace('\\', "/"), &short.replace('\\', "/"))
}

#[cfg(windows)]
fn escape_response_path(path: &str) -> String {
    path.replace('\\', "\\\\").replace(' ', "\\ ")
}

fn is_cc_family_probe(arguments: &[OsString]) -> bool {
    arguments.iter().any(|argument| argument == "-E")
        && arguments.iter().any(|argument| {
            argument
                .to_string_lossy()
                .contains("detect_compiler_family.c")
        })
}

fn application_root() -> Result<PathBuf, String> {
    let executable = env::current_exe()
        .map_err(|error| format!("cannot resolve this executable path: {error}"))?;
    let target_bin = executable
        .parent()
        .ok_or_else(|| "executable path has no parent directory".to_owned())?;
    let app_bin = target_bin
        .parent()
        .ok_or_else(|| "executable path is not inside a target bin directory".to_owned())?;
    let root = app_bin
        .parent()
        .ok_or_else(|| "target bin directory is not inside a Vapor app root".to_owned())?;
    let zig = root
        .join("tools")
        .join("zig")
        .join(format!("zig{}", env::consts::EXE_SUFFIX));
    if Path::new(&zig).is_file() {
        Ok(root.to_path_buf())
    } else {
        Err(format!(
            "Vapor's Zig compiler is missing at {}",
            zig.display()
        ))
    }
}

#[cfg(windows)]
fn windows_short_path(path: &Path) -> Result<PathBuf, String> {
    let output = Command::new("cmd.exe")
        .args(["/d", "/c", "for %I in (%VAPOR_LINK_PATHS%) do @echo %~sI"])
        .env("VAPOR_LINK_PATHS", format!("\"{}\"", path.display()))
        .output()
        .map_err(|error| format!("cannot resolve a short app path: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cmd.exe could not resolve a short app path (exit {})",
            output.status
        ));
    }
    let short_path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if short_path.is_empty() || short_path.contains(' ') {
        return Err(format!(
            "Windows returned an unusable short app path for '{}'",
            path.display()
        ));
    }
    Ok(PathBuf::from(short_path))
}
