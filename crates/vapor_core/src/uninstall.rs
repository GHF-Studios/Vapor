//! Vapor installation teardown and explicit purge operations.
//!
//! Ordinary uninstall removes machine integration only. Vapor-owned OS user
//! data and authored source survive unless explicitly purged.

use crate::installation::{
    InstallationError, VAPOR_HOME_ENV, VAPOR_HOME_MANIFEST_FILE_NAME, VaporInstallation,
};
use crate::source::{SourceError, source_state};
use crate::superworkspace::{
    SUPERWORKSPACE_MANIFEST_FILE_NAME, SuperworkspaceError, VaporSuperworkspace,
};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const PROFILE_BLOCK_START: &str = "# >>> Vapor managed PATH >>>";
const PROFILE_BLOCK_END: &str = "# <<< Vapor managed PATH <<<";
const MANAGED_SUPERWORKSPACE_REPOSITORIES: &[&str] =
    &["Loo-Cast", "Vapor-Client", "Vapor-Platform-Server"];
const LEGACY_STEAM_VAPOR_ENTRIES: &[&str] = &[
    ".vapor",
    "bin",
    "cargo-home",
    "content",
    "docs",
    "examples",
    "metadata",
    "output",
    "resources",
    "rustup",
    "rustup-home",
    "tools",
    "App.vapor.toml",
];

#[derive(Debug, Clone, Copy, Default)]
pub struct UninstallOptions {
    pub purge_user_data: bool,
    pub purge_superworkspace: bool,
    /// Remove all inventoried Vapor-owned roots, including managed source
    /// checkouts and old per-user data locations. The caller must preview and
    /// explicitly confirm the exact targets before setting this flag.
    pub full_nuke: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullNukePreview {
    /// Exact non-overlapping paths (files or directories) a full nuke deletes.
    pub paths: Vec<PathBuf>,
}

/// Return the exact existing Vapor-owned roots eligible for full removal.
/// Only canonical, known per-user locations are considered; Steam game files
/// and unrelated source folders outside these roots are never included.
pub fn preview_full_nuke(
    installation: &VaporInstallation,
) -> Result<FullNukePreview, UninstallError> {
    let mut candidates = vec![installation.root.clone()];
    if let Ok(default_home) = VaporInstallation::default_home_root() {
        candidates.push(default_home);
    }
    if let Some(local) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        candidates.push(local.join("Vapor"));
    }
    if let Some(roaming) = env::var_os("APPDATA").map(PathBuf::from) {
        candidates.push(roaming.join("Vapor"));
    }
    for product_root in known_legacy_steam_product_roots()? {
        candidates.extend(legacy_steam_vapor_paths(&product_root));
    }
    if cfg!(not(windows)) {
        if let Some(data) = env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
            candidates.push(data.join("vapor"));
        }
        if let Some(cache) = env::var_os("XDG_CACHE_HOME").map(PathBuf::from) {
            candidates.push(cache.join("vapor"));
        }
        if let Some(config) = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
            candidates.push(config.join("vapor"));
        }
        if let Some(home) = home_dir() {
            candidates.extend([
                home.join(".vapor"),
                home.join(".cache/vapor"),
                home.join(".config/vapor"),
                home.join(".local/share/vapor"),
            ]);
        }
    }

    let protected_home = home_dir().map(|path| fs::canonicalize(&path).unwrap_or(path));
    let mut roots = Vec::new();
    for candidate in candidates {
        let metadata = match fs::symlink_metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(UninstallError::Io {
                    path: candidate,
                    source,
                });
            }
        };
        if metadata.file_type().is_symlink() || !(metadata.is_dir() || metadata.is_file()) {
            return Err(UninstallError::UnsafePurgeRoot { path: candidate });
        }
        let canonical = fs::canonicalize(&candidate).map_err(|source| UninstallError::Io {
            path: candidate.clone(),
            source,
        })?;
        if canonical.parent().is_none()
            || protected_home
                .as_deref()
                .is_some_and(|home| canonical == home)
        {
            return Err(UninstallError::UnsafePurgeRoot { path: canonical });
        }
        if !roots.iter().any(|root| canonical == *root) {
            roots.push(canonical);
        }
    }
    roots.sort_by_key(|root| (root.components().count(), root.clone()));
    let mut non_overlapping = Vec::new();
    for root in roots {
        if !non_overlapping
            .iter()
            .any(|parent: &PathBuf| root.starts_with(parent))
        {
            non_overlapping.push(root);
        }
    }
    Ok(FullNukePreview {
        paths: non_overlapping,
    })
}

fn legacy_steam_vapor_paths(product_root: &Path) -> Vec<PathBuf> {
    LEGACY_STEAM_VAPOR_ENTRIES
        .iter()
        .map(|name| product_root.join(name))
        .collect()
}

fn known_legacy_steam_product_roots() -> Result<Vec<PathBuf>, UninstallError> {
    let steam_roots = steam_roots_with_libraries(default_steam_roots())?;
    let mut product_roots = steam_roots
        .into_iter()
        .map(|steam_root| steam_root.join("steamapps/common/Loo Cast"))
        .collect::<Vec<_>>();
    product_roots.sort();
    product_roots.dedup();
    Ok(product_roots)
}

fn steam_roots_with_libraries(
    mut steam_roots: Vec<PathBuf>,
) -> Result<Vec<PathBuf>, UninstallError> {
    steam_roots.sort();
    steam_roots.dedup();
    let known_roots = steam_roots.clone();
    for steam_root in known_roots {
        let library_file = steam_root.join("steamapps/libraryfolders.vdf");
        let contents = match fs::read_to_string(&library_file) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(UninstallError::Io {
                    path: library_file,
                    source,
                });
            }
        };
        for library in parse_steam_library_paths(&contents) {
            if !steam_roots.contains(&library) {
                steam_roots.push(library);
            }
        }
    }
    steam_roots.sort();
    steam_roots.dedup();
    Ok(steam_roots)
}

fn default_steam_roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let steam_root = env::var_os("PROGRAMFILES(X86)")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"))
            .join("Steam");
        return vec![steam_root];
    }

    #[cfg(target_os = "linux")]
    {
        let Some(home) = home_dir() else {
            return Vec::new();
        };
        return [
            home.join(".steam/steam"),
            home.join(".steam/root"),
            home.join(".local/share/Steam"),
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        ]
        .into_iter()
        .collect::<Vec<_>>();
    }

    #[cfg(target_os = "macos")]
    {
        let Some(home) = home_dir() else {
            return Vec::new();
        };
        return vec![home.join("Library/Application Support/Steam")];
    }

    #[allow(unreachable_code)]
    Vec::new()
}

fn parse_steam_library_paths(source: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in source.lines() {
        let mut values = Vec::with_capacity(2);
        let mut value = String::new();
        let mut in_string = false;
        let mut escaped = false;
        for character in line.chars() {
            if !in_string {
                if character == '"' {
                    in_string = true;
                    value.clear();
                }
                continue;
            }
            if escaped {
                value.push(match character {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
                escaped = false;
            } else {
                match character {
                    '\\' => escaped = true,
                    '"' => {
                        values.push(std::mem::take(&mut value));
                        in_string = false;
                        if values.len() == 2 {
                            break;
                        }
                    }
                    other => value.push(other),
                }
            }
        }
        if values.first().is_some_and(|key| key == "path")
            && let Some(path) = values.get(1)
            && !path.is_empty()
        {
            let path = PathBuf::from(path);
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths
}

#[derive(Debug, Clone)]
pub struct UninstallReport {
    pub installation_root: PathBuf,
    pub user_data_root: PathBuf,
    pub superworkspace_root: Option<PathBuf>,
    pub integration: Vec<String>,
    pub user_data_purged: bool,
    pub superworkspace_purged: bool,
    pub full_nuke_paths: Vec<PathBuf>,
}

pub fn uninstall_installation(
    options: UninstallOptions,
) -> Result<UninstallReport, UninstallError> {
    let installation = VaporInstallation::discover().map_err(UninstallError::Installation)?;
    uninstall_installation_for(&installation, options)
}

/// Uninstall integration for a specific Vapor Installation.
pub fn uninstall_installation_for(
    installation: &VaporInstallation,
    options: UninstallOptions,
) -> Result<UninstallReport, UninstallError> {
    let user_data_root = installation.user_data_root();

    let superworkspace_root = if options.purge_superworkspace {
        Some(active_superworkspace(installation)?)
    } else {
        None
    };

    // Phase 1: resolve and validate every safety-sensitive operation before
    // mutating integration, user data, or authored source.
    let preflight = preflight_uninstall(
        installation,
        &user_data_root,
        superworkspace_root.as_deref(),
        options,
    )?;

    #[cfg(windows)]
    let deferred_full_nuke = if preflight.full_nuke_paths.is_empty() {
        None
    } else {
        Some(stage_full_nuke_after_process_exit(
            &preflight.full_nuke_paths,
        )?)
    };

    let mut superworkspace_purged = false;

    if let Some(superworkspace) = &preflight.superworkspace_purge_root {
        superworkspace_purged = remove_preflighted_tree(superworkspace)?;
    }

    // Remove machine integration only after all safety-sensitive paths pass
    // preflight. Then remove the runtime payload while retaining user-owned
    // content by default.
    let integration = match remove_machine_integration(installation) {
        Ok(integration) => integration,
        Err(error) => {
            #[cfg(windows)]
            drop(deferred_full_nuke);
            return Err(error);
        }
    };
    let user_data_purged = if let Some(user_data) = &preflight.user_data_purge_root {
        remove_preflighted_tree(user_data)?
    } else if preflight.full_nuke_paths.is_empty() {
        remove_runtime_payload(installation)?;
        false
    } else {
        false
    };
    let mut full_nuke_paths = Vec::new();
    if !preflight.full_nuke_paths.is_empty() {
        #[cfg(windows)]
        deferred_full_nuke
            .expect("a deferred cleanup was staged for non-empty Windows roots")
            .commit()?;
        #[cfg(not(windows))]
        for path in &preflight.full_nuke_paths {
            if remove_preflighted_tree(path)? {
                full_nuke_paths.push(path.clone());
            }
        }
        #[cfg(windows)]
        full_nuke_paths.clone_from(&preflight.full_nuke_paths);
    }

    Ok(UninstallReport {
        installation_root: installation.root.clone(),
        user_data_root,
        superworkspace_root,
        integration,
        user_data_purged,
        superworkspace_purged,
        full_nuke_paths,
    })
}

#[derive(Debug)]
struct UninstallPreflight {
    superworkspace_purge_root: Option<PathBuf>,
    user_data_purge_root: Option<PathBuf>,
    full_nuke_paths: Vec<PathBuf>,
}

fn preflight_uninstall(
    installation: &VaporInstallation,
    user_data_root: &Path,
    superworkspace_root: Option<&Path>,
    options: UninstallOptions,
) -> Result<UninstallPreflight, UninstallError> {
    preflight_machine_integration_removal()?;

    if options.full_nuke {
        let preview = preview_full_nuke(installation)?;
        if preview.paths.is_empty() {
            return Err(UninstallError::UnsafePurgeRoot {
                path: installation.root.clone(),
            });
        }
        return Ok(UninstallPreflight {
            superworkspace_purge_root: None,
            user_data_purge_root: None,
            full_nuke_paths: preview.paths,
        });
    }

    let superworkspace_purge_root = if let Some(superworkspace) = superworkspace_root {
        let managed_sources = installation.root.join("sources");
        let root = preflight_owned_tree(
            superworkspace,
            &[installation.root.as_path(), user_data_root],
            Some(&managed_sources),
        )?
        .ok_or_else(|| UninstallError::UnsafePurgeRoot {
            path: superworkspace.to_path_buf(),
        })?;

        ensure_superworkspace_purge_safe(&root)?;
        Some(root)
    } else {
        None
    };

    let user_data_purge_root = if options.purge_user_data {
        let root = fs::canonicalize(user_data_root).map_err(|source| UninstallError::Io {
            path: user_data_root.to_path_buf(),
            source,
        })?;
        if root.parent().is_none() || home_dir().is_some_and(|home| same_path(&root, &home)) {
            return Err(UninstallError::UnsafePurgeRoot { path: root });
        }
        Some(root)
    } else {
        None
    };

    Ok(UninstallPreflight {
        superworkspace_purge_root,
        user_data_purge_root,
        full_nuke_paths: Vec::new(),
    })
}

fn preflight_owned_tree(
    path: &Path,
    protected: &[&Path],
    allowed_managed_child: Option<&Path>,
) -> Result<Option<PathBuf>, UninstallError> {
    if !path.exists() {
        return Ok(None);
    }

    let path = fs::canonicalize(path).map_err(|source| UninstallError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    if path.parent().is_none() || home_dir().is_some_and(|home| same_path(&path, &home)) {
        return Err(UninstallError::UnsafePurgeRoot { path });
    }

    let is_allowed_managed_child = allowed_managed_child
        .and_then(|allowed| fs::canonicalize(allowed).ok())
        .is_some_and(|allowed| same_path(&path, &allowed));

    for protected in protected {
        let protected = fs::canonicalize(protected).unwrap_or_else(|_| (*protected).to_path_buf());
        if path.starts_with(&protected) || protected.starts_with(&path) {
            if is_allowed_managed_child
                && path
                    .parent()
                    .is_some_and(|parent| same_path(parent, &protected))
            {
                continue;
            }
            return Err(UninstallError::UnsafePurgeRoot { path });
        }
    }

    Ok(Some(path))
}

fn preflight_machine_integration_removal() -> Result<(), UninstallError> {
    if cfg!(windows) {
        return Ok(());
    }

    for profile in shell_profiles() {
        validate_managed_profile_block(&profile)?;
    }

    Ok(())
}

fn validate_managed_profile_block(path: &Path) -> Result<(), UninstallError> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(UninstallError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let mut inside = false;
    for line in source.lines() {
        if line == PROFILE_BLOCK_START {
            inside = true;
            continue;
        }

        if inside && line == PROFILE_BLOCK_END {
            inside = false;
        }
    }

    if inside {
        return Err(UninstallError::MalformedProfileBlock {
            path: path.to_path_buf(),
        });
    }

    Ok(())
}

fn active_superworkspace(installation: &VaporInstallation) -> Result<PathBuf, UninstallError> {
    let state = source_state(installation).map_err(UninstallError::Source)?;

    if let Some(configured) = state.superworkspace.as_deref()
        && let Ok(superworkspace) = VaporSuperworkspace::discover_from(configured)
    {
        return Ok(superworkspace.root);
    }

    if let Some(active) = state.active.as_deref()
        && let Ok(superworkspace) = VaporSuperworkspace::discover_from(active)
    {
        return Ok(superworkspace.root);
    }

    if let Ok(current) = env::current_dir()
        && let Ok(superworkspace) = VaporSuperworkspace::discover_from(&current)
    {
        return Ok(superworkspace.root);
    }

    let mut known = BTreeSet::new();
    for source in &state.known {
        if let Ok(superworkspace) = VaporSuperworkspace::discover_from(source) {
            known.insert(superworkspace.root);
        }
    }

    if known.len() == 1 {
        return Ok(known.into_iter().next().expect("length checked"));
    }

    Err(UninstallError::NoActiveSuperworkspace {
        state_path: state.state_path,
    })
}

fn remove_runtime_payload(installation: &VaporInstallation) -> Result<(), UninstallError> {
    for name in ["bin", "docs", "examples", "metadata", "resources", "tools"] {
        let path = installation.root.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_file() => {
                fs::remove_file(&path).map_err(|source| UninstallError::Io {
                    path: path.clone(),
                    source,
                })?;
            }
            Ok(_) => fs::remove_dir_all(&path).map_err(|source| UninstallError::Io {
                path: path.clone(),
                source,
            })?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(UninstallError::Io {
                    path: path.clone(),
                    source,
                });
            }
        }
    }
    let marker = installation.root.join(VAPOR_HOME_MANIFEST_FILE_NAME);
    if marker.is_file() {
        fs::remove_file(&marker).map_err(|source| UninstallError::Io {
            path: marker.clone(),
            source,
        })?;
    }
    Ok(())
}

fn remove_machine_integration(
    installation: &VaporInstallation,
) -> Result<Vec<String>, UninstallError> {
    let mut removed = Vec::new();

    if cfg!(windows) {
        if remove_windows_user_path(installation)? {
            removed.push("removed Vapor host binary directory from user PATH".to_owned());
        }

        if remove_windows_user_environment(VAPOR_HOME_ENV, &installation.root)? {
            removed.push(format!(
                "removed user environment variable {VAPOR_HOME_ENV}"
            ));
        }
    } else {
        for profile in shell_profiles() {
            if remove_managed_profile_block(&profile, &installation.root)? {
                removed.push(format!(
                    "removed managed Vapor PATH block from {}",
                    profile.display()
                ));
            }
        }
    }

    Ok(removed)
}

fn shell_profiles() -> Vec<PathBuf> {
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };

    [".profile", ".bashrc", ".zshrc"]
        .into_iter()
        .map(|name| home.join(name))
        .collect()
}

fn remove_managed_profile_block(
    path: &Path,
    installation_root: &Path,
) -> Result<bool, UninstallError> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(UninstallError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    if !managed_profile_block_targets(&source, installation_root) {
        return Ok(false);
    }

    let mut changed = false;
    let mut inside = false;
    let mut output = String::new();

    for line in source.lines() {
        if line == PROFILE_BLOCK_START {
            changed = true;
            inside = true;
            continue;
        }

        if inside {
            if line == PROFILE_BLOCK_END {
                inside = false;
            }
            continue;
        }

        output.push_str(line);
        output.push('\n');
    }

    if inside {
        return Err(UninstallError::MalformedProfileBlock {
            path: path.to_path_buf(),
        });
    }

    if changed {
        fs::write(path, output).map_err(|source| UninstallError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }

    Ok(changed)
}

fn remove_windows_user_path(installation: &VaporInstallation) -> Result<bool, UninstallError> {
    let target = crate::HostTarget::current()
        .map_err(|error| UninstallError::Io {
            path: installation.root.clone(),
            source: io::Error::other(error),
        })?
        .triple();

    let bin = installation.root.join("bin").join(target);
    let legacy = format!(r"%VAPOR_HOME%\bin\{target}");
    let script = r#"
$bin = $args[0]
$legacy = $args[1]
$path = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($null -eq $path) { exit 0 }
$entries = @($path -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
$filtered = @($entries | Where-Object {
    -not [string]::Equals($_, $bin, [StringComparison]::OrdinalIgnoreCase) -and
    -not [string]::Equals($_, $legacy, [StringComparison]::OrdinalIgnoreCase)
})
if ($filtered.Count -ne $entries.Count) {
    [Environment]::SetEnvironmentVariable('Path', ($filtered -join ';'), 'User')
    Write-Output 'removed'
}
"#;

    let output = crate::managed_process::background_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .arg(&bin)
        .arg(&legacy)
        .output()
        .map_err(|source| UninstallError::CommandStart {
            command: "PowerShell user PATH cleanup".to_owned(),
            source,
        })?;

    if !output.status.success() {
        return Err(UninstallError::CommandFailed {
            command: "PowerShell user PATH cleanup".to_owned(),
            status: output.status.to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).contains("removed"))
}

fn remove_windows_user_environment(
    variable: &str,
    installation_root: &Path,
) -> Result<bool, UninstallError> {
    let script = r#"
$name = $args[0]
$expected = [IO.Path]::GetFullPath($args[1])
$current = [Environment]::GetEnvironmentVariable($name, 'User')
if ([string]::IsNullOrWhiteSpace($current)) { exit 0 }
try { $actual = [IO.Path]::GetFullPath($current) } catch { exit 0 }
if ([string]::Equals($actual, $expected, [StringComparison]::OrdinalIgnoreCase)) {
    [Environment]::SetEnvironmentVariable($name, $null, 'User')
    Write-Output 'removed'
}
"#;
    let output = crate::managed_process::background_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .arg(variable)
        .arg(installation_root)
        .output()
        .map_err(|source| UninstallError::CommandStart {
            command: format!("PowerShell user environment cleanup for {variable}"),
            source,
        })?;

    if !output.status.success() {
        return Err(UninstallError::CommandFailed {
            command: format!("PowerShell user environment cleanup for {variable}"),
            status: output.status.to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).contains("removed"))
}

fn managed_profile_block_targets(source: &str, installation_root: &Path) -> bool {
    let expected = format!(
        "export {VAPOR_HOME_ENV}={}",
        shell_single_quote(&installation_root.to_string_lossy())
    );
    let mut inside = false;
    for line in source.lines() {
        if line == PROFILE_BLOCK_START {
            inside = true;
            continue;
        }
        if inside && line == PROFILE_BLOCK_END {
            break;
        }
        if inside && line == expected {
            return true;
        }
    }
    false
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn ensure_superworkspace_purge_safe(root: &Path) -> Result<(), UninstallError> {
    let root = fs::canonicalize(root).map_err(|source| UninstallError::Io {
        path: root.to_path_buf(),
        source,
    })?;

    for entry in fs::read_dir(&root).map_err(|source| UninstallError::Io {
        path: root.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| UninstallError::Io {
            path: root.clone(),
            source,
        })?;
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(UninstallError::UnsafeSuperworkspaceEntry { path });
        };

        if matches!(name, SUPERWORKSPACE_MANIFEST_FILE_NAME | ".vaporignore") {
            continue;
        }

        let file_type = entry.file_type().map_err(|source| UninstallError::Io {
            path: path.clone(),
            source,
        })?;

        if matches!(name, ".idea" | ".run") {
            if !file_type.is_dir() || file_type.is_symlink() {
                return Err(UninstallError::UnsafeSuperworkspaceEntry { path });
            }
            continue;
        }

        if !MANAGED_SUPERWORKSPACE_REPOSITORIES.contains(&name)
            || !file_type.is_dir()
            || file_type.is_symlink()
        {
            return Err(UninstallError::UnsafeSuperworkspaceEntry { path });
        }

        ensure_expected_origin(name, &path)?;
        ensure_source_checkout_safe(name, &path)?;
    }

    Ok(())
}

fn ensure_expected_origin(name: &str, root: &Path) -> Result<(), UninstallError> {
    let origin = git_output(root, &["remote", "get-url", "origin"])?;
    let normalized = origin.trim().trim_end_matches('/').trim_end_matches(".git");
    let expected = format!("GHF-Studios/{name}");

    if !normalized.ends_with(&expected) {
        return Err(UninstallError::UnsafeSource {
            message: format!(
                "refusing to purge `{}` because origin `{}` does not match expected first-party repository `{expected}`",
                root.display(),
                origin.trim(),
            ),
        });
    }

    Ok(())
}

fn ensure_source_checkout_safe(name: &str, root: &Path) -> Result<(), UninstallError> {
    ensure_git_checkout_safe(name, root)?;

    let submodules = git_output(root, &["submodule", "status"])?;
    for line in submodules.lines() {
        let Some(prefix) = line.chars().next() else {
            continue;
        };

        if prefix == '-' {
            continue;
        }

        if matches!(prefix, '+' | 'U') {
            return Err(UninstallError::UnsafeSource {
                message: format!(
                    "refusing to purge source `{name}` because direct submodule state is not clean: {line}"
                ),
            });
        }

        let mut fields = line[1..].split_whitespace();
        let _commit = fields.next();
        let Some(path) = fields.next() else {
            continue;
        };
        let submodule = root.join(path);
        if submodule.is_dir() {
            ensure_git_checkout_safe(&format!("{name}/{path}"), &submodule)?;
        }
    }

    Ok(())
}

fn ensure_git_checkout_safe(name: &str, root: &Path) -> Result<(), UninstallError> {
    let inside = git_output(root, &["rev-parse", "--is-inside-work-tree"])?;
    if inside != "true" {
        return Err(UninstallError::UnsafeSource {
            message: format!(
                "source `{name}` is not a Git work tree: `{}`",
                root.display()
            ),
        });
    }

    let dirty = git_output(
        root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    if !dirty.is_empty() {
        return Err(UninstallError::UnsafeSource {
            message: format!(
                "refusing to purge source `{name}` because it has uncommitted/untracked changes:\n{dirty}"
            ),
        });
    }

    git_output(root, &["fetch", "--quiet", "--prune", "origin"])?;

    let local_only = git_output(
        root,
        &["rev-list", "--branches", "--not", "--remotes=origin"],
    )?;
    if !local_only.is_empty() {
        return Err(UninstallError::UnsafeSource {
            message: format!(
                "refusing to purge source `{name}` because local branches contain commits not present on origin"
            ),
        });
    }

    let remote_contains_head = git_output(root, &["branch", "-r", "--contains", "HEAD"])?;
    if remote_contains_head.is_empty() {
        return Err(UninstallError::UnsafeSource {
            message: format!(
                "refusing to purge source `{name}` because HEAD is not reachable from an origin branch"
            ),
        });
    }

    let local_tags = git_output(
        root,
        &[
            "for-each-ref",
            "--format=%(refname:strip=2) %(objectname)",
            "refs/tags",
        ],
    )?;
    let remote_tags = git_output(root, &["ls-remote", "--tags", "--refs", "origin"])?;

    let remote_tags = remote_tags
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let object = fields.next()?;
            let reference = fields.next()?;
            Some((
                reference.trim_start_matches("refs/tags/").to_owned(),
                object.to_owned(),
            ))
        })
        .collect::<BTreeMap<_, _>>();

    for line in local_tags.lines() {
        let Some((tag, object)) = line.split_once(' ') else {
            continue;
        };
        if remote_tags.get(tag).map(String::as_str) != Some(object) {
            return Err(UninstallError::UnsafeSource {
                message: format!(
                    "refusing to purge source `{name}` because local tag `{tag}` is not identically published on origin"
                ),
            });
        }
    }

    Ok(())
}

fn git_output(root: &Path, args: &[&str]) -> Result<String, UninstallError> {
    let command = format!("git -C {} {}", root.display(), args.join(" "));
    let output = crate::managed_process::background_command("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|source| UninstallError::CommandStart {
            command: command.clone(),
            source,
        })?;

    if !output.status.success() {
        return Err(UninstallError::CommandFailed {
            command,
            status: output.status.to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn remove_preflighted_tree(path: &Path) -> Result<bool, UninstallError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(UninstallError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(UninstallError::UnsafePurgeRoot {
            path: path.to_path_buf(),
        });
    }

    let result = if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else if metadata.is_file() {
        fs::remove_file(path)
    } else {
        return Err(UninstallError::UnsafePurgeRoot {
            path: path.to_path_buf(),
        });
    };
    result.map_err(|source| UninstallError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    Ok(true)
}

#[cfg(windows)]
struct PendingFullNuke {
    commit_file: PathBuf,
    cancel_file: PathBuf,
    committed: bool,
}

#[cfg(windows)]
impl PendingFullNuke {
    fn commit(mut self) -> Result<(), UninstallError> {
        fs::write(&self.commit_file, b"commit").map_err(|source| UninstallError::Io {
            path: self.commit_file.clone(),
            source,
        })?;
        self.committed = true;
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for PendingFullNuke {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::write(&self.cancel_file, b"cancel");
        }
    }
}

#[cfg(windows)]
fn stage_full_nuke_after_process_exit(
    roots: &[PathBuf],
) -> Result<PendingFullNuke, UninstallError> {
    let temp = env::temp_dir();
    let suffix = format!("{}-{}", std::process::id(), now_nonce());
    let roots_file = temp.join(format!("vapor-full-nuke-{suffix}.json"));
    let script_file = temp.join(format!("vapor-full-nuke-{suffix}.ps1"));
    let commit_file = temp.join(format!("vapor-full-nuke-{suffix}.commit"));
    let cancel_file = temp.join(format!("vapor-full-nuke-{suffix}.cancel"));
    let roots_json = serde_json::to_vec(roots).map_err(|error| UninstallError::Io {
        path: roots_file.clone(),
        source: io::Error::other(error),
    })?;
    fs::write(&roots_file, roots_json).map_err(|source| UninstallError::Io {
        path: roots_file.clone(),
        source,
    })?;
    let script = deferred_full_nuke_script();
    if let Err(source) = fs::write(&script_file, script) {
        let _ = fs::remove_file(&roots_file);
        let _ = fs::remove_file(&cancel_file);
        return Err(UninstallError::Io {
            path: script_file,
            source,
        });
    }
    let mut command = crate::managed_process::background_command("powershell");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script_file)
        .arg(std::process::id().to_string())
        .arg(&roots_file)
        .arg(&commit_file)
        .arg(&cancel_file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Err(source) = command.spawn() {
        let _ = fs::remove_file(&roots_file);
        let _ = fs::remove_file(&script_file);
        let _ = fs::remove_file(&cancel_file);
        return Err(UninstallError::CommandStart {
            command: "deferred full Vapor removal".to_owned(),
            source,
        });
    }
    Ok(PendingFullNuke {
        commit_file,
        cancel_file,
        committed: false,
    })
}

#[cfg(windows)]
fn deferred_full_nuke_script() -> &'static str {
    r#"
param([int]$ParentProcessId, [string]$RootsFile, [string]$CommitFile, [string]$CancelFile)
$ErrorActionPreference = 'Stop'
$committed = $false
try {
    while (Get-Process -Id $ParentProcessId -ErrorAction SilentlyContinue) { Start-Sleep -Milliseconds 500 }
    for ($attempt = 0; $attempt -lt 40; $attempt++) {
        if (Test-Path -LiteralPath $CancelFile) { return }
        if (Test-Path -LiteralPath $CommitFile) {
            if ((Get-Content -LiteralPath $CommitFile -Raw) -ceq 'commit') { $committed = $true; break }
            return
        }
        Start-Sleep -Milliseconds 250
    }
    if (-not $committed) { return }
    $roots = Get-Content -LiteralPath $RootsFile -Raw | ConvertFrom-Json
    foreach ($root in $roots) {
        if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
    }
} finally {
    Remove-Item -LiteralPath $RootsFile -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $CommitFile -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $CancelFile -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
}
"#
}

#[cfg(windows)]
fn now_nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn same_path(left: &Path, right: &Path) -> bool {
    fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf())
        == fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf())
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[derive(Debug)]
pub enum UninstallError {
    Installation(InstallationError),
    Source(SourceError),
    Superworkspace(SuperworkspaceError),
    NoActiveSuperworkspace { state_path: PathBuf },
    UnsafePurgeRoot { path: PathBuf },
    UnsafeSuperworkspaceEntry { path: PathBuf },
    UnsafeSource { message: String },
    MalformedProfileBlock { path: PathBuf },
    CommandStart { command: String, source: io::Error },
    CommandFailed { command: String, status: String },
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for UninstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installation(error) => error.fmt(formatter),
            Self::Source(error) => error.fmt(formatter),
            Self::Superworkspace(error) => error.fmt(formatter),
            Self::NoActiveSuperworkspace { state_path } => write!(
                formatter,
                "--purge-superworkspace could not resolve the canonical Superworkspace (source state: `{}`)",
                state_path.display(),
            ),
            Self::UnsafePurgeRoot { path } => {
                write!(formatter, "refusing unsafe purge root `{}`", path.display())
            }
            Self::UnsafeSuperworkspaceEntry { path } => write!(
                formatter,
                "refusing to purge Superworkspace because it contains unrecognized entry `{}`",
                path.display()
            ),
            Self::UnsafeSource { message } => formatter.write_str(message),
            Self::MalformedProfileBlock { path } => write!(
                formatter,
                "managed Vapor PATH block in `{}` has no closing marker; refusing to rewrite it",
                path.display()
            ),
            Self::CommandStart { command, source } => {
                write!(formatter, "failed to start `{command}`: {source}")
            }
            Self::CommandFailed { command, status } => {
                write!(formatter, "`{command}` failed with {status}")
            }
            Self::Io { path, source } => {
                write!(formatter, "failed to modify `{}`: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for UninstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Installation(error) => Some(error),
            Self::Source(error) => Some(error),
            Self::Superworkspace(error) => Some(error),
            Self::CommandStart { source, .. } | Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PROFILE_BLOCK_END, PROFILE_BLOCK_START, deferred_full_nuke_script,
        legacy_steam_vapor_paths, managed_profile_block_targets, parse_steam_library_paths,
        remove_preflighted_tree, steam_roots_with_libraries,
    };
    use crate::VAPOR_HOME_ENV;
    use std::path::{Path, PathBuf};

    #[test]
    fn managed_profile_cleanup_matches_only_the_selected_installation() {
        let primary = Path::new("/opt/vapor-primary");
        let other = Path::new("/opt/vapor-other");
        let profile = format!(
            "{PROFILE_BLOCK_START}\nexport {VAPOR_HOME_ENV}='{}'\nexport PATH=\"${{{VAPOR_HOME_ENV}}}/bin/x86_64-unknown-linux-gnu:$PATH\"\n{PROFILE_BLOCK_END}\n",
            primary.display()
        );

        assert!(managed_profile_block_targets(&profile, primary));
        assert!(!managed_profile_block_targets(&profile, other));
    }

    #[test]
    fn legacy_steam_cleanup_targets_vapor_entries_without_touching_packagepack() {
        let product_root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Loo Cast");
        let targets = legacy_steam_vapor_paths(product_root);
        for name in [
            ".vapor",
            "bin",
            "cargo-home",
            "content",
            "docs",
            "examples",
            "metadata",
            "output",
            "resources",
            "rustup",
            "rustup-home",
            "tools",
            "App.vapor.toml",
        ] {
            assert!(targets.contains(&product_root.join(name)), "missing {name}");
        }
        assert!(!targets.contains(&product_root.join("Packagepack.vapor.toml")));
        assert!(!targets.contains(&product_root.join("loo-cast-packagepack.exe")));
        assert!(!targets.contains(&product_root.join("loo-cast-packagepack")));
    }

    #[test]
    fn steam_library_discovery_reads_only_path_values_and_unescapes_windows_paths() {
        let source = r#"
            "libraryfolders"
            {
                "0"
                {
                    "path" "C:\\Program Files (x86)\\Steam"
                    "label" "not a library path"
                }
                "1"
                {
                    "path" "D:\\Games\\Steam Library"
                }
                "2"
                {
                    "apps" "C:\\not-a-library"
                }
            }
        "#;

        assert_eq!(
            parse_steam_library_paths(source),
            vec![
                PathBuf::from(r"C:\Program Files (x86)\Steam"),
                PathBuf::from(r"D:\Games\Steam Library"),
            ]
        );
    }

    #[test]
    fn steam_library_discovery_follows_the_default_install_library_index() {
        let root = std::env::temp_dir().join(format!(
            "vapor-steam-library-test-{}-{}",
            std::process::id(),
            super::now_nonce()
        ));
        let default_root = root.join("Steam");
        std::fs::create_dir_all(default_root.join("steamapps")).expect("create default Steam root");
        std::fs::write(
            default_root.join("steamapps/libraryfolders.vdf"),
            r#"
                "libraryfolders"
                {
                    "0"
                    {
                        "path" "C:\\Steam"
                    }
                    "1"
                    {
                        "path" "D:\\SteamLibrary"
                    }
                }
            "#,
        )
        .expect("write Steam library index");

        let roots = steam_roots_with_libraries(vec![default_root.clone()])
            .expect("resolve Steam library roots");

        assert!(roots.contains(&default_root));
        assert!(roots.contains(&PathBuf::from(r"C:\Steam")));
        assert!(roots.contains(&PathBuf::from(r"D:\SteamLibrary")));
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn full_nuke_removes_a_preflighted_legacy_manifest_file() {
        let root = std::env::temp_dir().join(format!(
            "vapor-full-nuke-file-test-{}-{}",
            std::process::id(),
            super::now_nonce()
        ));
        std::fs::create_dir_all(&root).expect("create disposable root");
        let target = root.join("App.vapor.toml");
        std::fs::write(&target, "legacy manifest").expect("write disposable manifest");

        assert!(remove_preflighted_tree(&target).expect("remove preflighted file"));
        assert!(!target.exists());
        std::fs::remove_dir_all(root).expect("remove disposable root");
    }

    #[cfg(windows)]
    #[test]
    fn deferred_full_nuke_requires_an_explicit_commit_marker() {
        use std::process::Command;

        let root = std::env::temp_dir().join(format!(
            "vapor-full-nuke-test-{}-{}",
            std::process::id(),
            super::now_nonce()
        ));
        let target = root.join("target");
        let plan = root.join("roots.json");
        let script = root.join("cleanup.ps1");
        let commit = root.join("commit");
        let cancel = root.join("cancel");
        std::fs::create_dir_all(&target).expect("create disposable target");
        std::fs::write(target.join("marker"), "test").expect("write disposable marker");
        std::fs::write(&plan, serde_json::to_vec(&[&target]).unwrap()).expect("write plan");
        std::fs::write(&script, deferred_full_nuke_script()).expect("write helper");
        std::fs::write(&cancel, "cancel").expect("write cancel marker");

        let run_helper = |script: &Path, plan: &Path, commit: &Path, cancel: &Path| {
            Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                ])
                .arg(&script)
                .arg("2147483647")
                .arg(&plan)
                .arg(&commit)
                .arg(&cancel)
                .status()
                .expect("run cleanup helper")
        };

        let status = run_helper(&script, &plan, &commit, &cancel);
        assert!(status.success());
        assert!(target.exists(), "no commit marker must leave data intact");

        std::fs::write(&plan, serde_json::to_vec(&[&target]).unwrap()).expect("rewrite plan");
        std::fs::write(&script, deferred_full_nuke_script()).expect("rewrite helper");
        let _ = std::fs::remove_file(&cancel);
        std::fs::write(&commit, "partial").expect("write malformed commit marker");
        let status = run_helper(&script, &plan, &commit, &cancel);
        assert!(status.success());
        assert!(target.exists(), "a partial marker must leave data intact");

        std::fs::write(&plan, serde_json::to_vec(&[&target]).unwrap()).expect("rewrite plan");
        std::fs::write(&script, deferred_full_nuke_script()).expect("rewrite helper");
        std::fs::write(&commit, "commit").expect("write commit marker");
        let status = run_helper(&script, &plan, &commit, &cancel);
        assert!(status.success());
        assert!(!target.exists(), "commit marker permits the planned delete");
        let _ = std::fs::remove_dir_all(root);
    }
}
