//! Signed acquisition and transactional activation of Vapor-managed Git.
//!
//! The signed application manifest is the sole authority for distributable Git
//! bytes. The resulting provider lives under Vapor User Data and is never
//! copied into the replaceable App Instance or Steam installation.

use crate::{VaporInstallation, git_provider};
use flate2::read::GzDecoder;
use reqwest::blocking::{Client, Response};
use semver::Version;
use std::{
    collections::HashSet,
    env,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tar::{Archive, EntryType};

use crate::release::{self, VerifiedManagedGitRelease, VerifiedReleaseManifest};

const CHANNEL: &str = "stable";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_SIGNATURE_BYTES: u64 = 128;
const MAX_ARCHIVE_ENTRIES: usize = 20_000;
const MAX_UNPACKED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MANAGED_GIT_PATH: &str = "tools/git";
static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct ManagedGitReport {
    pub version: String,
    pub path: PathBuf,
    pub changed: bool,
}

/// Ensure that Developer setup has a working explicitly selected Git provider.
///
/// A valid environment override or linked provider remains authoritative. If
/// neither exists, Git is fetched from the signed stable release and installed
/// transactionally under `User Data/tools/git`.
pub fn ensure_installed(
    installation: &VaporInstallation,
    mut report_progress: impl FnMut(&str),
) -> Result<ManagedGitReport, ManagedGitError> {
    let user_data = installation.user_data_root();
    let existing = git_provider::resolve(&user_data, &installation.state_root());
    if let Ok(provider) = &existing {
        return Ok(ManagedGitReport {
            version: provider_version(provider)?,
            path: provider.path().to_path_buf(),
            changed: false,
        });
    }

    let environment_provider = env::var_os("VAPOR_GIT").is_some_and(|value| !value.is_empty());
    let linked_provider = git_provider::linked_path(&installation.state_root())
        .map_err(ManagedGitError::Provider)?
        .is_some();
    if environment_provider || linked_provider {
        return Err(ManagedGitError::Provider(existing.err().unwrap_or_else(|| "explicit provider could not be resolved".to_owned())));
    }

    report_progress("Verifying signed Vapor-managed Git release");
    let client = release_client()?;
    let platform = vapor_product_layout::current_platform_id()
        .ok_or(ManagedGitError::UnsupportedPlatform)?;
    let manifest = fetch_manifest(&client, platform)?;
    let git = manifest
        .managed_git
        .as_ref()
        .ok_or(ManagedGitError::ReleaseHasNoGit)?;

    report_progress("Downloading signed Git payload");
    let tools_root = user_data.join("tools");
    ensure_managed_directory(&user_data, &tools_root)?;
    let target = user_data.join(MANAGED_GIT_PATH);
    ensure_safe_existing_directory(&target)?;

    let stage = StageDirectory::create(&tools_root)?;
    let archive_path = stage.path.join("git.tar.gz");
    download_verified(&client, git, &archive_path)?;
    let payload = stage.path.join("payload");
    fs::create_dir(&payload).map_err(|source| io_error("create Git extraction staging directory", &payload, source))?;

    report_progress("Extracting and validating Git payload");
    extract_payload(&archive_path, &payload)?;
    validate_provider(&payload)?;

    report_progress("Activating Vapor-managed Git");
    activate(&payload, &target, &tools_root)?;
    let provider = git_provider::resolve(&user_data, &installation.state_root())
        .map_err(ManagedGitError::Provider)?;
    let version = provider_version(&provider)?;
    Ok(ManagedGitReport {
        version,
        path: provider.path().to_path_buf(),
        changed: true,
    })
}

fn release_client() -> Result<Client, ManagedGitError> {
    Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30 * 60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(ManagedGitError::Network)
}

fn fetch_manifest(client: &Client, platform: &str) -> Result<VerifiedReleaseManifest, ManagedGitError> {
    let feed = format!(
        "{}/api/updates/v1/vapor/{CHANNEL}/{platform}/latest.json",
        release::RELEASE_ORIGIN
    );
    let manifest_bytes = read_bounded(client, &feed, MAX_MANIFEST_BYTES)?;
    let signature_bytes = read_bounded(client, &format!("{feed}.sig"), MAX_SIGNATURE_BYTES)?;
    let signature = std::str::from_utf8(&signature_bytes)
        .map_err(|_| ManagedGitError::InvalidSignatureEncoding)?;
    release::verify_manifest(
        &manifest_bytes,
        signature,
        CHANNEL,
        platform,
        &Version::parse(env!("CARGO_PKG_VERSION")).expect("Vapor package version is valid SemVer"),
    )
    .map_err(ManagedGitError::ReleaseVerification)
}

fn read_bounded(client: &Client, url: &str, limit: u64) -> Result<Vec<u8>, ManagedGitError> {
    let mut response = successful_response(client.get(url).send(), url)?;
    if response.content_length().is_some_and(|length| length > limit) {
        return Err(ManagedGitError::MetadataTooLarge);
    }
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("read Vapor release metadata", Path::new(url), error))?;
    if bytes.len() as u64 > limit {
        return Err(ManagedGitError::MetadataTooLarge);
    }
    Ok(bytes)
}

fn download_verified(
    client: &Client,
    git: &VerifiedManagedGitRelease,
    destination: &Path,
) -> Result<(), ManagedGitError> {
    let response = successful_response(client.get(&git.artifact.url).send(), &git.artifact.url)?;
    if response
        .content_length()
        .is_some_and(|length| length != git.artifact.size)
    {
        return Err(ManagedGitError::InvalidPayload("Content-Length differs from the signed size"));
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|source| io_error("create staged Git archive", destination, source))?;
    let copied = io::copy(
        &mut response.take(git.artifact.size.saturating_add(1)),
        &mut output,
    )
    .map_err(|error| io_error("download signed Git archive", destination, error))?;
    output
        .flush()
        .and_then(|_| output.sync_all())
        .map_err(|source| io_error("flush staged Git archive", destination, source))?;
    if copied != git.artifact.size {
        return Err(ManagedGitError::InvalidPayload("downloaded size differs from the signed size"));
    }
    release::verify_artifact(File::open(destination).map_err(|source| io_error("reopen staged Git archive", destination, source))?, &git.artifact)
        .map_err(ManagedGitError::ReleaseVerification)
}

fn successful_response(
    result: Result<Response, reqwest::Error>,
    url: &str,
) -> Result<Response, ManagedGitError> {
    let response = result.map_err(ManagedGitError::Network)?;
    if response.url().scheme() != "https" || response.url().host_str() != Some("vapor.ghf-studios.site") {
        return Err(ManagedGitError::UntrustedOrigin);
    }
    if !response.status().is_success() {
        return Err(ManagedGitError::HttpStatus {
            url: url.to_owned(),
            status: response.status().as_u16(),
        });
    }
    Ok(response)
}

fn extract_payload(archive_path: &Path, destination: &Path) -> Result<(), ManagedGitError> {
    let archive = File::open(archive_path).map_err(|source| io_error("open verified Git archive", archive_path, source))?;
    let mut archive = Archive::new(GzDecoder::new(archive));
    let entries = archive.entries().map_err(ManagedGitError::Archive)?;
    let mut seen = HashSet::new();
    let mut total = 0_u64;
    let mut count = 0_usize;
    let mut has_files = false;
    for entry in entries {
        let mut entry = entry.map_err(ManagedGitError::Archive)?;
        count += 1;
        if count > MAX_ARCHIVE_ENTRIES {
            return Err(ManagedGitError::ArchiveLimit);
        }
        let kind = entry.header().entry_type();
        if kind != EntryType::Regular && kind != EntryType::Directory && kind != EntryType::Link {
            return Err(ManagedGitError::UnsupportedArchiveEntry);
        }
        let path = entry.path().map_err(ManagedGitError::Archive)?;
        let relative = safe_archive_path(&path)?;
        if relative.as_os_str().is_empty() {
            if kind == EntryType::Directory { continue; }
            return Err(ManagedGitError::UnsafeArchivePath);
        }
        let collision_key = relative.to_string_lossy().to_lowercase();
        if !seen.insert(collision_key) {
            return Err(ManagedGitError::DuplicateArchivePath);
        }
        if kind == EntryType::Regular {
            total = total.checked_add(entry.header().size().map_err(ManagedGitError::Archive)?)
                .filter(|size| *size <= MAX_UNPACKED_BYTES)
                .ok_or(ManagedGitError::ArchiveLimit)?;
            has_files = true;
        }
        if kind == EntryType::Link {
            // The Linux Git payload uses hard links for the argv0-dispatched
            // command aliases. Preserve those links without accepting symlinks
            // or allowing a link to name anything outside this staged tree.
            let link_name = entry
                .link_name()
                .map_err(ManagedGitError::Archive)?
                .ok_or(ManagedGitError::UnsafeArchiveLink)?;
            let link_relative = safe_archive_path(&link_name)?;
            let link_key = link_relative.to_string_lossy().to_lowercase();
            if !seen.contains(&link_key) {
                return Err(ManagedGitError::UnsafeArchiveLink);
            }
            let link_source = destination.join(&link_relative);
            let source_metadata = fs::symlink_metadata(&link_source)
                .map_err(|source| io_error("inspect Git hard-link target", &link_source, source))?;
            if !source_metadata.file_type().is_file() {
                return Err(ManagedGitError::UnsafeArchiveLink);
            }

            let output = destination.join(&relative);
            let parent = output
                .parent()
                .ok_or(ManagedGitError::UnsafeArchivePath)?;
            fs::create_dir_all(parent)
                .map_err(|source| io_error("create Git hard-link parent", parent, source))?;
            let canonical_destination = fs::canonicalize(destination)
                .map_err(|source| io_error("resolve Git extraction directory", destination, source))?;
            let canonical_parent = fs::canonicalize(parent)
                .map_err(|source| io_error("resolve Git hard-link parent", parent, source))?;
            if !canonical_parent.starts_with(&canonical_destination) {
                return Err(ManagedGitError::UnsafeArchivePath);
            }
            fs::hard_link(&link_source, &output)
                .map_err(|source| io_error("create Git hard link", &output, source))?;
            has_files = true;
        } else if !entry.unpack_in(destination).map_err(ManagedGitError::Archive)? {
            return Err(ManagedGitError::UnsafeArchivePath);
        }
    }
    if count == 0 || !has_files {
        return Err(ManagedGitError::EmptyArchive);
    }
    Ok(())
}

fn safe_archive_path(path: &Path) -> Result<PathBuf, ManagedGitError> {
    if path.is_absolute() {
        return Err(ManagedGitError::UnsafeArchivePath);
    }
    let mut safe = PathBuf::new();
    for component in path.components() {
        let Component::Normal(part) = component else {
            if matches!(component, Component::CurDir) { continue; }
            return Err(ManagedGitError::UnsafeArchivePath);
        };
        let text = part.to_str().ok_or(ManagedGitError::UnsafeArchivePath)?;
        if text.is_empty()
            || text.chars().any(|character| matches!(character, '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*'))
            || matches!(text.chars().last(), Some('.' | ' '))
            || text.chars().any(char::is_control)
        {
            return Err(ManagedGitError::UnsafeArchivePath);
        }
        safe.push(part);
    }
    Ok(safe)
}

fn validate_provider(root: &Path) -> Result<(), ManagedGitError> {
    let executable = git_executable(root);
    if !executable.is_file() {
        return Err(ManagedGitError::InvalidPayload("expected Git executable is absent"));
    }
    run_git_health_check(root, &executable)
}

fn run_git_health_check(root: &Path, executable: &Path) -> Result<(), ManagedGitError> {
    let mut command = Command::new(executable);
    command.arg("--version").stdin(Stdio::null());
    let mut paths = vec![executable.parent().unwrap_or(root).to_path_buf()];
    #[cfg(windows)]
    paths.extend([root.join("ucrt64/bin"), root.join("mingw64/bin"), root.join("usr/bin"), root.join("cmd")]);
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    command.env("PATH", env::join_paths(paths).map_err(|_| ManagedGitError::InvalidPath)?);
    if let Some(exec_path) = [root.join("libexec/git-core"), root.join("ucrt64/libexec/git-core"), root.join("mingw64/libexec/git-core")]
        .into_iter().find(|path| path.is_dir())
    {
        command.env("GIT_EXEC_PATH", exec_path);
    }
    if let Some(template_path) = [root.join("share/git-core/templates"), root.join("ucrt64/share/git-core/templates"), root.join("mingw64/share/git-core/templates")]
        .into_iter().find(|path| path.is_dir())
    {
        command.env("GIT_TEMPLATE_DIR", template_path);
    }
    let output = command.output().map_err(ManagedGitError::NetworkIo)?;
    if !output.status.success() {
        return Err(ManagedGitError::GitHealthCheck(String::from_utf8_lossy(&output.stderr).trim().to_owned()));
    }
    Ok(())
}

fn activate(payload: &Path, target: &Path, tools_root: &Path) -> Result<(), ManagedGitError> {
    let suffix = NEXT_STAGE.fetch_add(1, Ordering::Relaxed);
    let backup = tools_root.join(format!(".git-backup-{}-{suffix}", std::process::id()));
    let had_previous = target.exists();
    if had_previous {
        fs::rename(target, &backup).map_err(|source| io_error("stage previous managed Git", target, source))?;
    }
    if let Err(source) = fs::rename(payload, target) {
        if had_previous { let _ = fs::rename(&backup, target); }
        return Err(io_error("activate managed Git", target, source));
    }
    let executable = git_executable(target);
    if let Err(error) = run_git_health_check(target, &executable) {
        let _ = fs::remove_dir_all(target);
        if had_previous { let _ = fs::rename(&backup, target); }
        return Err(error);
    }
    if had_previous { let _ = fs::remove_dir_all(&backup); }
    Ok(())
}

fn git_executable(root: &Path) -> PathBuf {
    if cfg!(windows) { root.join("cmd").join("git.exe") } else { root.join("bin").join("git") }
}

fn provider_version(provider: &git_provider::GitProvider) -> Result<String, ManagedGitError> {
    let output = provider
        .run_output(["--version"])
        .map_err(ManagedGitError::NetworkIo)?;
    if !output.status.success() {
        return Err(ManagedGitError::GitHealthCheck(String::from_utf8_lossy(&output.stderr).trim().to_owned()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let raw = text.trim().strip_prefix("git version ").ok_or(ManagedGitError::InvalidGitVersion)?;
    let version = raw.split_whitespace().next().unwrap_or(raw);
    if version.is_empty() || !version.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')) {
        return Err(ManagedGitError::InvalidGitVersion);
    }
    Ok(version.to_owned())
}

fn ensure_managed_directory(root: &Path, path: &Path) -> Result<(), ManagedGitError> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ManagedGitError::UnsafeManagedPath(path.to_path_buf()));
        }
        return Ok(());
    }
    fs::create_dir_all(path).map_err(|source| io_error("create Vapor-managed tools directory", path, source))?;
    let canonical_root = fs::canonicalize(root).map_err(|source| io_error("resolve Vapor User Data", root, source))?;
    let canonical_path = fs::canonicalize(path).map_err(|source| io_error("resolve Vapor tools directory", path, source))?;
    if !canonical_path.starts_with(canonical_root) {
        return Err(ManagedGitError::UnsafeManagedPath(path.to_path_buf()));
    }
    Ok(())
}

fn ensure_safe_existing_directory(path: &Path) -> Result<(), ManagedGitError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => Err(ManagedGitError::UnsafeManagedPath(path.to_path_buf())),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error("inspect managed Git path", path, source)),
    }
}

struct StageDirectory {
    path: PathBuf,
}

impl StageDirectory {
    fn create(parent: &Path) -> Result<Self, ManagedGitError> {
        for _ in 0..8 {
            let sequence = NEXT_STAGE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".git-stage-{}-{sequence}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(io_error("create managed Git staging directory", &path, source)),
            }
        }
        Err(ManagedGitError::StageCollision)
    }

}

impl Drop for StageDirectory {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.path); }
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> ManagedGitError {
    ManagedGitError::Io { operation, path: path.to_path_buf(), source }
}

#[derive(Debug)]
pub enum ManagedGitError {
    UnsupportedPlatform,
    Provider(String),
    ReleaseHasNoGit,
    ReleaseVerification(crate::release::ReleaseVerificationError),
    Network(reqwest::Error),
    NetworkIo(io::Error),
    Archive(io::Error),
    Io { operation: &'static str, path: PathBuf, source: io::Error },
    HttpStatus { url: String, status: u16 },
    UntrustedOrigin,
    MetadataTooLarge,
    InvalidSignatureEncoding,
    InvalidPayload(&'static str),
    ArchiveLimit,
    UnsupportedArchiveEntry,
    UnsafeArchivePath,
    DuplicateArchivePath,
    EmptyArchive,
    UnsafeArchiveLink,
    InvalidPath,
    GitHealthCheck(String),
    InvalidGitVersion,
    UnsafeManagedPath(PathBuf),
    StageCollision,
}

impl std::fmt::Display for ManagedGitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform => f.write_str("Vapor-managed Git is unavailable for this platform"),
            Self::Provider(message) => write!(f, "selected Git provider is unavailable: {message}"),
            Self::ReleaseHasNoGit => f.write_str("the signed stable Vapor release does not include a distributable Git payload"),
            Self::ReleaseVerification(error) => write!(f, "signed Git release was rejected: {error}"),
            Self::Network(error) => write!(f, "Vapor Git download failed: {error}"),
            Self::NetworkIo(error) => write!(f, "Vapor Git executable could not run: {error}"),
            Self::Archive(error) => write!(f, "signed Git archive could not be read: {error}"),
            Self::Io { operation, path, source } => write!(f, "could not {operation} '{}': {source}", path.display()),
            Self::HttpStatus { url, status } => write!(f, "Vapor release host returned HTTP {status} for {url}"),
            Self::UntrustedOrigin => f.write_str("Vapor release request resolved outside the pinned HTTPS origin"),
            Self::MetadataTooLarge => f.write_str("Vapor Git release metadata exceeded its size limit"),
            Self::InvalidSignatureEncoding => f.write_str("Vapor returned an invalid signed release signature encoding"),
            Self::InvalidPayload(message) => write!(f, "signed Git payload is invalid: {message}"),
            Self::ArchiveLimit => f.write_str("signed Git archive exceeded its entry or expanded-size limit"),
            Self::UnsupportedArchiveEntry => f.write_str("signed Git archive contains a link or unsupported entry type"),
            Self::UnsafeArchivePath => f.write_str("signed Git archive contains an unsafe or non-portable path"),
            Self::DuplicateArchivePath => f.write_str("signed Git archive contains duplicate or case-colliding paths"),
            Self::EmptyArchive => f.write_str("signed Git archive contains no files"),
            Self::UnsafeArchiveLink => f.write_str("signed Git archive contains an unsafe or unresolved hard link"),
            Self::InvalidPath => f.write_str("could not prepare a portable Git runtime path"),
            Self::GitHealthCheck(message) => write!(f, "staged Vapor Git failed its runtime check: {message}"),
            Self::InvalidGitVersion => f.write_str("Git returned an unrecognized version string"),
            Self::UnsafeManagedPath(path) => write!(f, "refusing to replace a non-directory or linked Vapor Git path: {}", path.display()),
            Self::StageCollision => f.write_str("could not allocate a unique managed Git staging directory"),
        }
    }
}

impl std::error::Error for ManagedGitError {}
