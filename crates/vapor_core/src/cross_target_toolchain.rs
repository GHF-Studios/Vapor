//! Vapor-home native tools required to build declared foreign targets.

use crate::{ManagedToolError, VaporInstallation, managed_tool::verify_sha256_file};
use reqwest::blocking::Client;
use std::{
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

pub const WINDOWS_GNU_LINKER_VERSION: &str = "20260616";
pub const LINUX_GNU_LINKER_VERSION: &str = "0.16.0";

const ZIG_LINUX_ARCHIVE_URL: &str =
    "https://ziglang.org/download/0.16.0/zig-x86_64-linux-0.16.0.tar.xz";
const ZIG_LINUX_ARCHIVE_SHA256: &str =
    "70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00";
const ZIG_WINDOWS_ARCHIVE_URL: &str =
    "https://ziglang.org/download/0.16.0/zig-x86_64-windows-0.16.0.zip";
const ZIG_WINDOWS_ARCHIVE_SHA256: &str =
    "68659eb5f1e4eb1437a722f1dd889c5a322c9954607f5edcf337bc3684a75a7e";

const LINUX_ARCHIVE_URL: &str = "https://github.com/mstorsjo/llvm-mingw/releases/download/20260616/llvm-mingw-20260616-ucrt-ubuntu-22.04-x86_64.tar.xz";
const LINUX_ARCHIVE_SHA256: &str =
    "534b92e067b22a6b4441f48ae9240a3341b17825d04d577eab0cf85c44b4deda";
const WINDOWS_ARCHIVE_URL: &str = "https://github.com/mstorsjo/llvm-mingw/releases/download/20260616/llvm-mingw-20260616-ucrt-x86_64.zip";
const WINDOWS_ARCHIVE_SHA256: &str =
    "b9b68a4d276e16fa25802aaba458e4638f64b3884c290aaccdc2d87083b6ca35";
const MAX_TOOLCHAIN_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const TOOLCHAIN_MARKER: &str = "vapor-toolchain.toml";

/// Whether the selected installation has the pinned Zig compiler and Linux
/// cross-target wrapper required for `x86_64-unknown-linux-gnu` builds.
pub fn linux_gnu_linker_installed(installation: &VaporInstallation) -> bool {
    let root = installation.user_data_root().join("tools/zig");
    let marker = fs::read_to_string(root.join(TOOLCHAIN_MARKER));
    marker.is_ok_and(|marker| {
        marker.lines().any(|line| line.trim() == "schema = 1")
            && marker
                .lines()
                .any(|line| line.trim() == format!("version = \"{LINUX_GNU_LINKER_VERSION}\""))
            && zig_executable(&root).is_file()
            && linux_wrapper_installed(installation)
    })
}

/// Install pinned Zig plus a wrapper that normalizes rustc/cc-rs target flags
/// to the target spelling accepted by Zig 0.16.
pub fn install_linux_gnu_linker(
    installation: &VaporInstallation,
) -> Result<bool, CrossTargetToolchainError> {
    if linux_gnu_linker_installed(installation) {
        return Ok(false);
    }
    if !cfg!(target_arch = "x86_64") || !cfg!(any(target_os = "linux", target_os = "windows")) {
        return Err(CrossTargetToolchainError::UnsupportedHost);
    }

    let (url, checksum, archive_name) = if cfg!(target_os = "linux") {
        (
            ZIG_LINUX_ARCHIVE_URL,
            ZIG_LINUX_ARCHIVE_SHA256,
            "zig-x86_64-linux-0.16.0.tar.xz",
        )
    } else {
        (
            ZIG_WINDOWS_ARCHIVE_URL,
            ZIG_WINDOWS_ARCHIVE_SHA256,
            "zig-x86_64-windows-0.16.0.zip",
        )
    };
    let vapor_home = installation.user_data_root();
    let tools_root = vapor_home.join("tools");
    let downloads = vapor_home.join("cache/downloads/toolchains");
    fs::create_dir_all(&tools_root).map_err(|error| CrossTargetToolchainError::Io {
        path: tools_root.clone(),
        message: error.to_string(),
    })?;
    fs::create_dir_all(&downloads).map_err(|error| CrossTargetToolchainError::Io {
        path: downloads.clone(),
        message: error.to_string(),
    })?;
    let archive = downloads.join(archive_name);
    let temporary = downloads.join(format!(".zig-{}.download", std::process::id()));
    let staging = tools_root.join(format!(".zig-{}.extracting", std::process::id()));
    let final_root = tools_root.join("zig");
    let result = (|| {
        let mut response = Client::builder()
            .user_agent("Vapor managed cross-target toolchain")
            .build()
            .map_err(|error| CrossTargetToolchainError::Download(error.to_string()))?
            .get(url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|error| CrossTargetToolchainError::Download(error.to_string()))?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_TOOLCHAIN_ARCHIVE_BYTES)
        {
            return Err(CrossTargetToolchainError::ArchiveTooLarge(
                MAX_TOOLCHAIN_ARCHIVE_BYTES,
            ));
        }
        let mut file =
            fs::File::create(&temporary).map_err(|error| CrossTargetToolchainError::Io {
                path: temporary.clone(),
                message: error.to_string(),
            })?;
        let bytes = io::copy(
            &mut response.by_ref().take(MAX_TOOLCHAIN_ARCHIVE_BYTES + 1),
            &mut file,
        )
        .map_err(|error| CrossTargetToolchainError::Download(error.to_string()))?;
        if bytes > MAX_TOOLCHAIN_ARCHIVE_BYTES {
            return Err(CrossTargetToolchainError::ArchiveTooLarge(
                MAX_TOOLCHAIN_ARCHIVE_BYTES,
            ));
        }
        file.sync_all()
            .map_err(|error| CrossTargetToolchainError::Io {
                path: temporary.clone(),
                message: error.to_string(),
            })?;
        verify_sha256_file(&temporary, checksum, url)
            .map_err(CrossTargetToolchainError::Integrity)?;
        fs::rename(&temporary, &archive).map_err(|error| CrossTargetToolchainError::Io {
            path: archive.clone(),
            message: error.to_string(),
        })?;
        fs::create_dir(&staging).map_err(|error| CrossTargetToolchainError::Io {
            path: staging.clone(),
            message: error.to_string(),
        })?;
        extract_archive(&archive, &staging)?;
        flatten_single_root(&staging)?;
        if !zig_executable(&staging).is_file() {
            return Err(CrossTargetToolchainError::IncompleteArchive {
                tool: "Zig",
                path: staging.clone(),
            });
        }
        write_zig_toolchain_marker(&staging, checksum)?;
        write_linux_wrapper(installation)?;
        promote_staged_bundle(&staging, &final_root, &tools_root)?;
        let _ = fs::remove_file(&archive);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        let _ = fs::remove_dir_all(&staging);
    }
    result.map(|()| true)
}

fn zig_executable(root: &Path) -> PathBuf {
    root.join(format!("zig{}", env::consts::EXE_SUFFIX))
}

fn linux_wrapper_installed(installation: &VaporInstallation) -> bool {
    let bin = installation.user_data_root().join("tools/cross/bin");
    if cfg!(windows) {
        bin.join("x86_64-unknown-linux-gnu-zig-cc.cmd").is_file()
            && bin.join("zig-cc.ps1").is_file()
            && bin.join("zig-ar.cmd").is_file()
    } else {
        bin.join("x86_64-unknown-linux-gnu-zig-cc").is_file() && bin.join("zig-ar").is_file()
    }
}

fn write_linux_wrapper(installation: &VaporInstallation) -> Result<(), CrossTargetToolchainError> {
    let bin = installation.user_data_root().join("tools/cross/bin");
    fs::create_dir_all(&bin).map_err(|error| CrossTargetToolchainError::Io {
        path: bin.clone(),
        message: error.to_string(),
    })?;
    #[cfg(windows)]
    {
        let command = bin.join("x86_64-unknown-linux-gnu-zig-cc.cmd");
        let script = bin.join("zig-cc.ps1");
        fs::write(
            &command,
            "@echo off\r\npowershell.exe -NoProfile -ExecutionPolicy Bypass -File \"%~dp0zig-cc.ps1\" %*\r\nexit /b %ERRORLEVEL%\r\n",
        )
        .map_err(|error| CrossTargetToolchainError::Io {
            path: command,
            message: error.to_string(),
        })?;
        fs::write(&script, WINDOWS_ZIG_CC_WRAPPER).map_err(|error| {
            CrossTargetToolchainError::Io {
                path: script,
                message: error.to_string(),
            }
        })?;
        let archiver = bin.join("zig-ar.cmd");
        fs::write(
            &archiver,
            "@echo off\r\n\"%~dp0\\..\\..\\zig\\zig.exe\" ar %*\r\nexit /b %ERRORLEVEL%\r\n",
        )
        .map_err(|error| CrossTargetToolchainError::Io {
            path: archiver,
            message: error.to_string(),
        })?;
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        let wrapper = bin.join("x86_64-unknown-linux-gnu-zig-cc");
        fs::write(&wrapper, LINUX_ZIG_CC_WRAPPER).map_err(|error| {
            CrossTargetToolchainError::Io {
                path: wrapper.clone(),
                message: error.to_string(),
            }
        })?;
        let mut permissions = fs::metadata(&wrapper)
            .map_err(|error| CrossTargetToolchainError::Io {
                path: wrapper.clone(),
                message: error.to_string(),
            })?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&wrapper, permissions).map_err(|error| {
            CrossTargetToolchainError::Io {
                path: wrapper,
                message: error.to_string(),
            }
        })?;
        let archiver = bin.join("zig-ar");
        fs::write(
            &archiver,
            "#!/usr/bin/env sh\nscript_dir=\"$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\"\nexec \"$script_dir/../../zig/zig\" ar \"$@\"\n",
        )
        .map_err(|error| CrossTargetToolchainError::Io {
            path: archiver.clone(),
            message: error.to_string(),
        })?;
        let mut permissions = fs::metadata(&archiver)
            .map_err(|error| CrossTargetToolchainError::Io {
                path: archiver.clone(),
                message: error.to_string(),
            })?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&archiver, permissions).map_err(|error| {
            CrossTargetToolchainError::Io {
                path: archiver,
                message: error.to_string(),
            }
        })?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
const LINUX_ZIG_CC_WRAPPER: &str = r##"#!/usr/bin/env bash
set -e
args=()
skip=0
for arg in "$@"; do
  if (( skip )); then skip=0; continue; fi
  case "$arg" in
    --target=*) ;;
    --target) skip=1 ;;
    *) args+=("$arg") ;;
  esac
done
script_dir="$(cd -- "$(dirname -- "$0")" && pwd)"
exec "$script_dir/../../zig/zig" cc -target x86_64-linux-gnu "${args[@]}"
"##;

#[cfg(windows)]
const WINDOWS_ZIG_CC_WRAPPER: &str = r#"$ErrorActionPreference = 'Stop'
function Quote-Argument([string]$Value) {
    $builder = New-Object System.Text.StringBuilder
    [void]$builder.Append('"')
    $slashes = 0
    foreach ($character in $Value.ToCharArray()) {
        if ($character -eq '\') { $slashes++; continue }
        if ($character -eq '"') {
            [void]$builder.Append(('\' * (2 * $slashes + 1)))
            [void]$builder.Append('"')
            $slashes = 0
            continue
        }
        if ($slashes -gt 0) { [void]$builder.Append(('\' * $slashes)); $slashes = 0 }
        [void]$builder.Append($character)
    }
    if ($slashes -gt 0) { [void]$builder.Append(('\' * (2 * $slashes))) }
    [void]$builder.Append('"')
    return $builder.ToString()
}
$filtered = New-Object System.Collections.Generic.List[string]
$skip = $false
foreach ($argument in $args) {
    if ($skip) { $skip = $false; continue }
    if ($argument -eq '--target') { $skip = $true; continue }
    if ($argument.StartsWith('--target=')) { continue }
    $filtered.Add($argument)
}
$zig = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\zig\zig.exe'))
$arguments = @('cc', '-target', 'x86_64-linux-gnu') + @($filtered | ForEach-Object { Quote-Argument $_ })
$start = New-Object System.Diagnostics.ProcessStartInfo
$start.FileName = $zig
$start.Arguments = $arguments -join ' '
$start.UseShellExecute = $false
$process = [System.Diagnostics.Process]::Start($start)
$process.WaitForExit()
exit $process.ExitCode
"#;

fn write_zig_toolchain_marker(
    root: &Path,
    checksum: &str,
) -> Result<(), CrossTargetToolchainError> {
    let marker = root.join(TOOLCHAIN_MARKER);
    let source = format!(
        "schema = 1\nversion = \"{LINUX_GNU_LINKER_VERSION}\"\ntarget = \"x86_64-unknown-linux-gnu\"\nsha256 = \"{checksum}\"\n"
    );
    fs::write(&marker, source).map_err(|error| CrossTargetToolchainError::Io {
        path: marker,
        message: error.to_string(),
    })
}

/// Whether the selected installation has the linker and archiver Vapor uses
/// for `x86_64-pc-windows-gnullvm` builds.
pub fn windows_gnu_linker_installed(installation: &VaporInstallation) -> bool {
    windows_gnu_linker_installed_at(&installation.user_data_root())
}

fn windows_gnu_linker_installed_at(vapor_home: &Path) -> bool {
    is_supported_ucrt_bundle(&vapor_home.join("tools/llvm-mingw"))
}

/// Install the pinned LLVM MinGW release into the active Vapor user's tools.
///
/// A previous unmarked bundle is moved aside after the replacement has been
/// downloaded, verified, extracted, and checked. Failed installs leave the
/// active bundle untouched.
pub fn install_windows_gnu_linker(
    installation: &VaporInstallation,
) -> Result<bool, CrossTargetToolchainError> {
    if windows_gnu_linker_installed(installation) {
        return Ok(false);
    }

    if !cfg!(target_arch = "x86_64") {
        return Err(CrossTargetToolchainError::UnsupportedHost);
    }
    let (url, checksum, archive_name) = if cfg!(target_os = "linux") {
        (
            LINUX_ARCHIVE_URL,
            LINUX_ARCHIVE_SHA256,
            format!("llvm-mingw-{WINDOWS_GNU_LINKER_VERSION}-ucrt-ubuntu-22.04-x86_64.tar.xz"),
        )
    } else if cfg!(target_os = "windows") {
        (
            WINDOWS_ARCHIVE_URL,
            WINDOWS_ARCHIVE_SHA256,
            format!("llvm-mingw-{WINDOWS_GNU_LINKER_VERSION}-ucrt-x86_64.zip"),
        )
    } else {
        return Err(CrossTargetToolchainError::UnsupportedHost);
    };

    let vapor_home = installation.user_data_root();
    let final_root = vapor_home.join("tools/llvm-mingw");

    let tools_root = vapor_home.join("tools");
    let downloads = vapor_home.join("cache/downloads/toolchains");
    fs::create_dir_all(&tools_root).map_err(|error| CrossTargetToolchainError::Io {
        path: tools_root.clone(),
        message: error.to_string(),
    })?;
    fs::create_dir_all(&downloads).map_err(|error| CrossTargetToolchainError::Io {
        path: downloads.clone(),
        message: error.to_string(),
    })?;

    let archive = downloads.join(archive_name);
    let temporary = downloads.join(format!(".llvm-mingw-{}.download", std::process::id()));
    let staging = tools_root.join(format!(".llvm-mingw-{}.extracting", std::process::id()));
    let result = (|| {
        let mut response = Client::builder()
            .user_agent("Vapor managed cross-target toolchain")
            .build()
            .map_err(|error| CrossTargetToolchainError::Download(error.to_string()))?
            .get(url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|error| CrossTargetToolchainError::Download(error.to_string()))?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_TOOLCHAIN_ARCHIVE_BYTES)
        {
            return Err(CrossTargetToolchainError::ArchiveTooLarge(
                MAX_TOOLCHAIN_ARCHIVE_BYTES,
            ));
        }
        let mut file =
            fs::File::create(&temporary).map_err(|error| CrossTargetToolchainError::Io {
                path: temporary.clone(),
                message: error.to_string(),
            })?;
        let bytes = io::copy(
            &mut response.by_ref().take(MAX_TOOLCHAIN_ARCHIVE_BYTES + 1),
            &mut file,
        )
        .map_err(|error| CrossTargetToolchainError::Download(error.to_string()))?;
        if bytes > MAX_TOOLCHAIN_ARCHIVE_BYTES {
            return Err(CrossTargetToolchainError::ArchiveTooLarge(
                MAX_TOOLCHAIN_ARCHIVE_BYTES,
            ));
        }
        file.sync_all()
            .map_err(|error| CrossTargetToolchainError::Io {
                path: temporary.clone(),
                message: error.to_string(),
            })?;
        verify_sha256_file(&temporary, checksum, url)
            .map_err(CrossTargetToolchainError::Integrity)?;
        fs::rename(&temporary, &archive).map_err(|error| CrossTargetToolchainError::Io {
            path: archive.clone(),
            message: error.to_string(),
        })?;
        fs::create_dir(&staging).map_err(|error| CrossTargetToolchainError::Io {
            path: staging.clone(),
            message: error.to_string(),
        })?;
        extract_archive(&archive, &staging)?;
        flatten_single_root(&staging)?;

        let staged_installation = VaporInstallation {
            root: staging.clone(),
            root_source: installation.root_source,
        };
        if !linker_files_installed(&staged_installation.root.join("bin")) {
            return Err(CrossTargetToolchainError::IncompleteArchive {
                tool: "LLVM-MinGW",
                path: staging.clone(),
            });
        }
        write_toolchain_marker(&staging, checksum)?;

        promote_staged_bundle(&staging, &final_root, &tools_root)?;
        let _ = fs::remove_file(&archive);
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        let _ = fs::remove_dir_all(&staging);
    }
    result.map(|()| true)
}

fn executable_in(directory: &Path, stem: &str) -> bool {
    let executable = format!("{stem}{}", env::consts::EXE_SUFFIX);
    directory.join(executable).is_file()
}

fn linker_files_installed(bin: &Path) -> bool {
    [
        "x86_64-w64-mingw32-clang",
        "x86_64-w64-mingw32-dlltool",
        "llvm-dlltool",
        "x86_64-w64-mingw32-ar",
    ]
    .into_iter()
    .all(|tool| executable_in(bin, tool))
        && bin
            .parent()
            .is_some_and(|root| root.join("x86_64-w64-mingw32/bin/libunwind.dll").is_file())
}

fn is_supported_ucrt_bundle(root: &Path) -> bool {
    let marker = match fs::read_to_string(root.join(TOOLCHAIN_MARKER)) {
        Ok(marker) => marker,
        Err(_) => return false,
    };
    [
        "schema = 1",
        &format!("version = \"{WINDOWS_GNU_LINKER_VERSION}\""),
        "runtime = \"ucrt\"",
        "target = \"x86_64-pc-windows-gnullvm\"",
    ]
    .into_iter()
    .all(|required| marker.lines().any(|line| line.trim() == required))
        && linker_files_installed(&root.join("bin"))
}

fn write_toolchain_marker(root: &Path, checksum: &str) -> Result<(), CrossTargetToolchainError> {
    let marker = root.join(TOOLCHAIN_MARKER);
    let source = format!(
        "schema = 1\nversion = \"{WINDOWS_GNU_LINKER_VERSION}\"\nruntime = \"ucrt\"\ntarget = \"x86_64-pc-windows-gnullvm\"\nsha256 = \"{checksum}\"\n"
    );
    fs::write(&marker, source).map_err(|error| CrossTargetToolchainError::Io {
        path: marker,
        message: error.to_string(),
    })
}

fn promote_staged_bundle(
    staging: &Path,
    final_root: &Path,
    tools_root: &Path,
) -> Result<(), CrossTargetToolchainError> {
    if !final_root.exists() {
        return fs::rename(staging, final_root).map_err(|error| CrossTargetToolchainError::Io {
            path: final_root.to_path_buf(),
            message: error.to_string(),
        });
    }

    let previous_root = tools_root.join(format!(
        "llvm-mingw-previous-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::rename(final_root, &previous_root).map_err(|error| CrossTargetToolchainError::Io {
        path: final_root.to_path_buf(),
        message: format!("could not preserve the previous toolchain: {error}"),
    })?;
    if let Err(error) = fs::rename(staging, final_root) {
        let restore = fs::rename(&previous_root, final_root);
        let restore_context = restore
            .err()
            .map(|restore| format!("; restoring the previous toolchain also failed: {restore}"))
            .unwrap_or_default();
        return Err(CrossTargetToolchainError::Io {
            path: final_root.to_path_buf(),
            message: format!("could not install the verified toolchain: {error}{restore_context}"),
        });
    }
    Ok(())
}

fn extract_archive(archive: &Path, destination: &Path) -> Result<(), CrossTargetToolchainError> {
    #[cfg(target_os = "linux")]
    let output = crate::managed_process::background_command("tar")
        .args(["-xJf"])
        .arg(archive)
        .arg("-C")
        .arg(destination)
        .arg("--strip-components=1")
        .output();

    #[cfg(target_os = "windows")]
    let output = {
        let command = format!(
            "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
            powershell_quote(&archive.to_string_lossy()),
            powershell_quote(&destination.to_string_lossy()),
        );
        crate::managed_process::background_command("powershell")
            .args(["-NoProfile", "-Command"])
            .arg(command)
            .output()
    };

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let output: Result<std::process::Output, std::io::Error> = Err(std::io::Error::other(
        "Windows GNU linker installation is unsupported on this host",
    ));

    let output = output.map_err(|error| CrossTargetToolchainError::Extract {
        archive: archive.to_path_buf(),
        message: error.to_string(),
    })?;
    if !output.status.success() {
        return Err(CrossTargetToolchainError::Extract {
            archive: archive.to_path_buf(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(())
}

fn flatten_single_root(staging: &Path) -> Result<(), CrossTargetToolchainError> {
    let mut entries = fs::read_dir(staging)
        .map_err(|error| CrossTargetToolchainError::Io {
            path: staging.to_path_buf(),
            message: error.to_string(),
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| CrossTargetToolchainError::Io {
            path: staging.to_path_buf(),
            message: error.to_string(),
        })?;
    if entries.len() == 1 && entries[0].file_type().is_ok_and(|kind| kind.is_dir()) {
        let nested = entries.remove(0).path();
        for entry in fs::read_dir(&nested).map_err(|error| CrossTargetToolchainError::Io {
            path: nested.clone(),
            message: error.to_string(),
        })? {
            let entry = entry.map_err(|error| CrossTargetToolchainError::Io {
                path: nested.clone(),
                message: error.to_string(),
            })?;
            fs::rename(entry.path(), staging.join(entry.file_name())).map_err(|error| {
                CrossTargetToolchainError::Io {
                    path: staging.join(entry.file_name()),
                    message: error.to_string(),
                }
            })?;
        }
        fs::remove_dir(nested).map_err(|error| CrossTargetToolchainError::Io {
            path: staging.to_path_buf(),
            message: error.to_string(),
        })?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn powershell_quote(value: &str) -> String {
    value.replace('\'', "''")
}

#[derive(Debug)]
pub enum CrossTargetToolchainError {
    UnsupportedHost,
    IncompleteArchive { tool: &'static str, path: PathBuf },
    ArchiveTooLarge(u64),
    Download(String),
    Integrity(ManagedToolError),
    Extract { archive: PathBuf, message: String },
    Io { path: PathBuf, message: String },
}

impl std::fmt::Display for CrossTargetToolchainError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedHost => write!(
                formatter,
                "cross-target build tools are available only on x86-64 Linux and Windows hosts"
            ),
            Self::IncompleteArchive { tool, path } => write!(
                formatter,
                "the downloaded {tool} archive did not contain the expected tool files at '{}'",
                path.display()
            ),
            Self::ArchiveTooLarge(limit) => write!(
                formatter,
                "the cross-target tool archive exceeds the {} GiB safety limit",
                limit / (1024 * 1024 * 1024)
            ),
            Self::Download(message) => write!(
                formatter,
                "could not download the cross-target build tools: {message}"
            ),
            Self::Integrity(error) => error.fmt(formatter),
            Self::Extract { archive, message } => write!(
                formatter,
                "could not extract cross-target tools from '{}': {message}",
                archive.display()
            ),
            Self::Io { path, message } => write!(
                formatter,
                "could not update '{}': {message}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for CrossTargetToolchainError {}

#[cfg(test)]
mod tests {
    use super::{is_supported_ucrt_bundle, promote_staged_bundle, windows_gnu_linker_installed_at};
    use std::{env, fs};

    #[test]
    fn windows_gnu_linker_readiness_requires_all_link_tools_runtime_and_ucrt_marker() {
        let root = env::temp_dir().join(format!(
            "vapor-windows-gnu-toolchain-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("tools/llvm-mingw/bin")).expect("create fake tool directory");
        assert!(!windows_gnu_linker_installed_at(&root));
        let suffix = env::consts::EXE_SUFFIX;
        for tool in [
            "x86_64-w64-mingw32-clang",
            "x86_64-w64-mingw32-dlltool",
            "llvm-dlltool",
            "x86_64-w64-mingw32-ar",
        ] {
            fs::write(
                root.join(format!("tools/llvm-mingw/bin/{tool}{suffix}")),
                "tool",
            )
            .expect("write fake cross tool");
            assert!(!windows_gnu_linker_installed_at(&root));
        }
        fs::create_dir_all(root.join("tools/llvm-mingw/x86_64-w64-mingw32/bin"))
            .expect("create target runtime directory");
        fs::write(
            root.join("tools/llvm-mingw/x86_64-w64-mingw32/bin/libunwind.dll"),
            "runtime",
        )
        .expect("write fake runtime");
        assert!(!windows_gnu_linker_installed_at(&root));
        fs::write(
            root.join("tools/llvm-mingw/vapor-toolchain.toml"),
            "schema = 1\nversion = \"20260616\"\nruntime = \"ucrt\"\ntarget = \"x86_64-pc-windows-gnullvm\"\n",
        )
        .expect("write the managed target metadata");
        assert!(windows_gnu_linker_installed_at(&root));
        assert!(!is_supported_ucrt_bundle(&root.join("tools")));

        fs::remove_dir_all(root).expect("remove fake tool directory");
    }

    #[test]
    fn verified_bundle_replaces_old_bundle_without_discarding_it() {
        let root = env::temp_dir().join(format!(
            "vapor-cross-toolchain-swap-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&root);
        let tools = root.join("tools");
        let current = tools.join("llvm-mingw");
        let previous_content = current.join("old-toolchain.txt");
        fs::create_dir_all(&current).expect("create the previous toolchain");
        fs::write(&previous_content, "previous").expect("write previous marker");
        let staging = tools.join(".llvm-mingw-staging");
        fs::create_dir_all(&staging).expect("create staged toolchain");
        fs::write(staging.join("new-toolchain.txt"), "verified").expect("write staged marker");

        promote_staged_bundle(&staging, &current, &tools).expect("promote staged toolchain");

        assert!(current.join("new-toolchain.txt").is_file());
        assert!(!current.join("old-toolchain.txt").exists());
        let preserved = fs::read_dir(&tools)
            .expect("read tools directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("llvm-mingw-previous-"))
            })
            .expect("previous bundle is preserved");
        assert!(preserved.join("old-toolchain.txt").is_file());
        fs::remove_dir_all(root).expect("remove test toolchains");
    }

    #[test]
    fn failed_bundle_promotion_restores_the_previous_toolchain() {
        let root = env::temp_dir().join(format!(
            "vapor-cross-toolchain-restore-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&root);
        let tools = root.join("tools");
        let current = tools.join("llvm-mingw");
        fs::create_dir_all(&current).expect("create the previous toolchain");
        fs::write(current.join("old-toolchain.txt"), "previous").expect("write old marker");
        let staging = tools.join("missing-staging");

        assert!(promote_staged_bundle(&staging, &current, &tools).is_err());
        assert_eq!(
            fs::read_to_string(current.join("old-toolchain.txt"))
                .expect("previous toolchain is restored"),
            "previous"
        );
        fs::remove_dir_all(root).expect("remove test toolchains");
    }

    fn unique_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after UNIX epoch")
            .as_nanos()
    }
}
