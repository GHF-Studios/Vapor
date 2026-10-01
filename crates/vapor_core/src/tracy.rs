//! Managed Tracy profiler integration.
//!
//! Tracy's wire protocol is version-sensitive. This provider therefore pins an
//! explicit compatible profiler version; it never floats to "latest".
//!
//! Vapor prefers a verified GHF-built artifact. On Linux, if that artifact has
//! not been published yet, Vapor may perform one disposable source build into
//! Installation-owned tool state. The source/build tree is removed afterwards.

use crate::{
    DisplayServer, HostCapability, HostEnvironment, HostOperatingSystem, HostTarget,
    ManagedToolError, ManagedToolRequest, ReleaseAssetStatus,
    download_verified_github_release_asset, find_executable, find_executable_on_path,
    github_release_client,
};
use reqwest::blocking::Client;
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const TRACY_PROFILER_VERSION: &str = "0.14.0";

const TRACY_UPSTREAM_COMMIT: &str = "099df3de3dc37eca4712c06b8320fb9c53596edd";
const TRACY_SOURCE_ARCHIVE: &str =
    "https://github.com/wolfpld/tracy/archive/099df3de3dc37eca4712c06b8320fb9c53596edd.tar.gz";

const MANAGED_RELEASE_REPOSITORY: &str = "GHF-Studios/Vapor";
const MANAGED_RELEASE_TAG_PREFIX: &str = "managed-tracy-v";

const TRACY_PATH_ENV: &str = "VAPOR_TRACY_PATH";
const TRACY_PATH_FILE: &str = "tracy-profiler-path";

const TRACY_CSVEXPORT_PATH_ENV: &str = "VAPOR_TRACY_CSVEXPORT_PATH";
const TRACY_ANALYSIS_DIR: &str = "tools/tracy-analysis";

const TRACY_CSVEXPORT_NAMES: &[&str] = &[
    "tracy-csvexport",
    "tracy-csvexport.exe",
];

const TRACY_EXECUTABLE_NAMES: &[&str] = &[
    "tracy-profiler",
    "tracy-profiler.exe",
    "Tracy",
    "Tracy.exe",
    "Tracy-release",
    "Tracy-release.exe",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TracyProfilerOrigin {
    ExplicitOverride,
    EnvironmentOverride,
    RememberedOverride,
    ManagedCache,
    SystemPath,
    ManagedDownload,
    ManagedSourceBuild,
}

impl fmt::Display for TracyProfilerOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ExplicitOverride => "explicit override",
            Self::EnvironmentOverride => "environment override",
            Self::RememberedOverride => "remembered override",
            Self::ManagedCache => "managed cache",
            Self::SystemPath => "system PATH",
            Self::ManagedDownload => "managed download",
            Self::ManagedSourceBuild => "managed source build",
        })
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedTracyProfiler {
    pub executable: PathBuf,
    pub version: &'static str,
    pub origin: TracyProfilerOrigin,
}

pub fn resolve_tracy_profiler(
    explicit: Option<&Path>,
    user_data_root: &Path,
) -> Result<ResolvedTracyProfiler, TracyError> {
    let executable_names = tracy_executable_names();

    if let Some(path) = explicit {
        let executable = find_executable(path, &executable_names).ok_or_else(|| {
            TracyError::message(format!(
                "no Tracy profiler executable found at `{}`",
                path.display()
            ))
        })?;

        return Ok(resolved(executable, TracyProfilerOrigin::ExplicitOverride));
    }

    if let Some(path) = env::var_os(TRACY_PATH_ENV) {
        let path = PathBuf::from(path);
        if let Some(executable) = find_executable(&path, &executable_names) {
            return Ok(resolved(
                executable,
                TracyProfilerOrigin::EnvironmentOverride,
            ));
        }
    }

    let remembered = user_data_root
        .join("development")
        .join(TRACY_PATH_FILE);

    if let Ok(path) = fs::read_to_string(&remembered) {
        let path = PathBuf::from(path.trim());
        if let Some(executable) = find_executable(&path, &executable_names) {
            return Ok(resolved(
                executable,
                TracyProfilerOrigin::RememberedOverride,
            ));
        }
    }

    let environment = HostEnvironment::current();

    if let Ok(environment) = &environment {
        if let Ok(request) = tracy_request(environment) {
            let root = request.install_root(user_data_root, environment);
            if root.join("LICENSE.tracy").is_file() {
                if let Some(executable) = request.installed_executable(&root) {
                    return Ok(resolved(executable, TracyProfilerOrigin::ManagedCache));
                }
            }
        }
    }

    if let Some(executable) = find_executable_on_path(&executable_names) {
        return Ok(resolved(executable, TracyProfilerOrigin::SystemPath));
    }

    let environment = environment.map_err(|error| TracyError::message(error.to_string()))?;
    let request = tracy_request(&environment)?;

    acquire_managed_tracy(user_data_root, &environment, &request)
}

/// Resolve Tracy's CSV exporter without acquiring or building anything.
///
/// Game launch must never trigger profiler-tool construction. Saved-trace
/// analysis is a separate explicit workflow.
pub fn resolve_tracy_csvexport(
    user_data_root: &Path,
) -> Result<PathBuf, TracyError> {
    if let Some(path) = env::var_os(TRACY_CSVEXPORT_PATH_ENV) {
        let path = PathBuf::from(path);
        if let Some(executable) = find_executable(&path, &tracy_csvexport_names()) {
            return Ok(executable);
        }
    }

    if let Some(path) = env::var_os(TRACY_PATH_ENV) {
        let path = PathBuf::from(path);
        if let Some(parent) = profiler_parent_candidate(&path)
            && let Some(executable) = find_executable(&parent, &tracy_csvexport_names())
        {
            return Ok(executable);
        }
    }

    let remembered = user_data_root.join("development").join(TRACY_PATH_FILE);
    if let Ok(path) = fs::read_to_string(&remembered) {
        let path = PathBuf::from(path.trim());
        if let Some(parent) = profiler_parent_candidate(&path)
            && let Some(executable) = find_executable(&parent, &tracy_csvexport_names())
        {
            return Ok(executable);
        }
    }

    if let Some(executable) = find_executable_on_path(&tracy_csvexport_names()) {
        return Ok(executable);
    }

    if let Ok(environment) = HostEnvironment::current() {
        let managed = tracy_analysis_root(user_data_root, &environment)
            .join(csvexport_executable_name());
        if managed.is_file() {
            return Ok(managed);
        }
    }

    Err(TracyError::message(
        "Tracy saved-trace analysis needs `tracy-csvexport`, but Vapor did not find it. \
         This does not affect `vapor run --profiling`. Install the analysis helper \
         explicitly with `vapor profile setup`."
            .to_owned(),
    ))
}

/// Explicitly install only the saved-trace analysis helper.
///
/// This function is intentionally never called from `vapor run`.
pub fn install_tracy_csvexport(
    user_data_root: &Path,
) -> Result<PathBuf, TracyError> {
    if let Ok(existing) = resolve_tracy_csvexport(user_data_root) {
        return Ok(existing);
    }

    let environment =
        HostEnvironment::current().map_err(|error| TracyError::message(error.to_string()))?;

    if environment.target != HostTarget::LINUX_X86_64_GNU {
        return Err(TracyError::message(format!(
            "automatic tracy-csvexport installation is not yet implemented for {}; \
             install Tracy {} csvexport and put it on PATH or set {}",
            environment.target.triple(),
            TRACY_PROFILER_VERSION,
            TRACY_CSVEXPORT_PATH_ENV,
        )));
    }

    let destination_root = tracy_analysis_root(user_data_root, &environment);
    fs::create_dir_all(&destination_root)
        .map_err(|source| TracyError::io(destination_root.clone(), source))?;
    let destination = destination_root.join(csvexport_executable_name());

    let staging_root = user_data_root
        .join("tools")
        .join(".staging")
        .join(format!(
            "tracy-csvexport-{}-{}",
            TRACY_PROFILER_VERSION,
            std::process::id()
        ));

    if staging_root.exists() {
        fs::remove_dir_all(&staging_root)
            .map_err(|source| TracyError::io(staging_root.clone(), source))?;
    }
    fs::create_dir_all(&staging_root)
        .map_err(|source| TracyError::io(staging_root.clone(), source))?;

    let client = github_release_client().map_err(TracyError::managed)?;
    let result = (|| {
        ensure_command_available("tar")?;
        ensure_command_available("cmake")?;

        let archive_path = staging_root.join("tracy-source.tar.gz");
        let unpack_root = staging_root.join("source");
        let build_root = staging_root.join("build");
        fs::create_dir_all(&unpack_root)
            .map_err(|source| TracyError::io(unpack_root.clone(), source))?;

        let bytes = client
            .get(TRACY_SOURCE_ARCHIVE)
            .send()
            .map_err(TracyError::http)?
            .error_for_status()
            .map_err(TracyError::http)?
            .bytes()
            .map_err(TracyError::http)?;
        fs::write(&archive_path, &bytes)
            .map_err(|source| TracyError::io(archive_path.clone(), source))?;

        run_command(
            Command::new("tar")
                .arg("-xzf")
                .arg(&archive_path)
                .arg("-C")
                .arg(&unpack_root),
            "extract Tracy source archive",
        )?;

        let source_root = fs::read_dir(&unpack_root)
            .map_err(|source| TracyError::io(unpack_root.clone(), source))?
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.join("csvexport/CMakeLists.txt").is_file())
            .ok_or_else(|| {
                TracyError::message(
                    "downloaded Tracy source has no csvexport/CMakeLists.txt".to_owned(),
                )
            })?;

        run_command(
            Command::new("cmake")
                .arg("-B")
                .arg(&build_root)
                .arg("-S")
                .arg(source_root.join("csvexport"))
                .arg("-DCMAKE_BUILD_TYPE=Release")
                .arg(format!("-DGIT_REV={TRACY_UPSTREAM_COMMIT}")),
            "configure Tracy csvexport",
        )?;

        run_command(
            Command::new("cmake")
                .arg("--build")
                .arg(&build_root)
                .arg("--parallel"),
            "build Tracy csvexport",
        )?;

        let built = find_named_executable_recursive(
            &build_root,
            TRACY_CSVEXPORT_NAMES,
            0,
        )
        .ok_or_else(|| {
            TracyError::message(format!(
                "Tracy csvexport build completed but no executable was found under `{}`",
                build_root.display()
            ))
        })?;

        let temporary = destination.with_extension("partial");
        fs::copy(&built, &temporary)
            .map_err(|source| TracyError::io(temporary.clone(), source))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&temporary)
                .map_err(|source| TracyError::io(temporary.clone(), source))?
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&temporary, permissions)
                .map_err(|source| TracyError::io(temporary.clone(), source))?;
        }

        if destination.exists() {
            fs::remove_file(&destination)
                .map_err(|source| TracyError::io(destination.clone(), source))?;
        }
        fs::rename(&temporary, &destination)
            .map_err(|source| TracyError::io(destination.clone(), source))?;

        Ok(destination.clone())
    })();

    let _ = fs::remove_dir_all(&staging_root);
    result
}

fn profiler_parent_candidate(path: &Path) -> Option<PathBuf> {
    if path.is_dir() {
        Some(path.to_path_buf())
    } else {
        path.parent().map(Path::to_path_buf)
    }
}

fn tracy_csvexport_names() -> Vec<String> {
    TRACY_CSVEXPORT_NAMES
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

fn tracy_analysis_root(
    user_data_root: &Path,
    environment: &HostEnvironment,
) -> PathBuf {
    user_data_root
        .join(TRACY_ANALYSIS_DIR)
        .join(TRACY_PROFILER_VERSION)
        .join(environment.target.triple())
}

fn csvexport_executable_name() -> &'static str {
    if cfg!(windows) {
        "tracy-csvexport.exe"
    } else {
        "tracy-csvexport"
    }
}

fn find_named_executable_recursive(
    root: &Path,
    names: &[&str],
    depth: usize,
) -> Option<PathBuf> {
    if depth > 4 {
        return None;
    }

    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.is_file() {
            let name = path.file_name()?.to_str()?;
            if names.contains(&name) {
                return Some(path);
            }
        } else if path.is_dir()
            && let Some(found) = find_named_executable_recursive(&path, names, depth + 1)
        {
            return Some(found);
        }
    }

    None
}

pub fn remember_tracy_profiler(
    user_data_root: &Path,
    tracy: &Path,
) -> Result<(), TracyError> {
    let directory = user_data_root.join("development");
    fs::create_dir_all(&directory).map_err(|source| TracyError::io(directory.clone(), source))?;

    let path = directory.join(TRACY_PATH_FILE);
    fs::write(&path, tracy.to_string_lossy().as_bytes())
        .map_err(|source| TracyError::io(path, source))
}

fn resolved(
    executable: PathBuf,
    origin: TracyProfilerOrigin,
) -> ResolvedTracyProfiler {
    ResolvedTracyProfiler {
        executable,
        version: TRACY_PROFILER_VERSION,
        origin,
    }
}

fn tracy_executable_names() -> Vec<String> {
    TRACY_EXECUTABLE_NAMES
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

fn tracy_request(environment: &HostEnvironment) -> Result<ManagedToolRequest, TracyError> {
    let required_capabilities = match environment.target.operating_system {
        HostOperatingSystem::Linux => {
            let display = environment.display_server().ok_or_else(|| {
                TracyError::message(
                    "Vapor cannot choose a Linux Tracy artifact because no X11/Wayland \
                     display capability was detected; use `--tracy PATH` to override"
                        .to_owned(),
                )
            })?;
            vec![HostCapability::Display(display)]
        }
        HostOperatingSystem::Windows => Vec::new(),
    };

    let request = ManagedToolRequest::new(
        "tracy",
        TRACY_PROFILER_VERSION,
        tracy_executable_names(),
        required_capabilities,
    );

    if !request.matches_environment(environment) {
        return Err(TracyError::message(format!(
            "the current host environment does not satisfy Tracy's required capabilities: {}",
            request.environment_key()
        )));
    }

    Ok(request)
}

fn acquire_managed_tracy(
    user_data_root: &Path,
    environment: &HostEnvironment,
    request: &ManagedToolRequest,
) -> Result<ResolvedTracyProfiler, TracyError> {
    let root = request.install_root(user_data_root, environment);
    fs::create_dir_all(&root).map_err(|source| TracyError::io(root.clone(), source))?;

    let variant = tracy_variant(environment)?;
    let executable_name = match environment.target.operating_system {
        HostOperatingSystem::Windows => "tracy-profiler.exe",
        HostOperatingSystem::Linux => "tracy-profiler",
    };

    let binary_asset = format!(
        "tracy-profiler-{}-{}-{}{}",
        TRACY_PROFILER_VERSION,
        environment.target.triple(),
        variant,
        if environment.target.operating_system == HostOperatingSystem::Windows {
            ".exe"
        } else {
            ""
        }
    );

    let license_asset = format!("tracy-license-{TRACY_PROFILER_VERSION}.txt");
    let release_tag = format!("{MANAGED_RELEASE_TAG_PREFIX}{TRACY_PROFILER_VERSION}");
    let client = github_release_client().map_err(TracyError::managed)?;

    let license_destination = root.join("LICENSE.tracy");
    let executable_destination = root.join(executable_name);

    match download_verified_github_release_asset(
        &client,
        MANAGED_RELEASE_REPOSITORY,
        &release_tag,
        &license_asset,
        &license_destination,
        false,
    )
    .map_err(TracyError::managed)?
    {
        ReleaseAssetStatus::Downloaded => {
            match download_verified_github_release_asset(
                &client,
                MANAGED_RELEASE_REPOSITORY,
                &release_tag,
                &binary_asset,
                &executable_destination,
                true,
            )
            .map_err(TracyError::managed)?
            {
                ReleaseAssetStatus::Downloaded => {
                    return Ok(resolved(
                        executable_destination,
                        TracyProfilerOrigin::ManagedDownload,
                    ));
                }
                ReleaseAssetStatus::Missing => {}
            }
        }
        ReleaseAssetStatus::Missing => {}
    }

    if environment.target != HostTarget::LINUX_X86_64_GNU {
        return Err(TracyError::message(format!(
            "no managed Tracy {} artifact is published for {} / {}; \
             open Tracy explicitly from `vapor profile` with an explicit profiler override until that target has a proven artifact",
            TRACY_PROFILER_VERSION,
            environment.target.triple(),
            variant
        )));
    }

    build_tracy_from_source(
        &client,
        user_data_root,
        &root,
        environment.display_server().ok_or_else(|| {
            TracyError::message("Linux Tracy source build requires X11 or Wayland".to_owned())
        })?,
    )?;

    Ok(resolved(
        executable_destination,
        TracyProfilerOrigin::ManagedSourceBuild,
    ))
}

fn tracy_variant(environment: &HostEnvironment) -> Result<&'static str, TracyError> {
    match environment.target.operating_system {
        HostOperatingSystem::Windows => Ok("native"),
        HostOperatingSystem::Linux => match environment.display_server() {
            Some(DisplayServer::X11) => Ok("x11"),
            Some(DisplayServer::Wayland) => Ok("wayland"),
            None => Err(TracyError::message(
                "Linux Tracy requires a detected X11 or Wayland display".to_owned(),
            )),
        },
    }
}

fn build_tracy_from_source(
    client: &Client,
    user_data_root: &Path,
    destination_root: &Path,
    display: DisplayServer,
) -> Result<(), TracyError> {
    let staging_root = user_data_root
        .join("tools")
        .join(".staging")
        .join(format!(
            "tracy-{}-{}",
            TRACY_PROFILER_VERSION,
            std::process::id()
        ));

    if staging_root.exists() {
        fs::remove_dir_all(&staging_root)
            .map_err(|source| TracyError::io(staging_root.clone(), source))?;
    }

    fs::create_dir_all(&staging_root)
        .map_err(|source| TracyError::io(staging_root.clone(), source))?;

    let result = (|| {
        ensure_command_available("tar")?;
        ensure_command_available("cmake")?;

        let archive_path = staging_root.join("tracy-source.tar.gz");
        let unpack_root = staging_root.join("source");
        let build_root = staging_root.join("build");

        fs::create_dir_all(&unpack_root)
            .map_err(|source| TracyError::io(unpack_root.clone(), source))?;

        let bytes = client
            .get(TRACY_SOURCE_ARCHIVE)
            .send()
            .map_err(TracyError::http)?
            .error_for_status()
            .map_err(TracyError::http)?
            .bytes()
            .map_err(TracyError::http)?;

        fs::write(&archive_path, &bytes)
            .map_err(|source| TracyError::io(archive_path.clone(), source))?;

        run_command(
            Command::new("tar")
                .arg("-xzf")
                .arg(&archive_path)
                .arg("-C")
                .arg(&unpack_root),
            "extract Tracy source archive",
        )?;

        let source_root = fs::read_dir(&unpack_root)
            .map_err(|source| TracyError::io(unpack_root.clone(), source))?
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.join("profiler/CMakeLists.txt").is_file())
            .ok_or_else(|| {
                TracyError::message(
                    "downloaded Tracy source archive has no profiler/CMakeLists.txt".to_owned(),
                )
            })?;

        let mut configure = Command::new("cmake");
        configure
            .arg("-B")
            .arg(&build_root)
            .arg("-S")
            .arg(source_root.join("profiler"))
            .arg("-DCMAKE_BUILD_TYPE=Release")
            .arg(format!("-DGIT_REV={TRACY_UPSTREAM_COMMIT}"));

        match display {
            DisplayServer::X11 => {
                configure.arg("-DLEGACY=ON");
            }
            DisplayServer::Wayland => {
                configure.arg("-DLEGACY=OFF");
            }
        }

        run_command(&mut configure, "configure Tracy profiler")?;

        run_command(
            Command::new("cmake")
                .arg("--build")
                .arg(&build_root)
                .arg("--parallel"),
            "build Tracy profiler",
        )?;

        let built = find_built_profiler(&build_root).ok_or_else(|| {
            TracyError::message(format!(
                "Tracy build completed but no profiler executable was found under `{}`",
                build_root.display()
            ))
        })?;

        fs::create_dir_all(destination_root)
            .map_err(|source| TracyError::io(destination_root.to_path_buf(), source))?;

        let license_source = source_root.join("LICENSE");
        let license_destination = destination_root.join("LICENSE.tracy");
        fs::copy(&license_source, &license_destination)
            .map_err(|source| TracyError::io(license_destination.clone(), source))?;

        let executable_destination = destination_root.join("tracy-profiler");
        let temporary_executable = destination_root.join(".tracy-profiler.partial");

        fs::copy(&built, &temporary_executable)
            .map_err(|source| TracyError::io(temporary_executable.clone(), source))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&temporary_executable)
                .map_err(|source| TracyError::io(temporary_executable.clone(), source))?
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&temporary_executable, permissions)
                .map_err(|source| TracyError::io(temporary_executable.clone(), source))?;
        }

        if executable_destination.exists() {
            fs::remove_file(&executable_destination)
                .map_err(|source| TracyError::io(executable_destination.clone(), source))?;
        }

        fs::rename(&temporary_executable, &executable_destination)
            .map_err(|source| TracyError::io(executable_destination, source))?;

        Ok(())
    })();

    let _ = fs::remove_dir_all(&staging_root);
    result
}

fn ensure_command_available(command: &'static str) -> Result<(), TracyError> {
    Command::new(command)
        .arg("--version")
        .output()
        .map(|_| ())
        .map_err(|source| {
            TracyError::message(format!(
                "managed Tracy source fallback needs `{command}` but it could not be started: {source}"
            ))
        })
}

fn run_command(command: &mut Command, description: &'static str) -> Result<(), TracyError> {
    let status = command.status().map_err(|source| {
        TracyError::message(format!("failed to {description}: {source}"))
    })?;

    if status.success() {
        Ok(())
    } else {
        Err(TracyError::message(format!(
            "failed to {description}: process exited with {status}"
        )))
    }
}

fn find_built_profiler(root: &Path) -> Option<PathBuf> {
    let direct = [
        root.join("tracy-profiler"),
        root.join("tracy-profiler.exe"),
        root.join("Release/tracy-profiler.exe"),
    ];

    if let Some(found) = direct.into_iter().find(|path| path.is_file()) {
        return Some(found);
    }

    find_built_profiler_recursive(root, 0)
}

fn find_built_profiler_recursive(root: &Path, depth: usize) -> Option<PathBuf> {
    if depth > 3 {
        return None;
    }

    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.is_file() {
            let name = path.file_name()?.to_str()?;
            if name == "tracy-profiler" || name == "tracy-profiler.exe" {
                return Some(path);
            }
        } else if path.is_dir() {
            if let Some(found) = find_built_profiler_recursive(&path, depth + 1) {
                return Some(found);
            }
        }
    }

    None
}

#[derive(Debug)]
pub struct TracyError {
    message: String,
    managed: Option<ManagedToolError>,
    io: Option<io::Error>,
    http: Option<reqwest::Error>,
}

impl TracyError {
    fn message(message: String) -> Self {
        Self {
            message,
            managed: None,
            io: None,
            http: None,
        }
    }

    fn managed(error: ManagedToolError) -> Self {
        Self {
            message: error.to_string(),
            managed: Some(error),
            io: None,
            http: None,
        }
    }

    fn io(path: PathBuf, source: io::Error) -> Self {
        let message = format!("failed to access `{}`: {source}", path.display());
        Self {
            message,
            managed: None,
            io: Some(source),
            http: None,
        }
    }

    fn http(error: reqwest::Error) -> Self {
        Self {
            message: format!("failed to acquire Tracy source: {error}"),
            managed: None,
            io: None,
            http: Some(error),
        }
    }
}

impl fmt::Display for TracyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TracyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let Some(error) = &self.managed {
            return Some(error);
        }
        if let Some(error) = &self.io {
            return Some(error);
        }
        if let Some(error) = &self.http {
            return Some(error);
        }
        None
    }
}
