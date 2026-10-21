//! Installation-owned native tools required to build declared foreign targets.

use crate::{ManagedToolError, VaporInstallation, managed_tool::verify_sha256_file};
use reqwest::blocking::Client;
use std::{
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

pub const WINDOWS_GNU_LINKER_VERSION: &str = "20260616";

const LINUX_ARCHIVE_URL: &str = "https://github.com/mstorsjo/llvm-mingw/releases/download/20260616/llvm-mingw-20260616-ucrt-ubuntu-22.04-x86_64.tar.xz";
const LINUX_ARCHIVE_SHA256: &str =
    "534b92e067b22a6b4441f48ae9240a3341b17825d04d577eab0cf85c44b4deda";
const WINDOWS_ARCHIVE_URL: &str = "https://github.com/mstorsjo/llvm-mingw/releases/download/20260616/llvm-mingw-20260616-ucrt-x86_64.zip";
const WINDOWS_ARCHIVE_SHA256: &str =
    "b9b68a4d276e16fa25802aaba458e4638f64b3884c290aaccdc2d87083b6ca35";
const MAX_TOOLCHAIN_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const TOOLCHAIN_MARKER: &str = "vapor-toolchain.toml";

/// Whether the selected installation has the linker and archiver Vapor uses
/// for `x86_64-pc-windows-gnullvm` builds.
pub fn windows_gnu_linker_installed(installation: &VaporInstallation) -> bool {
    is_supported_ucrt_bundle(&installation.root.join("tools/llvm-mingw"))
}

/// Install the pinned LLVM MinGW release into the selected app installation.
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

    let final_root = installation.root.join("tools/llvm-mingw");

    let tools_root = installation.root.join("tools");
    let downloads = installation.root.join(".vapor/downloads");
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
            return Err(CrossTargetToolchainError::IncompleteArchive(
                staging.clone(),
            ));
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
    IncompleteArchive(PathBuf),
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
                "Windows GNU build tools are available only on x86-64 Linux and Windows hosts"
            ),
            Self::IncompleteArchive(path) => write!(
                formatter,
                "the downloaded Windows GNU tool archive did not contain the required compiler and archiver at '{}'",
                path.display()
            ),
            Self::ArchiveTooLarge(limit) => write!(
                formatter,
                "the Windows GNU tool archive exceeds the {} GiB safety limit",
                limit / (1024 * 1024 * 1024)
            ),
            Self::Download(message) => write!(
                formatter,
                "could not download the Windows GNU build tools: {message}"
            ),
            Self::Integrity(error) => error.fmt(formatter),
            Self::Extract { archive, message } => write!(
                formatter,
                "could not extract Windows GNU tools from '{}': {message}",
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
    use super::{is_supported_ucrt_bundle, promote_staged_bundle, windows_gnu_linker_installed};
    use crate::{InstallationRootSource, VaporInstallation};
    use std::{env, fs};

    #[test]
    fn windows_gnu_linker_readiness_requires_all_link_tools_runtime_and_ucrt_marker() {
        let root = env::temp_dir().join(format!(
            "vapor-windows-gnu-toolchain-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("tools/llvm-mingw/bin")).expect("create fake tool directory");
        let installation = VaporInstallation {
            root: root.clone(),
            root_source: InstallationRootSource::Explicit,
        };

        assert!(!windows_gnu_linker_installed(&installation));
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
            assert!(!windows_gnu_linker_installed(&installation));
        }
        fs::create_dir_all(root.join("tools/llvm-mingw/x86_64-w64-mingw32/bin"))
            .expect("create target runtime directory");
        fs::write(
            root.join("tools/llvm-mingw/x86_64-w64-mingw32/bin/libunwind.dll"),
            "runtime",
        )
        .expect("write fake runtime");
        assert!(!windows_gnu_linker_installed(&installation));
        fs::write(
            root.join("tools/llvm-mingw/vapor-toolchain.toml"),
            "schema = 1\nversion = \"20260616\"\nruntime = \"ucrt\"\ntarget = \"x86_64-pc-windows-gnullvm\"\n",
        )
        .expect("write the managed target metadata");
        assert!(windows_gnu_linker_installed(&installation));
        assert!(!is_supported_ucrt_bundle(&root.join("tools")));

        fs::remove_dir_all(root).expect("remove fake tool directory");
    }

    #[test]
    fn verified_bundle_replaces_old_bundle_without_discarding_it() {
        let root = env::temp_dir().join(format!(
            "vapor-cross-toolchain-swap-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
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
            std::thread::current().name().unwrap_or("test")
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
}
