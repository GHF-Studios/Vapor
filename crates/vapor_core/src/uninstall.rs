//! Vapor installation teardown and explicit purge operations.
//!
//! Ordinary uninstall removes machine integration only. Vapor-owned OS user
//! data and authored source survive unless explicitly purged.

use crate::installation::{
    InstallationError, VAPOR_HOME_ENV, VAPOR_HOME_MANIFEST_FILE_NAME, VaporInstallation,
};
use crate::source::{SourceError, SourceState, source_state, source_state_at_user_data_root};
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
    /// Permanently erase Vapor User Data and registered source checkouts.
    /// Unknown neighboring files in a shared Superworkspace are preserved.
    pub erase_all: bool,
}

#[derive(Debug, Clone)]
pub struct UninstallReport {
    pub installation_root: PathBuf,
    pub user_data_root: PathBuf,
    pub superworkspace_root: Option<PathBuf>,
    pub integration: Vec<String>,
    pub user_data_purged: bool,
    pub superworkspace_purged: bool,
    pub preserved_superworkspace_entries: Vec<PathBuf>,
}

/// Read-only preview of every Vapor-owned root the standalone full wipe will remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaporDataErasurePlan {
    pub user_data_root: PathBuf,
    pub superworkspace_root: Option<PathBuf>,
    pub source_repositories: Vec<PathBuf>,
    pub superworkspace_data: Vec<PathBuf>,
    pub preserved_superworkspace_entries: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct VaporDataErasureReport {
    pub user_data_root: PathBuf,
    pub superworkspace_root: Option<PathBuf>,
    pub integration: Vec<String>,
    pub preserved_superworkspace_entries: Vec<PathBuf>,
}

/// Preview a full Vapor wipe without requiring an installed App Instance.
///
/// This is the path used by the website bootstrap after a normal app removal,
/// when only Vapor User Data and the registered Superworkspace remain.
pub fn plan_vapor_data_erasure() -> Result<VaporDataErasurePlan, UninstallError> {
    let user_data_root = vapor_product_layout::user_data_root().ok_or_else(|| {
        UninstallError::Io {
            path: PathBuf::from("Vapor User Data"),
            source: io::Error::new(io::ErrorKind::NotFound, "canonical user-data root is unavailable"),
        }
    })?;
    let user_data_to_remove = preflight_owned_tree(&user_data_root, &[])?;
    let state = source_state_at_user_data_root(&user_data_root).map_err(UninstallError::Source)?;
    let registered = registered_superworkspace_for_erasure(&state)?;
    let superworkspace_root = match registered {
        Some(root) => preflight_owned_tree(&root, &[user_data_root.as_path()])?,
        None => None,
    };
    let (source_repositories, superworkspace_data, preserved_superworkspace_entries) = match &superworkspace_root {
        Some(root) => preview_superworkspace_erasure(root)?,
        None => (Vec::new(), Vec::new(), Vec::new()),
    };

    Ok(VaporDataErasurePlan {
        user_data_root: user_data_to_remove.unwrap_or(user_data_root),
        superworkspace_root,
        source_repositories,
        superworkspace_data,
        preserved_superworkspace_entries,
    })
}

/// Permanently erase Vapor User Data and registered source checkouts.
///
/// All roots and machine-integration files are validated before the first
/// deletion. Unrecognized neighbors in the shared Superworkspace are kept.
pub fn erase_all_vapor_data() -> Result<VaporDataErasureReport, UninstallError> {
    let plan = plan_vapor_data_erasure()?;
    erase_all_vapor_data_from_plan(&plan)
}

/// Execute the reviewed preview only if the registered removal boundary is
/// still identical to what the caller showed the user.
pub fn erase_all_vapor_data_from_plan(
    reviewed_plan: &VaporDataErasurePlan,
) -> Result<VaporDataErasureReport, UninstallError> {
    let plan = plan_vapor_data_erasure()?;
    if &plan != reviewed_plan {
        return Err(UninstallError::UnsafeSource {
            message: "registered Vapor paths changed after the erase preview; review the updated paths and confirm again".to_owned(),
        });
    }
    preflight_machine_integration_removal()?;
    let user_data_root = preflight_owned_tree(&plan.user_data_root, &[])?;
    let superworkspace_root = match &plan.superworkspace_root {
        Some(root) => {
            preflight_registered_superworkspace_paths(root, &[plan.user_data_root.as_path()])?;
            Some(root.clone())
        }
        None => None,
    };

    let mut preserved_superworkspace_entries = Vec::new();
    if let Some(root) = &superworkspace_root {
        let (_, preserved) = erase_registered_superworkspace(root)?;
        preserved_superworkspace_entries = preserved;
        if preserved_superworkspace_entries.is_empty() {
            fs::remove_dir(root).map_err(|source| UninstallError::Io {
                path: root.clone(),
                source,
            })?;
        }
    }
    if let Some(root) = user_data_root {
        remove_preflighted_tree(&root)?;
    }
    let integration = remove_all_vapor_user_integration(&plan.user_data_root)?;

    Ok(VaporDataErasureReport {
        user_data_root: plan.user_data_root,
        superworkspace_root,
        integration,
        preserved_superworkspace_entries,
    })
}

/// Remove shell/environment integration owned by the standalone product while
/// preserving Vapor User Data and registered source checkouts.
pub fn remove_standalone_machine_integration() -> Result<Vec<String>, UninstallError> {
    let user_data_root = vapor_product_layout::user_data_root().ok_or_else(|| UninstallError::Io {
        path: PathBuf::from("Vapor User Data"),
        source: io::Error::new(io::ErrorKind::NotFound, "canonical user-data root is unavailable"),
    })?;
    preflight_machine_integration_removal()?;
    let mut removed = Vec::new();
    if cfg!(windows) {
        if remove_windows_user_vapor_integration(&user_data_root)? {
            removed.push("removed Vapor user PATH entries and environment roots".to_owned());
        }
    } else {
        for profile in shell_profiles() {
            if remove_managed_profile_blocks_under_root(&profile, &user_data_root)? {
                removed.push(format!(
                    "removed managed Vapor PATH block from {}",
                    profile.display()
                ));
            }
        }
    }
    Ok(removed)
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

    let erase_all = options.erase_all;
    let purge_superworkspace = options.purge_superworkspace || erase_all;
    let purge_user_data = options.purge_user_data || erase_all;
    let superworkspace_root = if erase_all {
        match registered_superworkspace(installation) {
            Ok(root) => root,
            Err(UninstallError::NoActiveSuperworkspace { .. }) => None,
            Err(error) => return Err(error),
        }
    } else if purge_superworkspace {
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
        purge_user_data,
        erase_all,
    )?;

    if !purge_user_data && installation.legacy_state_root().exists() {
        installation
            .ensure_state_root()
            .map_err(UninstallError::Installation)?;
    }

    let mut superworkspace_purged = false;
    let mut preserved_superworkspace_entries = Vec::new();

    if let Some(superworkspace) = &preflight.superworkspace_purge_root {
        if erase_all {
            let (removed, preserved) = erase_registered_superworkspace(superworkspace)?;
            superworkspace_purged = removed;
            preserved_superworkspace_entries = preserved;
        } else {
            superworkspace_purged = remove_preflighted_tree(superworkspace)?;
        }
    }

    // Remove machine integration only after all safety-sensitive paths pass
    // preflight. Then remove the runtime payload while retaining user-owned
    // content by default.
    let integration = remove_machine_integration(installation)?;
    let user_data_purged = if let Some(user_data) = &preflight.user_data_purge_root {
        remove_preflighted_tree(user_data)?
    } else {
        false
    };
    remove_runtime_payload(installation)?;

    Ok(UninstallReport {
        installation_root: installation.root.clone(),
        user_data_root,
        superworkspace_root,
        integration,
        user_data_purged,
        superworkspace_purged,
        preserved_superworkspace_entries,
    })
}

/// Verify that first-party source under the active Superworkspace is clean
/// and fully published before a full Vapor Home erasure.
pub fn validate_superworkspace_for_erasure(
    installation: &VaporInstallation,
) -> Result<PathBuf, UninstallError> {
    // A Vapor Home marked by the installation itself is the explicit scope of
    // `erase all`; it must remain erasable even if source-state bookkeeping was
    // erased or left pointing at a previous workspace during a clean rebuild.
    let home_marker = installation.root.join("Vapor-Home.toml");
    if fs::read_to_string(&home_marker)
        .is_ok_and(|contents| contents.contains("id = \"ghf-studios/vapor\""))
    {
        return Ok(installation.root.clone());
    }

    let root = active_superworkspace(installation)?;
    ensure_superworkspace_purge_safe(&root)?;
    Ok(root)
}

#[derive(Debug)]
struct UninstallPreflight {
    superworkspace_purge_root: Option<PathBuf>,
    user_data_purge_root: Option<PathBuf>,
}

fn preflight_uninstall(
    installation: &VaporInstallation,
    user_data_root: &Path,
    superworkspace_root: Option<&Path>,
    purge_user_data: bool,
    erase_all: bool,
) -> Result<UninstallPreflight, UninstallError> {
    preflight_machine_integration_removal()?;

    let superworkspace_purge_root = if let Some(superworkspace) = superworkspace_root {
        let root = preflight_owned_tree_with_managed_child(
            superworkspace,
            &[installation.root.as_path(), user_data_root],
            Some(&installation.sources_root()),
        )?
        .ok_or_else(|| UninstallError::UnsafePurgeRoot {
            path: superworkspace.to_path_buf(),
        })?;

        if erase_all {
            preflight_registered_superworkspace(&root, installation, user_data_root)?;
        } else {
            ensure_superworkspace_purge_safe(&root)?;
        }
        Some(root)
    } else {
        None
    };

    let user_data_purge_root = if purge_user_data {
        preflight_owned_tree(user_data_root, &[installation.root.as_path()])?
    } else {
        None
    };

    Ok(UninstallPreflight {
        superworkspace_purge_root,
        user_data_purge_root,
    })
}

fn preflight_registered_superworkspace(
    root: &Path,
    installation: &VaporInstallation,
    user_data_root: &Path,
) -> Result<(), UninstallError> {
    for name in [".idea", ".run"] {
        preflight_owned_tree(&root.join(name), &[installation.root.as_path(), user_data_root])?;
    }
    let workspace = VaporSuperworkspace::discover_from(root)
        .map_err(UninstallError::Superworkspace)?;
    for repository in workspace.repositories {
        let repository_root = fs::canonicalize(&repository.root).map_err(|source| {
            UninstallError::Io {
                path: repository.root.clone(),
                source,
            }
        })?;
        if repository_root == root || !repository_root.starts_with(root) {
            return Err(UninstallError::UnsafePurgeRoot {
                path: repository_root,
            });
        }
        preflight_owned_tree(
            &repository_root,
            &[installation.root.as_path(), user_data_root],
        )?;
    }
    Ok(())
}

/// Remove all repositories Vapor recognizes in this registered Superworkspace,
/// while preserving unrelated files and directories beside them.
fn erase_registered_superworkspace(
    root: &Path,
) -> Result<(bool, Vec<PathBuf>), UninstallError> {
    let root = fs::canonicalize(root).map_err(|source| UninstallError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let workspace = VaporSuperworkspace::discover_from(&root)
        .map_err(UninstallError::Superworkspace)?;
    let mut repositories = workspace.repositories;
    // Container repositories can contain registered source repositories as
    // submodules. Remove deepest roots first so deleting the container cannot
    // invalidate paths that the same transaction still needs to process.
    repositories.sort_by_key(|repository| {
        std::cmp::Reverse(repository.root.components().count())
    });
    let owned_metadata = [SUPERWORKSPACE_MANIFEST_FILE_NAME, ".vaporignore"];
    let mut removed = false;
    for repository in repositories {
        let repo = fs::canonicalize(&repository.root).map_err(|source| UninstallError::Io {
            path: repository.root.clone(),
            source,
        })?;
        if repo == root || !repo.starts_with(&root) {
            return Err(UninstallError::UnsafePurgeRoot { path: repo });
        }
        remove_preflighted_tree(&repo)?;
        removed = true;
    }
    for name in owned_metadata {
        let path = root.join(name);
        if path.is_file() {
            fs::remove_file(&path).map_err(|source| UninstallError::Io {
                path: path.clone(),
                source,
            })?;
            removed = true;
        }
    }
    for name in [".idea", ".run"] {
        let path = root.join(name);
        if path.is_dir() {
            let owned_directory = fs::canonicalize(&path).map_err(|source| UninstallError::Io {
                path: path.clone(),
                source,
            })?;
            if owned_directory != root && owned_directory.starts_with(&root) {
                remove_preflighted_tree(&owned_directory)?;
                removed = true;
            }
        }
    }
    for name in ["sources"] {
        let path = root.join(name);
        if path.is_dir() && fs::read_dir(&path).is_ok_and(|mut entries| entries.next().is_none()) {
            fs::remove_dir(&path).map_err(|source| UninstallError::Io {
                path: path.clone(),
                source,
            })?;
        }
    }
    let preserved = fs::read_dir(&root)
        .map_err(|source| UninstallError::Io {
            path: root.clone(),
            source,
        })?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|source| UninstallError::Io {
                    path: root.clone(),
                    source,
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((removed, preserved))
}

fn preflight_owned_tree(
    path: &Path,
    protected: &[&Path],
) -> Result<Option<PathBuf>, UninstallError> {
    preflight_owned_tree_with_managed_child(path, protected, None)
}

fn preflight_owned_tree_with_managed_child(
    path: &Path,
    protected: &[&Path],
    allowed_managed_child: Option<&Path>,
) -> Result<Option<PathBuf>, UninstallError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
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
    if let Some(root) = registered_superworkspace(installation)? {
        return Ok(root);
    }
    let state = source_state(installation).map_err(UninstallError::Source)?;

    if let Ok(current) = env::current_dir()
        && let Ok(superworkspace) = VaporSuperworkspace::discover_from(&current)
    {
        return Ok(superworkspace.root);
    }

    Err(UninstallError::NoActiveSuperworkspace {
        state_path: state.state_path,
    })
}

fn registered_superworkspace(
    installation: &VaporInstallation,
) -> Result<Option<PathBuf>, UninstallError> {
    let state = source_state(installation).map_err(UninstallError::Source)?;

    registered_superworkspace_from_state(&state)
}

fn registered_superworkspace_from_state(
    state: &SourceState,
) -> Result<Option<PathBuf>, UninstallError> {
    if let Some(configured) = state.superworkspace.as_deref()
        && let Ok(superworkspace) = VaporSuperworkspace::discover_from(configured)
    {
        return Ok(Some(superworkspace.root));
    }

    if let Some(active) = state.active.as_deref()
        && let Ok(superworkspace) = VaporSuperworkspace::discover_from(active)
    {
        return Ok(Some(superworkspace.root));
    }

    let mut known = BTreeSet::new();
    for source in &state.known {
        if let Ok(superworkspace) = VaporSuperworkspace::discover_from(source) {
            known.insert(superworkspace.root);
        }
    }

    if known.len() == 1 {
        return Ok(known.into_iter().next());
    }

    if known.len() > 1 {
        return Err(UninstallError::NoActiveSuperworkspace {
            state_path: state.state_path.clone(),
        });
    }
    Ok(None)
}

/// Resolve every still-present registered source path before a permanent wipe.
/// A stale missing registration is harmless; an existing path that cannot be
/// mapped to the registered Superworkspace blocks the wipe for explicit review.
fn registered_superworkspace_for_erasure(
    state: &SourceState,
) -> Result<Option<PathBuf>, UninstallError> {
    let mut roots = BTreeSet::new();
    let mut registered_paths = Vec::new();
    if let Some(path) = &state.superworkspace {
        registered_paths.push(path);
    }
    if let Some(path) = &state.active {
        registered_paths.push(path);
    }
    registered_paths.extend(state.known.iter());

    for registered in registered_paths {
        if !registered.exists() {
            continue;
        }
        match VaporSuperworkspace::discover_from(registered) {
            Ok(workspace) => {
                roots.insert(workspace.root);
            }
            Err(error) => {
                return Err(UninstallError::UnsafeSource {
                    message: format!(
                        "registered source path `{}` could not be assigned to a Superworkspace: {error}",
                        registered.display()
                    ),
                });
            }
        }
    }
    if roots.len() > 1 {
        return Err(UninstallError::NoActiveSuperworkspace {
            state_path: state.state_path.clone(),
        });
    }
    Ok(roots.into_iter().next())
}

fn preview_superworkspace_erasure(
    root: &Path,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>), UninstallError> {
    let workspace = VaporSuperworkspace::discover_from(root).map_err(UninstallError::Superworkspace)?;
    let mut repositories = Vec::new();
    for repository in workspace.repositories {
        let canonical = fs::canonicalize(&repository.root).map_err(|source| UninstallError::Io {
            path: repository.root.clone(),
            source,
        })?;
        if canonical == root || !canonical.starts_with(root) {
            return Err(UninstallError::UnsafePurgeRoot { path: canonical });
        }
        repositories.push(canonical);
    }
    repositories.sort();

    let mut owned_data = Vec::new();
    for name in [SUPERWORKSPACE_MANIFEST_FILE_NAME, ".vaporignore", ".idea", ".run"] {
        let path = root.join(name);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => return Err(UninstallError::Io { path, source }),
        };
        let valid_type = if matches!(name, ".idea" | ".run") {
            metadata.is_dir()
        } else {
            metadata.is_file()
        };
        if metadata.file_type().is_symlink() || !valid_type {
            return Err(UninstallError::UnsafeSuperworkspaceEntry { path });
        }
        preflight_owned_tree(&path, &[])?;
        owned_data.push(path);
    }
    let mut preserved = Vec::new();
    for entry in fs::read_dir(root).map_err(|source| UninstallError::Io {
        path: root.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| UninstallError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let name = entry.file_name();
        if matches!(name.to_str(), Some(SUPERWORKSPACE_MANIFEST_FILE_NAME | ".vaporignore" | ".idea" | ".run"))
            || repositories.iter().any(|repository| repository == &path)
        {
            continue;
        }
        if name == "sources"
            && fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir())
            && fs::read_dir(&path).is_ok_and(|mut entries| entries.next().is_none())
        {
            continue;
        }
        preserved.push(path);
    }
    Ok((repositories, owned_data, preserved))
}

fn preflight_registered_superworkspace_paths(
    root: &Path,
    protected: &[&Path],
) -> Result<(), UninstallError> {
    for name in [".idea", ".run"] {
        preflight_owned_tree(&root.join(name), protected)?;
    }
    let workspace = VaporSuperworkspace::discover_from(root).map_err(UninstallError::Superworkspace)?;
    for repository in workspace.repositories {
        let repository_root = fs::canonicalize(&repository.root).map_err(|source| {
            UninstallError::Io {
                path: repository.root.clone(),
                source,
            }
        })?;
        if repository_root == root || !repository_root.starts_with(root) {
            return Err(UninstallError::UnsafePurgeRoot { path: repository_root });
        }
        preflight_owned_tree(&repository_root, protected)?;
    }
    Ok(())
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

fn remove_all_vapor_user_integration(
    user_data_root: &Path,
) -> Result<Vec<String>, UninstallError> {
    let mut removed = Vec::new();
    if cfg!(windows) {
        if remove_windows_user_vapor_integration(user_data_root)? {
            removed.push("removed Vapor user PATH entries and environment roots".to_owned());
        }
    } else {
        for profile in shell_profiles() {
            if remove_any_managed_profile_block(&profile)? {
                removed.push(format!(
                    "removed managed Vapor PATH block from {}",
                    profile.display()
                ));
            }
        }
    }
    Ok(removed)
}

fn remove_any_managed_profile_block(path: &Path) -> Result<bool, UninstallError> {
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
    let mut inside = false;
    let mut changed = false;
    let mut output = String::new();
    for line in source.lines() {
        if line == PROFILE_BLOCK_START {
            if inside {
                return Err(UninstallError::MalformedProfileBlock {
                    path: path.to_path_buf(),
                });
            }
            inside = true;
            changed = true;
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

#[cfg(not(windows))]
fn remove_managed_profile_blocks_under_root(
    path: &Path,
    user_data_root: &Path,
) -> Result<bool, UninstallError> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(UninstallError::Io { path: path.to_path_buf(), source });
        }
    };
    let canonical_root = fs::canonicalize(user_data_root).unwrap_or_else(|_| user_data_root.to_path_buf());
    let mut output = String::new();
    let mut block = Vec::new();
    let mut inside = false;
    let mut changed = false;
    for line in source.lines() {
        if line == PROFILE_BLOCK_START {
            if inside {
                return Err(UninstallError::MalformedProfileBlock { path: path.to_path_buf() });
            }
            inside = true;
            block.clear();
            block.push(line.to_owned());
            continue;
        }
        if inside {
            block.push(line.to_owned());
            if line == PROFILE_BLOCK_END {
                let targets_product = block.iter().any(|entry| {
                    entry
                        .strip_prefix(&format!("export {VAPOR_HOME_ENV}="))
                        .is_some_and(|value| vapor_home_is_within(value, &canonical_root))
                });
                if targets_product {
                    changed = true;
                } else {
                    for entry in &block {
                        output.push_str(entry);
                        output.push('\n');
                    }
                }
                block.clear();
                inside = false;
            }
            continue;
        }
        output.push_str(line);
        output.push('\n');
    }
    if inside {
        return Err(UninstallError::MalformedProfileBlock { path: path.to_path_buf() });
    }
    if changed {
        fs::write(path, output).map_err(|source| UninstallError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(changed)
}

#[cfg(not(windows))]
fn vapor_home_is_within(value: &str, user_data_root: &Path) -> bool {
    let value = value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .unwrap_or(value)
        .replace("'\\''", "'");
    let path = PathBuf::from(value);
    let path = fs::canonicalize(&path).unwrap_or(path);
    path.starts_with(user_data_root)
}

#[cfg(windows)]
fn remove_managed_profile_blocks_under_root(
    _path: &Path,
    _user_data_root: &Path,
) -> Result<bool, UninstallError> {
    Ok(false)
}

#[cfg(windows)]
fn remove_windows_user_vapor_integration(
    user_data_root: &Path,
) -> Result<bool, UninstallError> {
    let script = r#"
$root = [IO.Path]::GetFullPath($args[0]).TrimEnd([IO.Path]::DirectorySeparatorChar)
function Test-InVaporRoot($value) {
    if ([string]::IsNullOrWhiteSpace($value)) { return $false }
    try { $path = [IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($value)) } catch { return $false }
    return [string]::Equals($path, $root, [StringComparison]::OrdinalIgnoreCase) -or
        $path.StartsWith($root + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)
}
$changed = $false
$current = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($null -ne $current) {
    $entries = @($current -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    $kept = @($entries | Where-Object { -not (Test-InVaporRoot $_) })
    if ($kept.Count -ne $entries.Count) {
        [Environment]::SetEnvironmentVariable('Path', ($kept -join ';'), 'User')
        $changed = $true
    }
}
foreach ($name in @('VAPOR_HOME', 'LOO_CAST_APP_ROOT')) {
    $value = [Environment]::GetEnvironmentVariable($name, 'User')
    if (Test-InVaporRoot $value) {
        [Environment]::SetEnvironmentVariable($name, $null, 'User')
        $changed = $true
    }
}
if ($changed) { Write-Output 'removed' }
"#;
    let output = crate::managed_process::background_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .arg(user_data_root)
        .output()
        .map_err(|source| UninstallError::CommandStart {
            command: "PowerShell Vapor user integration cleanup".to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(UninstallError::CommandFailed {
            command: "PowerShell Vapor user integration cleanup".to_owned(),
            status: output.status.to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).contains("removed"))
}

#[cfg(not(windows))]
fn remove_windows_user_vapor_integration(
    _user_data_root: &Path,
) -> Result<bool, UninstallError> {
    Ok(false)
}

fn legacy_anchor_file() -> Option<PathBuf> {
    if cfg!(windows) {
        env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("AppData/Roaming"))
            })
            .map(|root| root.join("loo_cast/app_root.path"))
    } else {
        env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|root| root.join("loo_cast/app_root.path"))
    }
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
    let bins = bin.to_string_lossy().into_owned();
    let legacy = format!(r"%VAPOR_HOME%\bin\{target}");
    let script = r#"
param([string]$binsEncoded, [string]$legacyEncoded)
$ErrorActionPreference = 'Stop'
$path = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($null -eq $path) { exit 0 }
$entries = @($path -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
$managed = @($binsEncoded -split '\|') + @($legacyEncoded -split '\|')
$filtered = @($entries | Where-Object {
    $entry = $_
    try { $entry = [IO.Path]::GetFullPath($_).TrimEnd([char[]]@([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)) } catch { }
    $match = $false
    foreach ($candidate in $managed) {
        $normalized = $candidate
        try { $normalized = [IO.Path]::GetFullPath($candidate).TrimEnd([char[]]@([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)) } catch { }
        if ([string]::Equals($entry, $normalized, [StringComparison]::OrdinalIgnoreCase) -or
            [string]::Equals($_, $candidate, [StringComparison]::OrdinalIgnoreCase)) { $match = $true; break }
    }
    -not $match
})
if ($filtered.Count -ne $entries.Count) {
    [Environment]::SetEnvironmentVariable('Path', ($filtered -join ';'), 'User')
    try {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class VaporEnvironmentBroadcast {
    [DllImport("user32.dll", CharSet = CharSet.Auto)]
    public static extern IntPtr SendMessageTimeout(IntPtr hWnd, UInt32 message, IntPtr wParam, string lParam, UInt32 flags, UInt32 timeout, out IntPtr result);
}
'@
        $result = [IntPtr]::Zero
        $sent = [VaporEnvironmentBroadcast]::SendMessageTimeout([IntPtr](-1), 0x001A, [IntPtr]::Zero, 'Environment', 0x0002, 5000, [ref]$result)
        if ($sent -eq [IntPtr]::Zero) { Write-Warning 'PATH was removed, but Windows did not notify open applications.' }
    } catch { Write-Warning "PATH was removed, but Windows did not notify open applications: $_" }
    Write-Output 'removed'
} else {
    Write-Output 'unchanged'
}
"#;

    let command = format!(
        "& {{ {script} }} {} {}",
        powershell_literal(&bins),
        powershell_literal(&legacy),
    );
    let output = crate::managed_process::background_command(
        crate::managed_process::windows_powershell_executable(),
    )
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(command)
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

fn powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn remove_windows_user_environment(
    variable: &str,
    installation_root: &Path,
) -> Result<bool, UninstallError> {
    let script = r#"
param([string]$name, [string]$installationRoot)
$expected = [IO.Path]::GetFullPath($installationRoot)
$current = [Environment]::GetEnvironmentVariable($name, 'User')
if ([string]::IsNullOrWhiteSpace($current)) { exit 0 }
try { $actual = [IO.Path]::GetFullPath($current) } catch { exit 0 }
if ([string]::Equals($actual, $expected, [StringComparison]::OrdinalIgnoreCase)) {
    [Environment]::SetEnvironmentVariable($name, $null, 'User')
    Write-Output 'removed'
}
"#;
    let command = format!(
        "& {{ {script} }} {} {}",
        powershell_literal(variable),
        powershell_literal(&crate::install::windows_environment_path(installation_root)),
    );
    let output = crate::managed_process::background_command(
        crate::managed_process::windows_powershell_executable(),
    )
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(command)
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
        PROFILE_BLOCK_END, PROFILE_BLOCK_START, anchor_contents_target_installation,
        erase_registered_superworkspace, managed_profile_block_targets, preflight_owned_tree,
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

    #[test]
    fn erase_all_removes_registered_sources_but_preserves_unrelated_neighbors() {
        let root = std::env::temp_dir().join(format!(
            "vapor-erase-all-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after epoch")
                .as_nanos()
        ));
        let sources = root.join("sources");
        let repo = sources.join("sample-source");
        std::fs::create_dir_all(&repo).expect("create source checkout");
        std::fs::create_dir_all(root.join(".aws")).expect("create unrelated neighbor");
        std::fs::write(root.join("Superworkspace.vapor.toml"), "schema = 1\n\n[superworkspace]\n")
            .expect("write superworkspace manifest");
        std::fs::write(repo.join("Workspace.vapor.toml"), "schema = 1\n\n[workspace]\n")
            .expect("mark source checkout");
        std::fs::write(root.join(".aws/credentials"), "preserve me")
            .expect("write unrelated file");

        let (removed, preserved) = erase_registered_superworkspace(&root).expect("erase sources");

        assert!(removed);
        assert!(!repo.exists());
        assert!(root.join(".aws/credentials").is_file());
        assert!(preserved.contains(&root.join(".aws")));
        assert!(!root.join("Superworkspace.vapor.toml").exists());
        std::fs::remove_dir_all(root).expect("clean fixture");
    }

    #[cfg(unix)]
    #[test]
    fn destructive_preflight_refuses_a_symlink_root_without_following_it() {
        let base = std::env::temp_dir().join(format!(
            "vapor-erase-symlink-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after epoch")
                .as_nanos()
        ));
        let target = base.join("target");
        let link = base.join("registered-root");
        std::fs::create_dir_all(&target).expect("create target");
        std::fs::write(target.join("keep"), "data").expect("create protected data");
        std::os::unix::fs::symlink(&target, &link).expect("create root symlink");

        assert!(matches!(
            preflight_owned_tree(&link, &[]),
            Err(super::UninstallError::UnsafePurgeRoot { .. })
        ));
        assert!(target.join("keep").is_file());
        std::fs::remove_dir_all(base).expect("clean fixture");
    }
}
