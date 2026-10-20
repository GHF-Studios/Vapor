//! Discovery of the Steam installation/app root and external source root.
//!
//! # Two-root model
//!
//! Vapor deliberately separates app-owned machinery from authored source:
//!
//! - [`InstallationPaths`](crate::discovery::InstallationPaths) is anchored to the running executable. It contains
//!   bundled setup tools, binaries, libraries, and installed content.
//! - [`SourceWorkspace`](crate::discovery::SourceWorkspace) is anchored to an explicitly selected external source
//!   path. It contains critical authored source and must be outside the
//!   installation.
//!
//! [`EnvironmentPaths`](crate::discovery::EnvironmentPaths) pairs both roots and rejects overlapping layouts.
//! User navigation is confined to the source root; installation paths are
//! resources that commands may inspect or execute explicitly.

use crate::manifest::{self, VaporEntity};
use std::{
    env, fs,
    path::{Path, PathBuf},
};
use vapor_core::{ManagedToolchain, VaporInstallation};

/// Conventional installation directory containing runtime libraries.
pub const LIBRARY_DIR: &str = "lib";

/// Both independent roots required by one local Vapor session.
#[derive(Debug, Clone)]
pub struct EnvironmentPaths {
    installation: InstallationPaths,
    source: SourceWorkspace,
}

impl EnvironmentPaths {
    /// Discover only the Steam installation/app root from the running binary.
    ///
    /// # Errors
    ///
    /// Fails when the executable is not laid out as an installed Vapor app.
    pub fn discover_installation() -> Result<InstallationPaths, String> {
        let executable = env::current_exe()
            .map_err(|error| format!("failed to locate the running Vapor application: {error}"))?;
        InstallationPaths::from_executable(&executable)
    }

    /// Discover the installation from the executable and source from the caller.
    ///
    /// # Errors
    ///
    /// Fails when either root lacks a valid workspace marker or when the source
    /// workspace overlaps the Steam installation/app root.
    pub fn discover() -> Result<Self, String> {
        let installation = Self::discover_installation()?;
        let source = if let Some(configured) = configured_source(&installation)? {
            SourceWorkspace::from_source_path(&configured, &installation)?
        } else {
            let invocation = env::current_dir()
                .map_err(|error| format!("failed to read the invocation directory: {error}"))?;
            SourceWorkspace::from_invocation(&invocation, &installation)?
        };
        Ok(Self {
            installation,
            source,
        })
    }

    /// Discover both roots from explicit paths.
    ///
    /// This is useful to launch Vapor from another host process and to test an
    /// installation layout without changing process-global state.
    ///
    /// # Errors
    ///
    /// Returns the same validation errors as [`Self::discover`].
    pub fn from_paths(executable: &Path, invocation: &Path) -> Result<Self, String> {
        let installation = InstallationPaths::from_executable(executable)?;
        Self::from_installation_and_invocation(installation, invocation)
    }

    /// Build an active environment from an already-discovered installation and
    /// selected source invocation path.
    ///
    /// # Errors
    ///
    /// Fails when the selected source is invalid or overlaps the installation.
    pub fn from_installation_and_invocation(
        installation: InstallationPaths,
        invocation: &Path,
    ) -> Result<Self, String> {
        let source = SourceWorkspace::from_invocation(invocation, &installation)?;
        Ok(Self {
            installation,
            source,
        })
    }

    /// Build an active environment from an explicitly selected source path.
    ///
    /// Unlike ambient invocation discovery, this honors an exact nested
    /// `[workspace]` or `[root]` marker at the selected path. That lets
    /// `source open /path/to/workspace` select an intentional nested workspace
    /// without changing the default super-root behavior for ordinary invocation
    /// from inside root-owned submodules.
    ///
    /// # Errors
    ///
    /// Fails when the selected source is invalid or overlaps the installation.
    pub fn from_installation_and_source_path(
        installation: InstallationPaths,
        source_path: &Path,
    ) -> Result<Self, String> {
        let source = SourceWorkspace::from_source_path(source_path, &installation)?;
        Ok(Self {
            installation,
            source,
        })
    }

    /// Steam installation/app-root paths.
    pub fn installation(&self) -> &InstallationPaths {
        &self.installation
    }

    /// External authored-source workspace.
    pub fn source(&self) -> &SourceWorkspace {
        &self.source
    }
}

fn configured_source(installation: &InstallationPaths) -> Result<Option<PathBuf>, String> {
    if let Some(path) = env::var_os("VAPOR_WORKSPACE").filter(|value| !value.is_empty()) {
        return Ok(Some(PathBuf::from(path)));
    }
    let state = installation.state_dir().join("source-workspace");
    if !state.is_file() {
        return Ok(None);
    }
    let value = fs::read_to_string(&state)
        .map_err(|error| format!("failed to read '{}': {error}", state.display()))?;
    let value = value.trim();
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PathBuf::from(value)))
    }
}

/// Paths owned by the Steam installation/app root.
#[derive(Debug, Clone)]
pub struct InstallationPaths {
    executable: PathBuf,
    root: PathBuf,
    binaries: PathBuf,
    libraries: Option<PathBuf>,
    cargo: Option<PathBuf>,
    identity_id: String,
}

impl InstallationPaths {
    /// Discover an installation from any packaged application executable that
    /// shares a directory with the app-owned `vapor` command.
    ///
    /// This lets GUI facades use the same installation-owned providers without
    /// guessing app-data locations or duplicating the Steam app-root model.
    pub fn from_application_executable(executable: &Path) -> Result<Self, String> {
        let executable = canonical_file(executable, "Vapor application executable")?;
        let command = executable.with_file_name(format!("vapor{}", env::consts::EXE_SUFFIX));
        Self::from_executable(&command)
    }

    /// Discover an installation from the canonical executable location.
    ///
    /// The executable must be laid out as `<app-root>/bin/vapor[.exe]` or
    /// `<app-root>/bin/<target>/vapor[.exe]`.
    /// `<app-root>/App.vapor.toml` must declare `[root]`.
    ///
    /// # Errors
    ///
    /// Fails for a missing executable, missing app-root marker, invalid marker,
    /// or a marker that describes anything except the app root.
    pub fn from_executable(executable: &Path) -> Result<Self, String> {
        let executable = canonical_file(executable, "Vapor application executable")?;
        let binaries = executable
            .parent()
            .ok_or_else(|| format!("executable has no parent: {}", executable.display()))?
            .to_path_buf();
        let expected_name = format!("vapor{}", env::consts::EXE_SUFFIX);
        let candidate_root = if binaries.file_name().is_some_and(|name| name == "bin") {
            binaries
                .parent()
                .ok_or_else(|| format!("binary directory has no parent: {}", binaries.display()))?
                .to_path_buf()
        } else if binaries
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "bin")
        {
            binaries
                .parent()
                .and_then(Path::parent)
                .ok_or_else(|| {
                    format!(
                        "target binary directory has no app-root parent: {}",
                        binaries.display()
                    )
                })?
                .to_path_buf()
        } else {
            binaries.clone()
        };
        let flat_command = candidate_root.join("bin").join(&expected_name);
        let target_command = binaries
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "bin")
            .then(|| binaries.join(&expected_name));
        let expected = executable == flat_command
            || target_command
                .as_ref()
                .is_some_and(|expected| executable == *expected);
        if !expected {
            return Err(format!(
                "the running executable is not laid out as an installed Vapor application\n  executable: {}\n  candidate app root: {}\n  expected command: {}\n  expected target command: {}\nnote: this usually means a source-built target/debug/vapor was run directly\nhelp: place the bootstrap application outside every source root and run a packaged Vapor command",
                executable.display(),
                candidate_root.display(),
                flat_command.display(),
                candidate_root
                    .join("bin")
                    .join("<target>")
                    .join(&expected_name)
                    .display()
            ));
        }
        let root = candidate_root;
        let marker = root.join(manifest::APP_FILE_NAME);
        if !marker.is_file() {
            return Err(format!(
                "the installed Vapor application is missing its root manifest\n  executable: {}\n  app root:   {}\n  expected:   {}\nhelp: install or bootstrap the app root with a [root] {} beside bin/",
                executable.display(),
                root.display(),
                marker.display(),
                manifest::APP_FILE_NAME
            ));
        }
        let identity_id = require_installation_marker(&marker, &root)?;

        let libraries = optional_directory(&root, LIBRARY_DIR)?;
        let cargo = bundled_cargo_candidates(&root)?
            .into_iter()
            .find(|candidate| candidate.is_file());

        Ok(Self {
            executable,
            root,
            binaries,
            libraries,
            cargo,
            identity_id,
        })
    }

    /// Canonical running executable.
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Steam installation/app-root machinery boundary.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory containing the running Vapor executable.
    pub fn binaries(&self) -> &Path {
        &self.binaries
    }

    /// App-local Vapor metadata directory.
    pub fn vapor_dir(&self) -> PathBuf {
        self.root.join(".vapor")
    }

    /// App-local mutable Vapor state directory.
    pub fn state_dir(&self) -> PathBuf {
        self.vapor_dir().join("state")
    }

    /// Conventional `lib` directory, when installed.
    pub fn libraries(&self) -> Option<&Path> {
        self.libraries.as_deref()
    }

    /// Bundled Cargo executable, when present in a supported installation path.
    pub fn cargo(&self) -> Option<&Path> {
        self.cargo.as_deref()
    }

    /// Rescan the installation for Cargo after script-managed tools change.
    pub fn bundled_cargo(&self) -> Option<PathBuf> {
        self.try_bundled_cargo().ok().flatten()
    }

    /// Rescan the app instance for Cargo and report an invalid installed pin.
    pub fn try_bundled_cargo(&self) -> Result<Option<PathBuf>, String> {
        bundled_cargo_candidates(&self.root)
            .map(|candidates| candidates.into_iter().find(|candidate| candidate.is_file()))
    }

    /// Stable identity declared by the installation marker.
    pub fn identity_id(&self) -> &str {
        &self.identity_id
    }
}

/// Critical authored source that is managed separately from the installation.
#[derive(Debug, Clone)]
pub struct SourceWorkspace {
    invocation: PathBuf,
    root: PathBuf,
    identity_id: String,
}

impl SourceWorkspace {
    /// Discover the highest source marker above `invocation`.
    ///
    /// # Errors
    ///
    /// Fails when the invocation directory is invalid, no marker exists, the
    /// highest marker is not a source root, or the source root overlaps the Steam
    /// installation.
    pub fn from_invocation(
        invocation: &Path,
        installation: &InstallationPaths,
    ) -> Result<Self, String> {
        let invocation = canonical_directory(invocation, "invocation directory")?;
        let marker = highest_marker(&invocation).ok_or_else(|| {
            format!(
                    "'{}' is not inside an external Vapor source root: no {} exists in any ancestor\nhelp: invoke Vapor from a source repository, not from the Steam app directory",
                invocation.display(),
                manifest::WORKSPACE_FILE_NAME
            )
        })?;
        Self::from_marker(invocation, marker, installation)
    }

    /// Discover a source root from an explicit source selection.
    ///
    /// If the selected directory itself contains a top-level `[workspace]` or
    /// `[root]` source marker, that directory is the source root. Otherwise this
    /// falls back to ambient invocation discovery so selecting a content child
    /// still resolves to its owning source root.
    ///
    /// # Errors
    ///
    /// Fails with the same diagnostics as [`Self::from_invocation`].
    pub fn from_source_path(
        source_path: &Path,
        installation: &InstallationPaths,
    ) -> Result<Self, String> {
        let invocation = canonical_directory(source_path, "source path")?;
        if let Some(marker) = exact_source_marker(&invocation)? {
            return Self::from_marker(invocation, marker, installation);
        }
        Self::from_invocation(&invocation, installation)
    }

    fn from_marker(
        invocation: PathBuf,
        marker: PathBuf,
        installation: &InstallationPaths,
    ) -> Result<Self, String> {
        let root = marker
            .parent()
            .expect("an ancestor marker always has a parent")
            .to_path_buf();
        let identity_id = require_source_marker(&marker, &root)?;

        if roots_overlap(&root, installation.root()) {
            if root == installation.root() {
                return Err(format!(
                    "no external source root is selected: invocation resolved the Steam installation/app root itself\n  app root: {}\nhelp: invoke Vapor from a separate source repository, or select a remembered source root in Vapor",
                    root.display()
                ));
            }
            return Err(format!(
                "the selected source root and Steam installation/app root are not disjoint\n  source root: {}\n  app root:    {}\nhelp: keep authored repositories outside the Steam installation/app root",
                root.display(),
                installation.root().display()
            ));
        }

        Ok(Self {
            invocation,
            root,
            identity_id,
        })
    }

    /// Directory from which Vapor was invoked.
    pub fn invocation(&self) -> &Path {
        &self.invocation
    }

    /// Highest external Vapor source root containing authored source.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Stable identity declared by the source marker.
    pub fn identity_id(&self) -> &str {
        &self.identity_id
    }

    /// Whether a canonical path belongs to this source workspace.
    pub fn contains(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
    }
}

/// Reject a canonical path outside the given boundary.
///
/// # Errors
///
/// Returns an error containing both paths when `candidate` is not below `root`.
pub fn ensure_contained(root: &Path, candidate: &Path) -> Result<(), String> {
    let root = shell_safe_path(root.to_path_buf());
    let candidate = shell_safe_path(candidate.to_path_buf());
    if candidate.starts_with(&root) {
        Ok(())
    } else {
        Err(format!(
            "Vapor source boundary violation: '{}' is outside '{}'",
            candidate.display(),
            root.display()
        ))
    }
}

fn highest_marker(start: &Path) -> Option<PathBuf> {
    start.ancestors().filter_map(source_marker_at).last()
}

fn exact_source_marker(start: &Path) -> Result<Option<PathBuf>, String> {
    let Some(marker) = source_marker_at(start) else {
        return Ok(None);
    };
    let source = fs::read_to_string(&marker)
        .map_err(|error| format!("failed to read '{}': {error}", marker.display()))?;
    if has_top_level_table(&source, "root") || has_top_level_table(&source, "workspace") {
        Ok(Some(marker))
    } else {
        Ok(None)
    }
}

fn source_marker_at(directory: &Path) -> Option<PathBuf> {
    [
        manifest::APP_SOURCE_FILE_NAME,
        manifest::WORKSPACE_FILE_NAME,
        manifest::REGISTRY_FILE_NAME,
    ]
    .into_iter()
    .map(|name| directory.join(name))
    .find(|marker| marker.is_file())
}

fn has_top_level_table(source: &str, expected: &str) -> bool {
    source.lines().any(|line| {
        let line = line.trim();
        let Some(table) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        else {
            return false;
        };
        !table.starts_with('[') && table.trim() == expected
    })
}

fn require_installation_marker(marker: &Path, root: &Path) -> Result<String, String> {
    match manifest::read(marker, root)? {
        VaporEntity::Root { id, .. } => Ok(id),
        VaporEntity::Registry { id, .. } => Err(format!(
            "highest Vapor manifest '{}' describes registry '{id}', not the Steam installation/app root; the installation must contain [root]",
            marker.display()
        )),
        VaporEntity::Workspace { id, .. } => Err(format!(
            "highest Vapor manifest '{}' describes workspace '{id}', not the Steam installation/app root; the installation must contain [root]",
            marker.display()
        )),
        VaporEntity::Content { kind, id, .. } => Err(format!(
            "highest Vapor manifest '{}' describes {kind} '{id}', not the Steam installation/app root; the installation must contain [root]",
            marker.display()
        )),
    }
}

fn require_source_marker(marker: &Path, root: &Path) -> Result<String, String> {
    match manifest::read(marker, root)? {
        VaporEntity::Root { id, .. } | VaporEntity::Workspace { id, .. } => Ok(id),
        VaporEntity::Registry { id, .. } => Err(format!(
            "highest Vapor manifest '{}' describes registry '{id}', not a source root; open a [root] or [workspace] repository",
            marker.display()
        )),
        VaporEntity::Content { kind, id, .. } => Err(format!(
            "highest Vapor manifest '{}' describes {kind} '{id}', not a source root; content must live inside a [workspace] repository",
            marker.display()
        )),
    }
}

fn roots_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn canonical_file(path: &Path, label: &str) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("failed to resolve {label} '{}': {error}", path.display()))?;
    let canonical = shell_safe_path(canonical);
    if canonical.is_file() {
        Ok(canonical)
    } else {
        Err(format!("{label} is not a file: {}", canonical.display()))
    }
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("failed to resolve {label} '{}': {error}", path.display()))?;
    let canonical = shell_safe_path(canonical);
    if canonical.is_dir() {
        Ok(canonical)
    } else {
        Err(format!(
            "{label} is not a directory: {}",
            canonical.display()
        ))
    }
}

#[cfg(windows)]
fn shell_safe_path(path: PathBuf) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    if let Some(path) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{path}"));
    }
    if let Some(path) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(path);
    }
    path
}

#[cfg(not(windows))]
fn shell_safe_path(path: PathBuf) -> PathBuf {
    path
}

fn optional_directory(root: &Path, name: &str) -> Result<Option<PathBuf>, String> {
    let candidate = root.join(name);
    if !candidate.exists() {
        return Ok(None);
    }
    let canonical = canonical_directory(&candidate, name)?;
    ensure_contained(root, &canonical)?;
    Ok(Some(canonical))
}

fn bundled_cargo_candidates(root: &Path) -> Result<Vec<PathBuf>, String> {
    let executable = format!("cargo{}", env::consts::EXE_SUFFIX);
    let toolchains_root = root.join("rustup-home").join("toolchains");
    let installation = VaporInstallation::from_explicit_root(root.to_path_buf())
        .map_err(|error| error.to_string())?;
    let pin =
        ManagedToolchain::installation_pin(&installation).map_err(|error| error.to_string())?;
    let mut toolchains = fs::read_dir(&toolchains_root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .collect::<Vec<_>>();

    if let Some(pin) = pin {
        let prefix = format!("{}-", pin.identifier());
        return Ok(toolchains
            .into_iter()
            .filter(|entry| {
                let name = entry.file_name();
                name == pin.identifier() || name.to_string_lossy().starts_with(&prefix)
            })
            .map(|entry| entry.path().join("bin").join(&executable))
            .collect());
    }

    toolchains.sort_by(|left, right| {
        let left_name = left.file_name().to_string_lossy().into_owned();
        let right_name = right.file_name().to_string_lossy().into_owned();
        toolchain_version_key(&right_name)
            .cmp(&toolchain_version_key(&left_name))
            .then_with(|| right_name.cmp(&left_name))
    });

    let mut candidates = toolchains
        .into_iter()
        .map(|entry| entry.path().join("bin").join(&executable))
        .collect::<Vec<_>>();
    candidates.push(root.join("cargo-home").join("bin").join(&executable));
    candidates.push(root.join("bin").join(executable));
    Ok(candidates)
}

fn toolchain_version_key(name: &str) -> Vec<u32> {
    name.split('-')
        .next()
        .unwrap_or(name)
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::bundled_cargo_candidates;
    use std::{env, fs, path::Path};

    #[test]
    fn installed_toolchain_metadata_selects_exact_cargo_independent_of_ambient_override() {
        let root = env::temp_dir().join(format!(
            "vapor-pinned-cargo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after epoch")
                .as_nanos()
        ));
        let toolchains = root.join("rustup-home/toolchains");
        let pinned = toolchains.join("1.97.0-x86_64-unknown-linux-gnu/bin/cargo");
        let ambient = toolchains.join("1.83.0-x86_64-unknown-linux-gnu/bin/cargo");
        fs::create_dir_all(pinned.parent().expect("pinned cargo parent"))
            .expect("create pinned toolchain");
        fs::create_dir_all(ambient.parent().expect("ambient cargo parent"))
            .expect("create other toolchain");
        fs::write(&pinned, "").expect("write pinned cargo marker");
        fs::write(&ambient, "").expect("write other cargo marker");
        let metadata = root.join("metadata/toolchain.toml");
        fs::create_dir_all(metadata.parent().expect("metadata parent"))
            .expect("create metadata directory");
        fs::write(
            &metadata,
            "schema = 1\nchannel = \"stable\"\nversion = \"1.97.0\"\ndate = \"2026-07-09\"\n",
        )
        .expect("write installed pin");
        fs::write(root.join("App.vapor.toml"), "[root]\n").expect("write app installation marker");

        let candidates = bundled_cargo_candidates(&root).expect("discover bundled Cargo");

        assert_eq!(candidates.first(), Some(&pinned));
        assert!(!candidates.iter().any(|candidate| candidate == &ambient));
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.starts_with(Path::new(&root)))
        );

        fs::remove_dir_all(root).expect("remove fixture");
    }
}
