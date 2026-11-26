//! Small path contract shared by Vapor's standalone installer and Steam handoff.
//!
//! The Steam entrypoint intentionally depends on this crate instead of the full
//! Vapor Core, keeping the depot bootstrap small while resolving the same
//! OS-owned user-data root and active standalone application.

use std::{
    env, fmt, fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use semver::Version;

pub const APPLICATION_DIRECTORY: &str = "apps/vapor";
pub const RELEASES_DIRECTORY: &str = "releases";
pub const ACTIVE_VERSION_FILE: &str = "active.version";
static NEXT_POINTER_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub enum ProductLayoutError {
    InvalidVersion(String),
    InvalidProductPath(PathBuf),
    ReadActiveVersion { path: PathBuf, source: io::Error },
    ActiveEntrypointMissing(PathBuf),
    InvalidActivePointer(PathBuf),
    CreateDirectory { path: PathBuf, source: io::Error },
    WriteActiveVersion { path: PathBuf, source: io::Error },
    ListReleases { path: PathBuf, source: io::Error },
}

impl fmt::Display for ProductLayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidVersion(version) => write!(
                formatter,
                "the active Vapor version pointer contains an unsafe version segment: {version:?}"
            ),
            Self::InvalidProductPath(path) => write!(
                formatter,
                "Vapor product path must not traverse a symbolic link or non-directory: {}",
                path.display()
            ),
            Self::ReadActiveVersion { path, source } => write!(
                formatter,
                "could not read active Vapor version pointer {}: {source}",
                path.display()
            ),
            Self::ActiveEntrypointMissing(path) => write!(
                formatter,
                "the active Vapor release is incomplete; its entrypoint is missing: {}",
                path.display()
            ),
            Self::InvalidActivePointer(path) => write!(
                formatter,
                "the active Vapor version pointer must be a regular file: {}",
                path.display()
            ),
            Self::CreateDirectory { path, source } => write!(
                formatter,
                "could not create Vapor release directory {}: {source}",
                path.display()
            ),
            Self::WriteActiveVersion { path, source } => write!(
                formatter,
                "could not safely update the active Vapor version pointer {}: {source}",
                path.display()
            ),
            Self::ListReleases { path, source } => write!(
                formatter,
                "could not list installed Vapor releases in {}: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ProductLayoutError {}

/// Return Vapor's canonical per-user data root using the shared OS policy.
pub fn user_data_root() -> Option<PathBuf> {
    match env::consts::OS {
        "windows" => env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("USERPROFILE")
                    .map(|home| PathBuf::from(home).join("AppData/Local"))
            })
            .map(|root| root.join("Vapor")),
        "macos" => env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Library/Application Support/Vapor")),
        _ => env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|value| value.is_absolute())
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
            .map(|root| root.join("vapor")),
    }
}

/// Canonical persistent and disposable paths below Vapor User Data.
///
/// Release binaries live in immutable versioned roots. Toolchains, Cargo
/// registries, caches, downloads, and generated output use this separate
/// user-data layout so installing another app version never changes their
/// location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaporUserDataLayout {
    root: PathBuf,
}

impl VaporUserDataLayout {
    pub fn discover() -> Option<Self> {
        user_data_root().map(Self::from_root)
    }

    pub fn from_root(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn tools_root(&self) -> PathBuf {
        self.root.join("tools")
    }

    pub fn cargo_home(&self) -> PathBuf {
        self.root.join("tools/cargo/home")
    }

    pub fn cargo_bin(&self) -> PathBuf {
        self.cargo_home().join("bin")
    }

    pub fn rustup_home(&self) -> PathBuf {
        self.root.join("tools/rustup/home")
    }

    pub fn rustup_bin(&self) -> PathBuf {
        self.root.join("tools/rustup/bin")
    }

    pub fn rust_toolchains(&self) -> PathBuf {
        self.rustup_home().join("toolchains")
    }

    pub fn build_cache(&self) -> PathBuf {
        self.root.join("cache/build")
    }

    pub fn output_root(&self) -> PathBuf {
        self.root.join("output")
    }
}

/// Locations for immutable standalone Vapor releases in Vapor User Data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandaloneProductLayout {
    root: PathBuf,
}

impl StandaloneProductLayout {
    pub fn discover() -> Option<Self> {
        user_data_root().map(|root| Self::from_user_data_root(root))
    }

    pub fn from_user_data_root(user_data_root: PathBuf) -> Self {
        Self {
            root: user_data_root.join(APPLICATION_DIRECTORY),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn releases_root(&self) -> PathBuf {
        self.root.join(RELEASES_DIRECTORY)
    }

    pub fn active_version_path(&self) -> PathBuf {
        self.root.join(ACTIVE_VERSION_FILE)
    }

    pub fn release_root(&self, version: &str) -> Result<PathBuf, ProductLayoutError> {
        if !valid_version_segment(version) {
            return Err(ProductLayoutError::InvalidVersion(version.to_owned()));
        }
        Ok(self.releases_root().join(version))
    }

    /// Resolve the platform-specific entrypoint in one immutable release.
    pub fn entrypoint_path(&self, version: &str) -> Result<PathBuf, ProductLayoutError> {
        let release_root = self.release_root(version)?;
        let target = current_target_triple().ok_or_else(|| {
            ProductLayoutError::ActiveEntrypointMissing(release_root.join("bin"))
        })?;
        Ok(release_root
            .join("bin")
            .join(target)
            .join(format!("vapor-entrypoint{}", env::consts::EXE_SUFFIX)))
    }

    /// Read the active release key, validating it as a single safe directory
    /// segment before returning it.
    pub fn active_version(&self) -> Result<Option<String>, ProductLayoutError> {
        let apps_root = self.root.parent().ok_or_else(|| {
            ProductLayoutError::InvalidProductPath(self.root.clone())
        })?;
        for directory in [apps_root, self.root.as_path()] {
            match fs::symlink_metadata(directory) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    return Err(ProductLayoutError::InvalidProductPath(directory.to_path_buf()));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(source) => {
                    return Err(ProductLayoutError::ReadActiveVersion {
                        path: self.active_version_path(),
                        source,
                    });
                }
            }
        }
        let pointer = self.active_version_path();
        match fs::symlink_metadata(&pointer) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(ProductLayoutError::InvalidActivePointer(pointer));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ProductLayoutError::ReadActiveVersion {
                    path: pointer,
                    source,
                });
            }
        }
        let version = match fs::read_to_string(&pointer) {
            Ok(version) => version,
            Err(source) => {
                return Err(ProductLayoutError::ReadActiveVersion {
                    path: pointer,
                    source,
                });
            }
        };
        let version = version.trim();
        if !valid_version_segment(version) {
            return Err(ProductLayoutError::InvalidVersion(version.to_owned()));
        }
        Ok(Some(version.to_owned()))
    }

    /// Atomically point Steam's creator handoff at a complete installed release.
    /// The pointer is replaced only after its new contents have been flushed.
    pub fn activate_version(&self, version: &str) -> Result<(), ProductLayoutError> {
        self.validated_entrypoint_path(version)?;
        let apps_root = self.root.parent().ok_or_else(|| ProductLayoutError::CreateDirectory {
            path: self.root.clone(),
            source: io::Error::new(io::ErrorKind::InvalidInput, "product root has no parent"),
        })?;
        for directory in [apps_root, self.root.as_path()] {
            let metadata = fs::symlink_metadata(directory).map_err(|source| {
                ProductLayoutError::CreateDirectory {
                    path: directory.to_path_buf(),
                    source,
                }
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(ProductLayoutError::CreateDirectory {
                    path: directory.to_path_buf(),
                    source: io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Vapor product path must not traverse a symbolic link",
                    ),
                });
            }
        }
        fs::create_dir_all(&self.root).map_err(|source| ProductLayoutError::CreateDirectory {
            path: self.root.clone(),
            source,
        })?;

        let pointer = self.active_version_path();
        for _ in 0..16 {
            let sequence = NEXT_POINTER_TEMP.fetch_add(1, Ordering::Relaxed);
            let temporary = self.root.join(format!(
                ".{ACTIVE_VERSION_FILE}.{}.{}.tmp",
                std::process::id(),
                sequence
            ));
            let mut file = match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(ProductLayoutError::WriteActiveVersion {
                        path: temporary,
                        source,
                    });
                }
            };
            let result = (|| {
                use io::Write;
                writeln!(file, "{version}")?;
                file.sync_all()?;
                replace_pointer(&temporary, &pointer)?;
                #[cfg(unix)]
                FileSync::sync_directory(&self.root)?;
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            return result.map_err(|source| ProductLayoutError::WriteActiveVersion {
                path: pointer,
                source,
            });
        }
        Err(ProductLayoutError::WriteActiveVersion {
            path: pointer,
            source: io::Error::new(
                io::ErrorKind::AlreadyExists,
                "could not allocate a unique temporary pointer file",
            ),
        })
    }

    /// List complete releases available for this machine, newest SemVer first.
    pub fn installed_versions(&self) -> Result<Vec<String>, ProductLayoutError> {
        let releases = self.releases_root();
        match fs::symlink_metadata(&releases) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(ProductLayoutError::ListReleases {
                    path: releases,
                    source: io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Vapor releases path must be a real directory",
                    ),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(ProductLayoutError::ListReleases {
                    path: releases,
                    source,
                });
            }
        }
        let entries = match fs::read_dir(&releases) {
            Ok(entries) => entries,
            Err(source) => {
                return Err(ProductLayoutError::ListReleases {
                    path: releases,
                    source,
                });
            }
        };
        let mut versions = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| ProductLayoutError::ListReleases {
                path: releases.clone(),
                source,
            })?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !valid_version_segment(name) || !entry.file_type().map_err(|source| {
                ProductLayoutError::ListReleases {
                    path: entry.path(),
                    source,
                }
            })?.is_dir() {
                continue;
            }
            if self.validated_entrypoint_path(name).is_ok() {
                versions.push(name.to_owned());
            }
        }
        versions.sort_by(|left, right| compare_release_versions(left, right));
        Ok(versions)
    }

    /// Resolve the active release's Steam entrypoint, if one is installed.
    ///
    /// The pointer is local state, not a source of trust: it can select only a
    /// single safe child directory under the registered Vapor release root.
    pub fn active_entrypoint(&self) -> Result<Option<PathBuf>, ProductLayoutError> {
        if current_platform_id().is_none() {
            return Ok(None);
        }
        let Some(version) = self.active_version()? else {
            return Ok(None);
        };
        self.validated_entrypoint_path(&version).map(Some)
    }

    fn validated_entrypoint_path(&self, version: &str) -> Result<PathBuf, ProductLayoutError> {
        let path = self.entrypoint_path(version)?;
        let apps_root = self.root.parent().ok_or_else(|| {
            ProductLayoutError::ActiveEntrypointMissing(path.clone())
        })?;
        let releases_root = self.releases_root();
        for directory in [apps_root, self.root.as_path(), releases_root.as_path()] {
            let metadata = fs::symlink_metadata(directory)
                .map_err(|_| ProductLayoutError::ActiveEntrypointMissing(path.clone()))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(ProductLayoutError::ActiveEntrypointMissing(path));
            }
        }
        let version_root = self.release_root(version)
            .map_err(|_| ProductLayoutError::ActiveEntrypointMissing(path.clone()))?;
        let bin_root = version_root.join("bin");
        let target_root = bin_root.join(current_target_triple().unwrap_or_default());
        for directory in [version_root.as_path(), bin_root.as_path(), target_root.as_path()] {
            let metadata = fs::symlink_metadata(directory)
                .map_err(|_| ProductLayoutError::ActiveEntrypointMissing(path.clone()))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(ProductLayoutError::ActiveEntrypointMissing(path));
            }
        }
        let canonical_releases = releases_root
            .canonicalize()
            .map_err(|_| ProductLayoutError::ActiveEntrypointMissing(path.clone()))?;
        let canonical_release = version_root
            .canonicalize().ok()
            .filter(|root| root.starts_with(&canonical_releases))
            .ok_or_else(|| ProductLayoutError::ActiveEntrypointMissing(path.clone()))?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| ProductLayoutError::ActiveEntrypointMissing(path.clone()))?;
        let canonical_entrypoint = path
            .canonicalize()
            .map_err(|_| ProductLayoutError::ActiveEntrypointMissing(path.clone()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file()
            || !canonical_entrypoint.starts_with(&canonical_release)
        {
            return Err(ProductLayoutError::ActiveEntrypointMissing(path));
        }
        Ok(path)
    }
}

#[cfg(unix)]
struct FileSync;

#[cfg(unix)]
impl FileSync {
    fn sync_directory(path: &Path) -> io::Result<()> {
        fs::File::open(path)?.sync_all()
    }
}

pub fn current_platform_id() -> Option<&'static str> {
    match (env::consts::OS, env::consts::ARCH) {
        ("windows", "x86_64") => Some("windows-x86_64"),
        ("linux", "x86_64") => Some("linux-x86_64"),
        _ => None,
    }
}

fn current_target_triple() -> Option<&'static str> {
    match (env::consts::OS, env::consts::ARCH) {
        ("windows", "x86_64") => Some("x86_64-pc-windows-gnullvm"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        _ => None,
    }
}

fn replace_pointer(temporary: &Path, pointer: &Path) -> io::Result<()> {
    #[cfg(not(windows))]
    {
        fs::rename(temporary, pointer)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let temporary = temporary.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
        let pointer = pointer.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
        let result = unsafe {
            MoveFileExW(
                temporary.as_ptr(),
                pointer.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

fn valid_version_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'))
}

fn compare_release_versions(left: &str, right: &str) -> std::cmp::Ordering {
    fn key(value: &str) -> Option<(Version, bool, Option<Version>)> {
        if let Some(inner) = value.strip_prefix("0.5.0-pre-alpha-") {
            let inner = Version::parse(inner).ok()?;
            if !inner.pre.is_empty() || !inner.build.is_empty() {
                return None;
            }
            return Some((Version::parse("0.5.0").ok()?, false, Some(inner)));
        }
        let stable = Version::parse(value).ok()?;
        if !stable.pre.is_empty() {
            return None;
        }
        Some((stable, true, None))
    }

    match (key(left), key(right)) {
        (Some(left_key), Some(right_key)) => right_key
            .cmp(&left_key)
            .then_with(|| right.cmp(left)),
        _ => right.cmp(left),
    }
}

#[cfg(test)]
mod version_order_tests {
    use super::compare_release_versions;
    use std::cmp::Ordering;

    #[test]
    fn installed_release_versions_use_semver_precedence() {
        assert_eq!(
            compare_release_versions("0.5.0-pre-alpha-0.1.10", "0.5.0-pre-alpha-0.1.9"),
            Ordering::Less
        );
        assert_eq!(
            compare_release_versions("0.5.0-pre-alpha-0.2.0", "0.5.0-pre-alpha-0.1.99"),
            Ordering::Less
        );
    }
}
