//! Canonical Vapor-home storage accounting and narrowly scoped cleanup.
//!
//! Cleanup candidates are discovered from a short allowlist of generated-data
//! locations. Callers select a discovered candidate ID; they never supply a
//! filesystem path to a deletion operation.

use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{VAPOR_HOME_MANIFEST_FILE_NAME, VaporInstallation};

#[derive(Clone, Debug, Serialize)]
/// Complete inventory of the marked Vapor home.
pub struct StorageInventory {
    /// Canonical path of the Vapor home being measured.
    pub home: PathBuf,
    /// Sum of measured logical file sizes in the home.
    pub total_bytes: u64,
    /// Whether any inaccessible files or directories were skipped.
    pub scan_incomplete: bool,
    /// Top-level folders, including important folders retained by cleanup.
    pub areas: Vec<StorageArea>,
    /// Generated data folders that may be removed individually.
    pub candidates: Vec<CleanupCandidate>,
}

#[derive(Clone, Debug, Serialize)]
/// One top-level Vapor-home area and its retention policy.
pub struct StorageArea {
    /// Display name of the area.
    pub name: String,
    /// Path relative to the Vapor home.
    pub relative_path: String,
    /// Measured logical file size.
    pub bytes: u64,
    /// Number of files measured.
    pub files: u64,
    /// Whether the area is retained, mixed, or unclassified.
    pub importance: StorageImportance,
    /// Why routine cleanup retains this area.
    pub explanation: String,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
/// Retention status for a broad storage area.
pub enum StorageImportance {
    /// Runtime, toolchain, user state, or acquired content; never removed here.
    Essential,
    /// Contains separate generated candidates; the broad folder is not deletable.
    Mixed,
    /// Display-only data with no cleanup rule.
    Unclassified,
}

#[derive(Clone, Debug, Serialize)]
/// One generated folder eligible for targeted cleanup.
pub struct CleanupCandidate {
    /// Stable, relative selection token. It is looked up in a fresh scan and is
    /// never interpreted as a path supplied by the caller.
    /// Stable selection token printed by the CLI and shown in the UI.
    pub id: String,
    /// Short item name.
    pub label: String,
    /// Human-readable group, such as build cache or Cargo registry.
    pub category: String,
    /// Path relative to Vapor home.
    pub relative_path: String,
    /// Measured logical file size.
    pub bytes: u64,
    /// Expected rebuild or download cost.
    pub cost: CleanupCost,
    /// User-visible effect of deleting this item.
    pub consequence: String,
    #[serde(skip)]
    path: PathBuf,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
/// Cost that deletion will impose on later use.
pub enum CleanupCost {
    /// Subsequent development builds must regenerate compiled artifacts.
    Rebuild,
    /// Packages or repositories must be downloaded again.
    Redownload,
    /// Generated local product output must be recreated.
    Recreate,
}

#[derive(Clone, Debug, Serialize)]
/// Result of removing one reviewed cleanup candidate.
pub struct CleanupResult {
    /// Selection token of the removed item.
    pub id: String,
    /// Removed path relative to Vapor home.
    pub relative_path: String,
    /// Logical bytes measured immediately before removal; physical space
    /// reclaimed may differ because of compression, sparse files, or links.
    pub measured_bytes_before_delete: u64,
}

#[derive(Default)]
struct TreeSize {
    bytes: u64,
    files: u64,
    incomplete: bool,
}

/// Inspect only the marked, canonical Vapor home and selected generated data.
///
/// # Errors
///
/// Returns an error if the home marker cannot be read or the home root cannot
/// be scanned. Partial failures within child trees are represented by
/// [`StorageInventory::scan_incomplete`].
pub fn inspect(installation: &VaporInstallation) -> Result<StorageInventory, String> {
    let home = validate_home(&installation.root)?;
    let read_entries = fs::read_dir(&home)
        .map_err(|error| format!("cannot read Vapor home '{}': {error}", home.display()))?;
    let mut entries = Vec::new();
    let mut scan_incomplete = false;
    for entry in read_entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                scan_incomplete = true;
                continue;
            }
        };
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(_) => {
                scan_incomplete = true;
                continue;
            }
        };
        if !kind.is_symlink() {
            entries.push((
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            ));
        }
    }
    entries.sort_by(|left, right| left.0.to_lowercase().cmp(&right.0.to_lowercase()));

    let mut areas = Vec::new();
    let mut total_bytes = 0_u64;
    for (name, path) in entries {
        let size = tree_size(&path);
        total_bytes = total_bytes.saturating_add(size.bytes);
        scan_incomplete |= size.incomplete;
        let (importance, explanation) = area_importance(&name);
        areas.push(StorageArea {
            relative_path: name.clone(),
            name,
            bytes: size.bytes,
            files: size.files,
            importance,
            explanation: explanation.to_owned(),
        });
    }

    let mut candidates = Vec::new();
    add_child_candidates(
        &home,
        "cache/build",
        CleanupCost::Rebuild,
        "Close active Cargo builds first. Clears compiled build artifacts and incremental state. Source is kept; the next build will recompile and may take substantially longer.",
        &mut candidates,
        &mut scan_incomplete,
    );
    add_child_candidates(
        &home,
        "cache/repositories",
        CleanupCost::Redownload,
        "Removes this cached repository copy. Vapor will need to fetch it again when requested.",
        &mut candidates,
        &mut scan_incomplete,
    );
    for cache in [
        "tools/cargo/home/registry/cache",
        "tools/cargo/home/registry/src",
        "tools/cargo/home/registry/index",
        "tools/cargo/home/git/checkouts",
        "tools/cargo/home/git/db",
    ] {
        add_child_candidates(
            &home,
            cache,
            CleanupCost::Redownload,
            "Removes cached Cargo package data. Cargo will download it again when needed; network use and setup time will increase.",
            &mut candidates,
            &mut scan_incomplete,
        );
    }
    for output in ["output/dev", "output/release"] {
        add_child_candidates(
            &home,
            output,
            CleanupCost::Recreate,
            "Close any product using this output first. Rebuild or redeploy that product before using the output again.",
            &mut candidates,
            &mut scan_incomplete,
        );
    }
    discover_source_targets(&home, &mut candidates, &mut scan_incomplete);
    candidates.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

    Ok(StorageInventory {
        home,
        total_bytes,
        scan_incomplete,
        areas,
        candidates,
    })
}

/// Remove one current candidate after the caller repeats its exact ID and the
/// size seen during review. No caller-provided path is used for filesystem IO.
/// The implementation rechecks path components and size immediately before
/// deletion. Like other same-user filesystem utilities, it cannot prevent a
/// concurrent same-user process from replacing a parent between that check
/// and the OS path-based recursive delete.
///
/// # Errors
///
/// Returns an error if the confirmation, item identity, or expected size does
/// not match the current inventory, or if filesystem removal fails.
pub fn remove(
    installation: &VaporInstallation,
    id: &str,
    expected_bytes: u64,
    confirmation: &str,
) -> Result<CleanupResult, String> {
    let required_confirmation = format!("DELETE {id}");
    if confirmation != required_confirmation {
        return Err(format!(
            "type '{required_confirmation}' to confirm this deletion"
        ));
    }

    let inventory = inspect(installation)?;
    if inventory.scan_incomplete {
        return Err(
            "storage scan was incomplete; refresh after resolving access errors before deleting"
                .to_owned(),
        );
    }
    let candidate = inventory
        .candidates
        .into_iter()
        .find(|candidate| candidate.id == id)
        .ok_or_else(|| "that cleanup item is no longer available; refresh the list".to_owned())?;
    if candidate.bytes != expected_bytes {
        return Err(format!(
            "the item changed since review (now {} bytes); refresh and review it again",
            candidate.bytes
        ));
    }

    let home = inventory.home;
    let relative = candidate
        .path
        .strip_prefix(&home)
        .map_err(|_| "cleanup item resolved outside Vapor home; nothing was deleted".to_owned())?;
    let mut ancestor = home.clone();
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        ancestor.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&ancestor)
            .map_err(|error| format!("cannot inspect cleanup item: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err("cleanup path contains a symbolic link; nothing was deleted".to_owned());
        }
        if index + 1 < components.len() && !metadata.is_dir() {
            return Err("cleanup path changed type; nothing was deleted".to_owned());
        }
    }
    let metadata = fs::symlink_metadata(&candidate.path)
        .map_err(|error| format!("cannot inspect cleanup item: {error}"))?;
    if !metadata.is_dir() {
        return Err("cleanup item changed type; nothing was deleted".to_owned());
    }
    let target = fs::canonicalize(&candidate.path)
        .map_err(|error| format!("cannot resolve cleanup item: {error}"))?;
    if target == home || !target.starts_with(&home) {
        return Err("cleanup item resolved outside Vapor home; nothing was deleted".to_owned());
    }

    let latest_size = tree_size(&target);
    if latest_size.incomplete {
        return Err("cleanup item became unreadable; nothing was deleted".to_owned());
    }
    if latest_size.bytes != expected_bytes {
        return Err(format!(
            "the item changed during review (now {} bytes); refresh and review it again",
            latest_size.bytes
        ));
    }

    fs::remove_dir_all(&target)
        .map_err(|error| format!("could not remove '{}': {error}", candidate.relative_path))?;
    Ok(CleanupResult {
        id: candidate.id,
        relative_path: candidate.relative_path,
        measured_bytes_before_delete: latest_size.bytes,
    })
}

fn validate_home(root: &Path) -> Result<PathBuf, String> {
    let home = fs::canonicalize(root)
        .map_err(|error| format!("cannot resolve Vapor home '{}': {error}", root.display()))?;
    if !home.join(VAPOR_HOME_MANIFEST_FILE_NAME).is_file() {
        return Err(format!(
            "'{}' is not a marked Vapor home; refusing to scan or delete files there",
            home.display()
        ));
    }
    Ok(home)
}

fn area_importance(name: &str) -> (StorageImportance, &'static str) {
    match name.to_ascii_lowercase().as_str() {
        "cache" | "output" | "sources" => (
            StorageImportance::Mixed,
            "Contains individually listed generated items; this folder itself cannot be deleted here.",
        ),
        "tools" => (
            StorageImportance::Essential,
            "Managed runtimes and toolchains. Removing them can break Vapor or require a large reinstall.",
        ),
        "content" => (
            StorageImportance::Essential,
            "Installed or locally acquired content. Kept by routine cleanup.",
        ),
        "data" | "state" | "config" => (
            StorageImportance::Essential,
            "Vapor state or configuration. Kept by routine cleanup.",
        ),
        "bin" => (
            StorageImportance::Essential,
            "Vapor runtime files. Kept by routine cleanup.",
        ),
        _ => (
            StorageImportance::Unclassified,
            "Unclassified data. It is shown for accounting and never offered for deletion.",
        ),
    }
}

fn add_child_candidates(
    home: &Path,
    relative_parent: &str,
    cost: CleanupCost,
    consequence: &str,
    output: &mut Vec<CleanupCandidate>,
    incomplete: &mut bool,
) {
    let parent = home.join(relative_parent);
    let Ok(parent_metadata) = fs::symlink_metadata(&parent) else {
        return;
    };
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(&parent) else {
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            *incomplete = true;
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            *incomplete = true;
            continue;
        };
        if !kind.is_dir() || kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        let Some(relative) = path.strip_prefix(home).ok() else {
            *incomplete = true;
            continue;
        };
        let relative_path = relative.to_string_lossy().replace('\\', "/");
        let size = tree_size(&path);
        *incomplete |= size.incomplete;
        if size.bytes == 0 {
            continue;
        }
        output.push(CleanupCandidate {
            id: relative_path.clone(),
            label: entry.file_name().to_string_lossy().into_owned(),
            category: candidate_category(relative_parent).to_owned(),
            relative_path,
            bytes: size.bytes,
            cost,
            consequence: consequence.to_owned(),
            path,
        });
    }
}

fn discover_source_targets(home: &Path, output: &mut Vec<CleanupCandidate>, incomplete: &mut bool) {
    let sources = home.join("sources");
    let Ok(metadata) = fs::symlink_metadata(&sources) else {
        return;
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return;
    }
    let mut pending = vec![sources];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                *incomplete = true;
                continue;
            }
        };
        for entry in entries {
            let Ok(entry) = entry else {
                *incomplete = true;
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                *incomplete = true;
                continue;
            };
            if kind.is_symlink() || !kind.is_dir() {
                continue;
            }
            let path = entry.path();
            if entry.file_name() == "target"
                && path
                    .parent()
                    .is_some_and(|parent| parent.join("Cargo.toml").is_file())
            {
                let relative_path = path
                    .strip_prefix(home)
                    .map(|path| path.to_string_lossy().replace('\\', "/"));
                if let Ok(relative_path) = relative_path {
                    let size = tree_size(&path);
                    *incomplete |= size.incomplete;
                    if size.bytes > 0 {
                        output.push(CleanupCandidate {
                        id: relative_path.clone(),
                        label: format!("Rust build output · {}", path.parent().and_then(Path::file_name).unwrap_or_default().to_string_lossy()),
                        category: "Rust build output".to_owned(),
                        relative_path,
                        bytes: size.bytes,
                        cost: CleanupCost::Rebuild,
                        consequence: "Close active Cargo builds first. Deletes Cargo build products and incremental compilation state. Source is preserved; the next build will recompile and may be substantially slower.".to_owned(),
                        path,
                    });
                    }
                }
                continue;
            }
            if !matches!(
                entry.file_name().to_string_lossy().as_ref(),
                ".git" | ".idea" | "node_modules"
            ) {
                pending.push(path);
            }
        }
    }
}

fn candidate_category(relative_parent: &str) -> &'static str {
    match relative_parent {
        "cache/build" => "Build cache",
        "cache/repositories" => "Repository cache",
        "tools/cargo/home/registry/cache"
        | "tools/cargo/home/registry/src"
        | "tools/cargo/home/registry/index" => "Cargo registry cache",
        "tools/cargo/home/git/checkouts" | "tools/cargo/home/git/db" => "Cargo Git cache",
        "output/dev" | "output/release" => "Generated product output",
        _ => "Generated data",
    }
}

fn tree_size(root: &Path) -> TreeSize {
    let mut size = TreeSize::default();
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() => return size,
        Ok(metadata) if metadata.is_file() => {
            size.bytes = metadata.len();
            size.files = 1;
            return size;
        }
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return size,
        Err(_) => {
            size.incomplete = true;
            return size;
        }
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                size.incomplete = true;
                continue;
            }
        };
        for entry in entries {
            let Ok(entry) = entry else {
                size.incomplete = true;
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                size.incomplete = true;
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                match entry.metadata() {
                    Ok(metadata) => {
                        size.bytes = size.bytes.saturating_add(metadata.len());
                        size.files = size.files.saturating_add(1);
                    }
                    Err(_) => size.incomplete = true,
                }
            }
        }
    }
    size
}
