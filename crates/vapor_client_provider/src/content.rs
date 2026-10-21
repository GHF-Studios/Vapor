//! Workshop/content discovery, packaging, installation state, and receipts.
//!
//! The module keeps SteamUGC and filesystem staging behind Vapor content
//! operations. Authored intent is read from source role-specific manifests; generated
//! app-owned state is written under the Steam installation/app root.

use crate::{
    discovery::{EnvironmentPaths, InstallationPaths, ensure_contained},
    manifest::{self, ContentKind, VaporEntity},
    steam,
    workspace::WorkspaceManifest,
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
pub use vapor_client_domain::{
    InstallCandidateAvailability, InstallPlan, InstallPlanDependency, InstallPlanNode,
    InstallPlanSource, SelectedInstallCandidate,
};
use vapor_content::{ContentCandidate, ContentVersionId, DependencySpec};

mod authoring;
mod launch;
mod storage;
mod workshop;
pub use authoring::*;
use authoring::{
    find_local_source, source_candidates, validate_artifact_dependency_roles,
    validate_dependency_edge, validate_source_relationship_cardinality,
};
#[cfg(test)]
use authoring::{rewrite_fork_source, source_manifest_for_fork};
pub use launch::*;
use storage::{
    CacheRecord, ContentIndex, InstalledRecord, acquire_content_index_lock, load_index,
    resolve_installed_selector, save_index, write_lock, write_receipt,
};
pub use workshop::*;
use workshop::{
    WorkshopDownload, is_published_file_id, resolve_download_app_id,
    resolve_live_workshop_download, run_steamcmd_workshop_download,
    run_steamcmd_workshop_downloads,
};

const CONTENT_STATE_SCHEMA: u32 = 1;
const FINGERPRINT_ALGORITHM: &str = "vapor-fnv1a-64-v1";

/// Canonical app-root layout for generated content state.
#[derive(Debug, Clone)]
pub struct ContentLayout {
    app_content: PathBuf,
    steam_downloads: PathBuf,
    cache: PathBuf,
    installed: PathBuf,
    disabled: PathBuf,
    quarantine: PathBuf,
    output_packages: PathBuf,
    output_temporary: PathBuf,
    output_scripts: PathBuf,
    state: PathBuf,
    locks: PathBuf,
    receipts: PathBuf,
    selection: PathBuf,
    index: PathBuf,
}

impl ContentLayout {
    /// Build layout paths under the Steam installation/app root.
    pub fn new(installation: &InstallationPaths) -> Self {
        let app_content = installation.root().join("content");
        let state = installation.state_dir().join("content");
        Self {
            steam_downloads: app_content.join("workshop/downloads"),
            cache: app_content.join("cache"),
            installed: app_content.join("installed"),
            disabled: app_content.join("disabled"),
            quarantine: app_content.join("quarantine"),
            output_packages: installation.root().join("output/content/packages"),
            output_temporary: installation.root().join("output/content/temporary"),
            output_scripts: installation.root().join("output/content/scripts"),
            app_content,
            locks: state.join("locks"),
            receipts: state.join("receipts"),
            selection: state.join("selection.toml"),
            index: state.join("index.toml"),
            state,
        }
    }

    /// Runtime content root.
    pub fn app_content(&self) -> &Path {
        &self.app_content
    }

    /// Provider-observed Steam Workshop download records.
    pub fn steam_downloads(&self) -> &Path {
        &self.steam_downloads
    }

    /// Vapor-managed package cache root.
    pub fn cache(&self) -> &Path {
        &self.cache
    }

    /// Active installed content root.
    pub fn installed(&self) -> &Path {
        &self.installed
    }

    /// Disabled but retained content root.
    pub fn disabled(&self) -> &Path {
        &self.disabled
    }

    /// Corrupted or incomplete content quarantine root.
    pub fn quarantine(&self) -> &Path {
        &self.quarantine
    }

    /// Content package staging output.
    pub fn output_packages(&self) -> &Path {
        &self.output_packages
    }

    /// Temporary staging roots confined to the app-owned content output tree.
    pub fn output_temporary(&self) -> &Path {
        &self.output_temporary
    }

    /// Steam/Workshop script preview output.
    pub fn output_scripts(&self) -> &Path {
        &self.output_scripts
    }

    /// Generated content state root.
    pub fn state(&self) -> &Path {
        &self.state
    }

    /// Generated resolution lock directory.
    pub fn locks(&self) -> &Path {
        &self.locks
    }

    /// Operation receipt directory.
    pub fn receipts(&self) -> &Path {
        &self.receipts
    }

    /// Selected packagepack state path.
    pub fn selection(&self) -> &Path {
        &self.selection
    }

    /// Installed-content index path.
    pub fn index(&self) -> &Path {
        &self.index
    }
}

mod model;
mod packaging;
pub use model::*;
use packaging::package_with_runtime_targets;
use packaging::{
    RuntimeTarget, TemporaryDirectory, read_authored_content, read_deployed_manifest,
    validate_runtime_output_names,
};
pub use packaging::{package, package_for_target, package_for_targets};

/// Validate content dependency, conflict, and Workshop metadata.
///
/// # Errors
///
/// Returns an error when required references or publication fields are invalid.
pub fn validate(
    paths: &EnvironmentPaths,
    selector: Option<&str>,
) -> Result<ValidationReport, String> {
    let catalog = discover(paths)?;
    let selected = select_artifacts(&catalog, selector)?;
    let mut diagnostics = Vec::new();
    let mut errors = Vec::new();

    for artifact in &selected {
        let relationships = artifact
            .dependencies()
            .iter()
            .map(|dependency| dependency.relationship().to_owned())
            .collect::<Vec<_>>();
        if let Err(error) =
            validate_source_relationship_cardinality(artifact.kind(), &relationships)
        {
            errors.push(format!("{}: {error}", artifact.id()));
        }
        let mut seen_dependencies = BTreeSet::new();
        for dependency in artifact.dependencies() {
            if !seen_dependencies.insert((dependency.relationship(), dependency.id())) {
                errors.push(format!(
                    "{} declares duplicate {} reference '{}'",
                    artifact.id(),
                    dependency.relationship(),
                    dependency.id()
                ));
                continue;
            }
            let requirement = match dependency
                .version()
                .map(semver::VersionReq::parse)
                .transpose()
            {
                Ok(requirement) => requirement,
                Err(error) => {
                    errors.push(format!(
                        "{} has an invalid version requirement for {}: {error}",
                        artifact.id(),
                        dependency.id()
                    ));
                    continue;
                }
            };
            let local_child = find_local_source(&catalog, dependency.id(), requirement.as_ref());
            if let Some(child) = local_child {
                if let Err(error) =
                    validate_dependency_edge(artifact.kind(), dependency.relationship(), child)
                {
                    errors.push(error);
                }
            } else if !dependency.optional() && dependency.workshop_id().is_none() {
                errors.push(format!(
                    "{} requires missing {} reference '{}' at version {}",
                    artifact.id(),
                    dependency.relationship(),
                    dependency.id(),
                    dependency.version().unwrap_or("any")
                ));
            } else if dependency.optional() {
                diagnostics.push(format!(
                    "{} has optional {} reference '{}' outside this workspace",
                    artifact.id(),
                    dependency.relationship(),
                    dependency.id()
                ));
            }
            if dependency.optional()
                && matches!(
                    (artifact.kind(), dependency.relationship()),
                    (ContentKind::Packagepack, "engine" | "enginepack")
                        | (ContentKind::Game, "engine")
                        | (ContentKind::Enginepack, "engine")
                        | (ContentKind::Gamepack, "game")
                )
            {
                errors.push(format!(
                    "{} cannot mark its required '{}' relationship optional",
                    artifact.id(),
                    dependency.relationship()
                ));
            }
        }
        for conflict in artifact.conflicts() {
            if conflict.id() == artifact.id() {
                errors.push(format!("{} conflicts with itself", artifact.id()));
            }
        }
        if artifact.workshop().app_id().is_none() {
            diagnostics.push(format!(
                "{} has no [{}.steam].app-id; local lifecycle is available but Workshop publish is not",
                artifact.id(),
                artifact.kind()
            ));
        }
    }

    if errors.is_empty() {
        Ok(ValidationReport {
            checked: selected
                .iter()
                .map(|artifact| artifact.id().to_owned())
                .collect(),
            diagnostics,
        })
    } else {
        Err(errors.join("\n"))
    }
}

/// Acquire content into the app-owned cache.
///
/// Local source artifacts are packaged and copied into the cache. Numeric
/// Workshop IDs install from an existing cache until a live SteamUGC provider is
/// available in the running app.
///
/// # Errors
///
/// Returns provider, discovery, package, or filesystem errors.
pub fn acquire(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
    account: Option<&str>,
) -> Result<AcquireReport, String> {
    let layout = ContentLayout::new(installation);
    if is_published_file_id(selector) && account.is_some_and(|value| !value.trim().is_empty()) {
        let app_id = resolve_download_app_id(installation, paths, selector)?;
        return acquire_workshop_item(installation, app_id, selector, account);
    }

    if let Some(paths) = paths {
        let catalog = discover(paths)?;
        if let Some(artifact) = catalog.find(selector) {
            let package = package(paths, artifact.id(), false)?;
            let cache_key = artifact
                .workshop()
                .published_file_id()
                .map_or_else(|| slug(artifact.id()), ToOwned::to_owned);
            let cache_root = layout.cache().join("packages").join(cache_key);
            reset_directory(installation.root(), &cache_root)?;
            copy_tree(package.root(), &cache_root, package.root(), &[])?;
            let receipt = write_receipt(
                installation,
                "acquire",
                artifact.id(),
                "cached",
                Some(&format!("cache={}", cache_root.display())),
            )?;
            let mut index = load_index(&layout)?;
            index.caches.insert(
                artifact.id().to_owned(),
                CacheRecord {
                    artifact_id: artifact.id().to_owned(),
                    workshop_id: artifact
                        .workshop()
                        .published_file_id()
                        .map(ToOwned::to_owned),
                    version: artifact.version().map(ToOwned::to_owned),
                    cache_root: cache_root.clone(),
                    fingerprint: package.fingerprint().clone(),
                    acquired_at: now_seconds(),
                },
            );
            save_index(&layout, &index)?;
            return Ok(AcquireReport {
                artifact_id: artifact.id().to_owned(),
                cache_root,
                fingerprint: package.fingerprint().clone(),
                receipt,
            });
        }
    }

    let index = load_index(&layout)?;
    if let Some((artifact_id, cache)) = index.cache_by_selector(selector) {
        let receipt = write_receipt(
            installation,
            "acquire",
            &artifact_id,
            "cached-already",
            Some(&format!("cache={}", cache.cache_root.display())),
        )?;
        return Ok(AcquireReport {
            artifact_id,
            cache_root: cache.cache_root.clone(),
            fingerprint: cache.fingerprint.clone(),
            receipt,
        });
    }

    if let Some(seed) = root_content_seed(installation, selector)? {
        return acquire_workshop_item(installation, seed.app_id, &seed.workshop_id, account);
    }

    if is_published_file_id(selector) {
        let app_id = resolve_download_app_id(installation, paths, selector)?;
        return acquire_workshop_item(installation, app_id, selector, account);
    }

    Err(format!(
        "cannot acquire Workshop item '{selector}': no matching source artifact, app-owned cache entry, root content seed, or numeric PublishedFileId exists\nhelp: acquire from an open source artifact, add a [[root.content]] seed, or pass a PublishedFileId"
    ))
}

/// Acquire one or more content items into the app-owned cache.
///
/// Numeric PublishedFileIds are downloaded through one SteamCMD provider
/// session. Mixed source/cache selectors fall back to the single-item
/// acquisition path.
///
/// # Errors
///
/// Returns provider, discovery, package, or filesystem errors.
pub fn acquire_many(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selectors: &[String],
    account: Option<&str>,
) -> Result<Vec<AcquireReport>, String> {
    if selectors.is_empty() {
        return Err("at least one content target is required".to_owned());
    }
    if selectors
        .iter()
        .all(|selector| is_published_file_id(selector))
    {
        let downloads = selectors
            .iter()
            .map(|selector| {
                Ok(WorkshopDownload {
                    app_id: resolve_download_app_id(installation, paths, selector)?,
                    published_file_id: selector.to_owned(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        return acquire_workshop_items(installation, &downloads, account);
    }
    selectors
        .iter()
        .map(|selector| acquire(installation, paths, selector, account))
        .collect()
}

/// Download one or more content items into the app-owned cache.
///
/// An explicit account means the caller wants a live provider refresh when the
/// selector can be resolved to a Workshop item. Other selectors fall back to
/// the ordinary acquisition path.
///
/// # Errors
///
/// Returns provider, discovery, package, or filesystem errors.
pub fn download_many(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selectors: &[String],
    account: Option<&str>,
) -> Result<Vec<AcquireReport>, String> {
    if selectors.is_empty() {
        return Err("at least one content target is required".to_owned());
    }
    if account.unwrap_or("").trim().is_empty() {
        return acquire_many(installation, paths, selectors, account);
    }

    let index = load_index(&ContentLayout::new(installation))?;
    let mut downloads = Vec::new();
    let mut fallback = Vec::new();
    let mut seen = BTreeSet::new();
    for selector in selectors {
        if let Some(download) =
            resolve_live_workshop_download(installation, paths, &index, selector)?
        {
            if seen.insert((download.app_id, download.published_file_id.clone())) {
                downloads.push(download);
            }
        } else {
            fallback.push(selector);
        }
    }

    let mut reports = Vec::new();
    if !downloads.is_empty() {
        reports.extend(acquire_workshop_items(installation, &downloads, account)?);
    }
    for selector in fallback {
        reports.push(acquire(installation, paths, selector, account)?);
    }
    Ok(reports)
}

fn acquire_workshop_item(
    installation: &InstallationPaths,
    app_id: u32,
    published_file_id: &str,
    account: Option<&str>,
) -> Result<AcquireReport, String> {
    let account = workshop_download_account(account);
    let provider_root =
        run_steamcmd_workshop_download(installation, app_id, published_file_id, &account)?;
    import_workshop_item(installation, app_id, published_file_id, &provider_root)
}

fn acquire_workshop_items(
    installation: &InstallationPaths,
    downloads: &[WorkshopDownload],
    account: Option<&str>,
) -> Result<Vec<AcquireReport>, String> {
    let account = workshop_download_account(account);
    let provider_roots = run_steamcmd_workshop_downloads(installation, downloads, &account)?;
    downloads
        .iter()
        .zip(provider_roots.iter())
        .map(|(download, provider_root)| {
            import_workshop_item(
                installation,
                download.app_id,
                &download.published_file_id,
                provider_root,
            )
        })
        .collect()
}

fn workshop_download_account(account: Option<&str>) -> String {
    let account = account.unwrap_or("").trim();
    if account.is_empty() {
        "anonymous".to_owned()
    } else {
        account.to_owned()
    }
}

fn import_workshop_item(
    installation: &InstallationPaths,
    app_id: u32,
    published_file_id: &str,
    provider_root: &Path,
) -> Result<AcquireReport, String> {
    import_workshop_item_with_constraints(
        installation,
        app_id,
        published_file_id,
        provider_root,
        None,
    )
}

fn import_workshop_item_with_constraints(
    installation: &InstallationPaths,
    app_id: u32,
    published_file_id: &str,
    provider_root: &Path,
    constraints: Option<&BTreeMap<String, Vec<String>>>,
) -> Result<AcquireReport, String> {
    let layout = ContentLayout::new(installation);
    let observed_root = layout
        .steam_downloads()
        .join(app_id.to_string())
        .join(published_file_id);
    reset_directory(installation.root(), &observed_root)?;
    copy_tree(provider_root, &observed_root, provider_root, &[])?;
    let artifact = read_deployed_manifest(&observed_root)?;
    if let Some(requirements) = constraints.and_then(|all| all.get(artifact.id())) {
        let version = artifact
            .version()
            .and_then(|version| semver::Version::parse(version).ok())
            .ok_or_else(|| {
                format!(
                    "Workshop update for {} has no valid semantic version; dependent compositions were left unchanged",
                    artifact.id()
                )
            })?;
        for requirement in requirements {
            let requirement = semver::VersionReq::parse(requirement).map_err(|error| {
                format!("saved version requirement '{requirement}' is invalid: {error}")
            })?;
            if !requirement.matches(&version) {
                return Err(format!(
                    "Workshop update {}@{version} does not satisfy every dependent composition; the previous Library copy was preserved",
                    artifact.id()
                ));
            }
        }
    }
    let cache_root = layout.cache().join("packages").join(published_file_id);
    let packages = cache_root
        .parent()
        .ok_or_else(|| "cache package path has no parent directory".to_owned())?;
    fs::create_dir_all(packages).map_err(io("create content package cache", packages))?;
    let _index_lock = acquire_content_index_lock(&layout)?;
    let mut index = load_index(&layout)?;
    if index.caches.values().any(|record| {
        record.workshop_id.as_deref() == Some(published_file_id)
            && record.artifact_id != artifact.id()
    }) {
        return Err(format!(
            "Workshop item {published_file_id} changed its Vapor content ID; the previous Library item was preserved"
        ));
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let staging = packages.join(format!(".{published_file_id}.incoming-{nonce}"));
    let backup = packages.join(format!(".{published_file_id}.previous-{nonce}"));
    if staging.exists() || backup.exists() {
        return Err("could not allocate a temporary Workshop cache transaction".to_owned());
    }
    fs::create_dir(&staging).map_err(io("create Workshop cache staging directory", &staging))?;
    if let Err(error) = copy_tree(&observed_root, &staging, &observed_root, &[]) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    let fingerprint = match fingerprint_tree(&staging) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    let previous = match fs::symlink_metadata(&cache_root) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            if let Err(error) = fs::rename(&cache_root, &backup) {
                let _ = fs::remove_dir_all(&staging);
                return Err(io("stage previous cached artifact", &cache_root)(error));
            }
            true
        }
        Ok(_) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(format!(
                "existing Workshop cache path is not a regular directory: {}",
                cache_root.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(io("inspect previous cached artifact", &cache_root)(error));
        }
    };
    if let Err(error) = fs::rename(&staging, &cache_root) {
        if previous {
            let _ = fs::rename(&backup, &cache_root);
        }
        let _ = fs::remove_dir_all(&staging);
        return Err(io("activate downloaded Workshop artifact", &cache_root)(
            error,
        ));
    }
    index
        .caches
        .retain(|_, record| record.workshop_id.as_deref() != Some(published_file_id));
    index.caches.insert(
        artifact.id().to_owned(),
        CacheRecord {
            artifact_id: artifact.id().to_owned(),
            workshop_id: Some(published_file_id.to_owned()),
            version: artifact.version().map(ToOwned::to_owned),
            cache_root: cache_root.clone(),
            fingerprint: fingerprint.clone(),
            acquired_at: now_seconds(),
        },
    );
    if let Err(error) = save_index(&layout, &index) {
        let _ = fs::remove_dir_all(&cache_root);
        if previous {
            let restore = fs::rename(&backup, &cache_root);
            return Err(match restore {
                Ok(()) => format!("{error}; previous cached artifact restored"),
                Err(restore) => format!("{error}; cache recovery failed: {restore}"),
            });
        }
        return Err(error);
    }
    if previous {
        fs::remove_dir_all(&backup).map_err(io("remove previous cached artifact", &backup))?;
    }
    let receipt = write_receipt(
        installation,
        "download",
        artifact.id(),
        "downloaded",
        Some(&format!(
            "workshop-id={published_file_id}; observed={}; cache={}",
            observed_root.display(),
            cache_root.display()
        )),
    )?;
    Ok(AcquireReport {
        artifact_id: artifact.id().to_owned(),
        cache_root,
        fingerprint,
        receipt,
    })
}

/// Import a Workshop download supplied by Steamworks into Vapor's app-owned cache.
///
/// # Errors
///
/// Returns validation, package, or filesystem errors when the downloaded item
/// cannot be copied and indexed as deployed content.
pub fn import_workshop_item_from_provider(
    installation: &InstallationPaths,
    app_id: u32,
    published_file_id: &str,
    provider_root: &Path,
) -> Result<AcquireReport, String> {
    import_workshop_item(installation, app_id, published_file_id, provider_root)
}

/// Import a Workshop item only when its version satisfies all supplied
/// Auto-Install composition constraints. A rejected update leaves the previous
/// Library copy intact.
pub fn import_workshop_item_from_provider_with_constraints(
    installation: &InstallationPaths,
    app_id: u32,
    published_file_id: &str,
    provider_root: &Path,
    constraints: &BTreeMap<String, Vec<String>>,
) -> Result<AcquireReport, String> {
    import_workshop_item_with_constraints(
        installation,
        app_id,
        published_file_id,
        provider_root,
        Some(constraints),
    )
}

/// Install source or cached content into the app-owned installed-content tree.
///
/// # Errors
///
/// Returns dependency, conflict, provider, verification, or filesystem errors.
pub fn install(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
) -> Result<Vec<InstallReport>, String> {
    install_with_account(installation, paths, selector, None)
}

/// Install source or cached content for one runtime target.
///
/// # Errors
///
/// Returns dependency, conflict, provider, verification, or filesystem errors.
pub fn install_for_target(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
    target_triple: Option<&str>,
) -> Result<Vec<InstallReport>, String> {
    install_with_account_for_target(installation, paths, selector, None, target_triple)
}

/// Install source or cached content for one or more runtime targets.
///
/// # Errors
///
/// Returns dependency, conflict, provider, verification, or filesystem errors.
pub fn install_for_targets(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
    target_triples: &[String],
) -> Result<Vec<InstallReport>, String> {
    install_with_account_for_targets(installation, paths, selector, None, target_triples)
}

/// Install source or cached content, downloading Workshop dependencies with an account when needed.
///
/// # Errors
///
/// Returns dependency, conflict, provider, verification, or filesystem errors.
pub fn install_with_account(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
    account: Option<&str>,
) -> Result<Vec<InstallReport>, String> {
    install_with_account_for_target(installation, paths, selector, account, None)
}

/// Install source or cached content with an explicit runtime target.
///
/// # Errors
///
/// Returns dependency, conflict, provider, verification, or filesystem errors.
pub fn install_with_account_for_target(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
    account: Option<&str>,
    target_triple: Option<&str>,
) -> Result<Vec<InstallReport>, String> {
    let targets = target_triple
        .map(|target| vec![target.to_owned()])
        .unwrap_or_default();
    install_with_account_for_targets(installation, paths, selector, account, &targets)
}

/// Install source or cached content with explicit runtime targets.
///
/// # Errors
///
/// Returns dependency, conflict, provider, verification, or filesystem errors.
pub fn install_with_account_for_targets(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
    account: Option<&str>,
    target_triples: &[String],
) -> Result<Vec<InstallReport>, String> {
    let runtime_targets = RuntimeTarget::many(target_triples)?;
    let layout = ContentLayout::new(installation);
    let catalog = paths.map(discover).transpose()?;
    let mut index = load_index(&layout)?;
    let mut reports = Vec::new();
    let mut visiting = BTreeSet::new();
    let context = InstallContext {
        installation,
        paths,
        catalog: catalog.as_ref(),
        layout: &layout,
        account,
        runtime_targets: &runtime_targets,
    };
    let mut state = InstallState {
        index: &mut index,
        visiting: &mut visiting,
        reports: &mut reports,
    };
    install_selector(
        &context,
        InstallSelectorRequest {
            selector,
            expected_id: None,
            parent_kind: None,
            relationship: None,
            require_workspace: false,
        },
        &mut state,
    )?;
    save_index(&layout, &index)?;
    Ok(reports)
}

/// Update installed content by reinstalling it from source or cache.
///
/// # Errors
///
/// Returns the same errors as [`install`].
pub fn update(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: Option<&str>,
) -> Result<Vec<InstallReport>, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    let targets = if let Some(selector) = selector {
        vec![resolve_installed_selector(&index, selector)?]
    } else {
        index.installed.keys().cloned().collect()
    };
    if targets.is_empty() {
        return Err("no installed content to update".to_owned());
    }
    let mut reports = Vec::new();
    for target in targets {
        reports.extend(install(installation, paths, &target)?);
    }
    Ok(reports)
}

/// Disable installed content without deleting its artifact root.
///
/// # Errors
///
/// Returns an error when the target is unknown or the move fails.
pub fn disable(installation: &InstallationPaths, selector: &str) -> Result<InstallReport, String> {
    move_enabled_state(installation, selector, false)
}

/// Enable disabled content without reinstalling it.
///
/// # Errors
///
/// Returns an error when the target is unknown or the move fails.
pub fn enable(installation: &InstallationPaths, selector: &str) -> Result<InstallReport, String> {
    move_enabled_state(installation, selector, true)
}

/// Uninstall content and remove generated installed-state records.
///
/// # Errors
///
/// Returns an error when generated state cannot be updated.
pub fn uninstall(
    installation: &InstallationPaths,
    selector: &str,
) -> Result<UninstallReport, String> {
    let layout = ContentLayout::new(installation);
    let mut index = load_index(&layout)?;
    let artifact_id = resolve_installed_selector(&index, selector)?;
    let mut removed = false;
    for root in [
        layout.installed().join(&artifact_id),
        layout.disabled().join(&artifact_id),
    ] {
        if root.exists() {
            remove_directory(installation.root(), &root)?;
            removed = true;
        }
    }
    index.installed.remove(&artifact_id);
    save_index(&layout, &index)?;
    if current_selection(installation)?
        .as_ref()
        .is_some_and(|selection| selection.artifact_id() == artifact_id)
    {
        clear_selection(installation)?;
    }
    let receipt = write_receipt(
        installation,
        "uninstall",
        &artifact_id,
        if removed { "removed" } else { "not-present" },
        None,
    )?;
    Ok(UninstallReport {
        artifact_id,
        removed,
        receipt,
    })
}

/// Select an installed packagepack for play.
///
/// # Errors
///
/// Returns an error when the target is not installed, not enabled, not a
/// packagepack, or the selection cannot be written.
pub fn select_packagepack(
    installation: &InstallationPaths,
    selector: &str,
) -> Result<PackagepackSelection, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    let artifact_id = resolve_installed_selector(&index, selector)?;
    let record = index
        .installed
        .get(&artifact_id)
        .ok_or_else(|| format!("content is not installed: {artifact_id}"))?;
    if record.kind != ContentKind::Packagepack.to_string() {
        return Err(format!(
            "selected content must be a packagepack: {} is {}",
            artifact_id, record.kind
        ));
    }
    if !record.enabled {
        return Err(format!("cannot select disabled packagepack: {artifact_id}"));
    }
    if !record.installed_root.exists() {
        return Err(format!(
            "cannot select packagepack with missing artifact root: {}",
            record.installed_root.display()
        ));
    }
    let selection = PackagepackSelection {
        schema_version: CONTENT_STATE_SCHEMA,
        artifact_id: artifact_id.clone(),
        installed_root: record.installed_root.clone(),
        fingerprint: record.fingerprint.clone(),
        selected_at: now_seconds(),
    };
    if let Some(parent) = layout.selection().parent() {
        fs::create_dir_all(parent).map_err(io("create content selection parent", parent))?;
    }
    let encoded = toml::to_string_pretty(&selection)
        .map_err(|error| format!("failed to encode packagepack selection: {error}"))?;
    fs::write(layout.selection(), encoded)
        .map_err(io("write packagepack selection", layout.selection()))?;
    write_receipt(
        installation,
        "select",
        &artifact_id,
        "selected",
        Some(&format!("root={}", selection.installed_root.display())),
    )?;
    Ok(selection)
}

/// Read the current selected packagepack.
///
/// # Errors
///
/// Returns filesystem or TOML parse errors.
pub fn current_selection(
    installation: &InstallationPaths,
) -> Result<Option<PackagepackSelection>, String> {
    let layout = ContentLayout::new(installation);
    if !layout.selection().is_file() {
        return Ok(None);
    }
    let source = fs::read_to_string(layout.selection())
        .map_err(io("read packagepack selection", layout.selection()))?;
    toml::from_str(&source).map(Some).map_err(|error| {
        format!(
            "failed to parse '{}': {error}",
            layout.selection().display()
        )
    })
}

/// Resolve an app manifest launch alias to its packagepack content ID.
///
/// # Errors
///
/// Returns an error when the app manifest has no matching launch alias or the
/// alias does not point at a packagepack seed.
pub fn default_packagepack_id(
    installation: &InstallationPaths,
    launch: &str,
) -> Result<String, String> {
    let seed = root_content_seed(installation, launch)?
        .ok_or_else(|| format!("app manifest has no default packagepack launch '{launch}'"))?;
    let expected_kind = ContentKind::Packagepack.to_string();
    if seed.kind != expected_kind {
        return Err(format!(
            "app manifest default launch '{launch}' must point at a packagepack, but '{}' is {}",
            seed.id, seed.kind
        ));
    }
    Ok(seed.id)
}

/// Clear the selected packagepack.
///
/// # Errors
///
/// Returns filesystem errors.
pub fn clear_selection(installation: &InstallationPaths) -> Result<(), String> {
    let layout = ContentLayout::new(installation);
    if layout.selection().exists() {
        fs::remove_file(layout.selection())
            .map_err(io("remove packagepack selection", layout.selection()))?;
    }
    write_receipt(installation, "deselect", "packagepack", "cleared", None)?;
    Ok(())
}

/// Verify installed content against app-owned receipts and indexes.
///
/// # Errors
///
/// Returns an error when state cannot be read.
pub fn verify(
    installation: &InstallationPaths,
    selector: Option<&str>,
) -> Result<Vec<VerifyReport>, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    let targets = if let Some(selector) = selector {
        vec![resolve_installed_selector(&index, selector)?]
    } else {
        index.installed.keys().cloned().collect()
    };
    Ok(targets
        .into_iter()
        .map(|artifact_id| verify_one(&index, &artifact_id))
        .collect())
}

/// Repair corrupted installed content by reinstalling from source or cache.
///
/// # Errors
///
/// Returns provider or filesystem errors when repair cannot proceed.
pub fn repair(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: Option<&str>,
) -> Result<Vec<InstallReport>, String> {
    let layout = ContentLayout::new(installation);
    let reports = verify(installation, selector)?;
    let mut repaired = Vec::new();
    for report in reports {
        if report.ok() {
            continue;
        }
        let corrupt = layout.installed().join(report.artifact_id());
        if corrupt.exists() {
            let quarantine = layout.quarantine().join(format!(
                "{}-{}",
                slug(report.artifact_id()),
                now_seconds()
            ));
            if let Some(parent) = quarantine.parent() {
                fs::create_dir_all(parent).map_err(io("create quarantine parent", parent))?;
            }
            fs::rename(&corrupt, &quarantine).map_err(io("quarantine content", &corrupt))?;
            write_receipt(
                installation,
                "repair",
                report.artifact_id(),
                "quarantined",
                Some(&format!("quarantine={}", quarantine.display())),
            )?;
        }
        repaired.extend(install(installation, paths, report.artifact_id())?);
    }
    if repaired.is_empty() {
        let target = selector.unwrap_or("all");
        write_receipt(installation, "repair", target, "already-ok", None)?;
    }
    Ok(repaired)
}

/// One exact artifact currently recorded in the app-owned installation index.
#[derive(Debug, Clone)]
pub struct InstalledArtifactSnapshot {
    artifact_id: String,
    name: Option<String>,
    version: Option<String>,
    kind: String,
    workshop_id: Option<String>,
    installed_root: PathBuf,
    fingerprint: Fingerprint,
    source: String,
    enabled: bool,
    dependencies: Vec<ContentReference>,
}

impl InstalledArtifactSnapshot {
    /// Installed content ID.
    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }

    /// Human-readable name authored by the content publisher, when present.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Installed exact version, when declared.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Rust-defined Vapor content kind.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Workshop PublishedFileId associated with the acquired or installed item.
    pub fn workshop_id(&self) -> Option<&str> {
        self.workshop_id.as_deref()
    }

    /// App-owned installed content directory.
    pub fn installed_root(&self) -> &Path {
        &self.installed_root
    }

    /// Fingerprint recorded for the installed tree.
    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// Acquisition/install source recorded by Vapor (for example `cache` or
    /// `source-package`).
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Whether this record is currently enabled.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Declared dependency and conflict edges from the installed manifest.
    pub fn dependencies(&self) -> &[ContentReference] {
        &self.dependencies
    }
}

/// Check whether an exact installed selector is present in app-owned state.
///
/// # Errors
///
/// Returns filesystem or TOML errors while reading the content index.
pub fn has_installed_artifact(
    installation: &InstallationPaths,
    selector: &str,
) -> Result<bool, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    Ok(resolve_installed_selector(&index, selector).is_ok())
}

/// Check whether an artifact is already present in Vapor's downloadable Library.
///
/// This does not read the package manifest, so callers can distinguish an absent
/// cache entry from a present but damaged one before inspecting its contents.
pub fn has_cached_artifact(
    installation: &InstallationPaths,
    selector: &str,
) -> Result<bool, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    Ok(index.cache_by_selector(selector).is_some())
}

/// Read one deployed artifact from Vapor's app-owned cache.
pub fn cached_artifact(
    installation: &InstallationPaths,
    selector: &str,
) -> Result<InstalledArtifactSnapshot, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    let (artifact_id, cached) = index
        .cache_by_selector(selector)
        .ok_or_else(|| format!("content is not in the app-owned cache: {selector}"))?;
    let artifact = read_deployed_manifest(&cached.cache_root)?;
    if artifact.id() != artifact_id {
        return Err(format!(
            "cached manifest identity {} does not match cache index {artifact_id}",
            artifact.id()
        ));
    }
    Ok(InstalledArtifactSnapshot {
        artifact_id,
        name: Some(artifact.name().to_owned()),
        version: artifact.version().map(ToOwned::to_owned),
        kind: artifact.kind().to_string(),
        workshop_id: cached.workshop_id.clone(),
        installed_root: cached.cache_root.clone(),
        fingerprint: cached.fingerprint.clone(),
        source: "cache".to_owned(),
        enabled: true,
        dependencies: artifact.dependencies().to_vec(),
    })
}

/// Check whether the cached version of an artifact satisfies an optional
/// semantic-version requirement.
pub fn cached_artifact_satisfies_version(
    installation: &InstallationPaths,
    selector: &str,
    requirement: Option<&str>,
) -> Result<bool, String> {
    let cached = cached_artifact(installation, selector)?;
    let requirement = requirement
        .map(semver::VersionReq::parse)
        .transpose()
        .map_err(|error| format!("invalid version requirement for {selector}: {error}"))?;
    Ok(version_satisfies(cached.version(), requirement.as_ref()))
}

/// Remove one acquired artifact from the local Library cache without touching
/// its installed copy. Callers must establish that no composition depends on
/// the item before requesting collection.
pub fn collect_cached_artifact(
    installation: &InstallationPaths,
    content_id: &str,
) -> Result<bool, String> {
    let layout = ContentLayout::new(installation);
    let _lock = acquire_content_index_lock(&layout)?;
    let mut index = load_index(&layout)?;
    let Some(record) = index.caches.get(content_id).cloned() else {
        return Ok(false);
    };

    let packages = layout.cache().join("packages");
    fs::create_dir_all(&packages).map_err(io("create content cache", &packages))?;
    let packages_root =
        fs::canonicalize(&packages).map_err(io("resolve content cache", &packages))?;
    let record_metadata = fs::symlink_metadata(&record.cache_root)
        .map_err(io("inspect cached artifact", &record.cache_root))?;
    if !record_metadata.file_type().is_dir() || record_metadata.file_type().is_symlink() {
        return Err(format!(
            "cached artifact path is not a regular directory: {}",
            record.cache_root.display()
        ));
    }
    let cache_root = fs::canonicalize(&record.cache_root)
        .map_err(io("resolve cached artifact", &record.cache_root))?;
    if !cache_root.starts_with(&packages_root) || cache_root == packages_root {
        return Err(format!(
            "cached artifact resolves outside the managed content cache: {}",
            record.cache_root.display()
        ));
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let quarantine = packages.join(format!(".collect-{}-{nonce}", slug(content_id)));
    fs::rename(&cache_root, &quarantine)
        .map_err(io("stage cached artifact collection", &cache_root))?;
    index.caches.remove(content_id);
    if let Err(error) = save_index(&layout, &index) {
        let restore = fs::rename(&quarantine, &cache_root);
        return Err(match restore {
            Ok(()) => format!("{error}; cached artifact restored"),
            Err(restore) => format!("{error}; cached artifact recovery failed: {restore}"),
        });
    }
    fs::remove_dir_all(&quarantine).map_err(io("collect cached artifact", &quarantine))?;
    Ok(true)
}

/// Read exact installed artifact records from the app-owned index.
///
/// Selectors may be content IDs or exact `id@version` identities. Every selector
/// must resolve; a missing graph node is returned as an error.
///
/// # Errors
///
/// Returns filesystem, TOML, or missing-artifact errors.
pub fn installed_artifacts(
    installation: &InstallationPaths,
    selectors: &[String],
) -> Result<Vec<InstalledArtifactSnapshot>, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    selectors
        .iter()
        .map(|selector| {
            let key = resolve_installed_selector(&index, selector)?;
            let record = index
                .installed
                .get(&key)
                .ok_or_else(|| format!("content is not installed: {selector}"))?;
            let artifact = read_deployed_manifest(&record.installed_root)?;
            Ok(InstalledArtifactSnapshot {
                artifact_id: record.artifact_id.clone(),
                name: Some(artifact.name().to_owned()),
                version: record.version.clone(),
                kind: record.kind.clone(),
                workshop_id: record.workshop_id.clone(),
                installed_root: record.installed_root.clone(),
                fingerprint: record.fingerprint.clone(),
                source: record.source.clone(),
                enabled: record.enabled,
                dependencies: record.dependencies.clone(),
            })
        })
        .collect()
}

fn installed_snapshot_from_record(record: &InstalledRecord) -> InstalledArtifactSnapshot {
    InstalledArtifactSnapshot {
        artifact_id: record.artifact_id.clone(),
        name: None,
        version: record.version.clone(),
        kind: record.kind.clone(),
        workshop_id: record.workshop_id.clone(),
        installed_root: record.installed_root.clone(),
        fingerprint: record.fingerprint.clone(),
        source: record.source.clone(),
        enabled: record.enabled,
        dependencies: record.dependencies.clone(),
    }
}

/// Read the app-owned content index.
///
/// # Errors
///
/// Returns filesystem or TOML parse errors.
pub fn installed_index(installation: &InstallationPaths) -> Result<Vec<String>, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    Ok(index.installed.keys().cloned().collect())
}

/// Read the app-owned cache's content identities for catalog discovery.
///
/// # Errors
///
/// Returns filesystem or TOML errors while reading the content index.
pub fn cached_index(installation: &InstallationPaths) -> Result<Vec<String>, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    Ok(index.caches.keys().cloned().collect())
}

/// Resolve a composition without packaging, downloading, installing, or writing state.
///
/// The selection order follows the shared install behavior: workspace source is
/// preferred for authored roots, compatible installed dependencies are reused,
/// then compatible workspace/cache content is selected. First-party seeds that
/// have not been acquired are reported as blockers because their dependency
/// manifests are not yet locally inspectable.
///
/// # Errors
///
/// Returns source, app-owned index, or cached-manifest inspection errors.
pub fn preview_install(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
) -> Result<InstallPlan, String> {
    let catalog = paths.map(discover).transpose()?;
    let layout = ContentLayout::new(installation);
    let mut provider = LocalInstallProvider {
        installation,
        catalog: catalog.as_ref(),
        index: load_index(&layout)?,
        records: BTreeMap::new(),
    };
    vapor_client_domain::resolve_install_plan(selector, &mut provider)
}

struct LocalInstallProvider<'a> {
    installation: &'a InstallationPaths,
    catalog: Option<&'a ContentCatalog>,
    index: ContentIndex,
    records: BTreeMap<ContentVersionId, ContentArtifact>,
}

impl LocalInstallProvider<'_> {
    fn retain_artifact(
        &mut self,
        artifact: ContentArtifact,
        source: InstallPlanSource,
        already_installed: bool,
    ) -> Result<InstallCandidateAvailability, String> {
        let candidates = source_candidates(std::slice::from_ref(&artifact))?;
        let Some(candidate) = candidates.iter().next().cloned() else {
            return Ok(InstallCandidateAvailability::Unavailable {
                message: Some(format!(
                    "content '{}' has no valid exact semantic version and cannot join a resolved composition",
                    artifact.id(),
                )),
                warning: false,
                seed: None,
            });
        };
        let identity = candidate.identity.clone();
        let artifact = self
            .records
            .entry(identity.clone())
            .or_insert(artifact)
            .clone();
        let conflicts = artifact
            .conflicts()
            .iter()
            .map(|conflict| conflict.id().to_owned())
            .collect::<Vec<_>>();
        let enabled_conflicts = conflicts
            .iter()
            .filter(|id| {
                self.index
                    .installed
                    .get(id.as_str())
                    .is_some_and(|installed| installed.enabled)
            })
            .cloned()
            .collect();
        Ok(InstallCandidateAvailability::Selected(
            SelectedInstallCandidate {
                candidate,
                plan_node: InstallPlanNode {
                    id: artifact.id().to_owned(),
                    version: artifact.version().map(ToOwned::to_owned),
                    kind: artifact.kind().to_string(),
                    published_file_id: artifact
                        .workshop()
                        .published_file_id()
                        .map(ToOwned::to_owned),
                    source,
                    already_installed,
                    dependencies: artifact
                        .dependencies()
                        .iter()
                        .map(|dependency| InstallPlanDependency {
                            id: dependency.id().to_owned(),
                            relationship: dependency.relationship().to_owned(),
                            version: dependency.version().map(ToOwned::to_owned),
                            optional: dependency.optional(),
                        })
                        .collect(),
                },
                conflicts,
                enabled_conflicts,
            },
        ))
    }
}

impl vapor_client_domain::InstallContentProvider for LocalInstallProvider<'_> {
    fn select_root(&mut self, selector: &str) -> Result<InstallCandidateAvailability, String> {
        let installed_key = resolve_installed_selector(&self.index, selector).ok();
        let installed = installed_key
            .as_ref()
            .and_then(|key| self.index.installed.get(key))
            .cloned();

        if let Some(artifact) = self
            .catalog
            .and_then(|catalog| catalog.find(selector))
            .cloned()
        {
            return self.retain_artifact(artifact, InstallPlanSource::Workspace, false);
        }

        if let Some((_, cached)) = self.index.cache_by_selector(selector) {
            let artifact = read_deployed_manifest(&cached.cache_root)?;
            return self.retain_artifact(artifact, InstallPlanSource::Cache, false);
        }

        if let Some(installed) = installed.as_ref()
            && installed.enabled
        {
            let artifact = read_deployed_manifest(&installed.installed_root)?;
            return self.retain_artifact(artifact, InstallPlanSource::Installed, true);
        }

        if let Some(seed) = root_content_seed(self.installation, selector)? {
            return Ok(InstallCandidateAvailability::Unavailable {
                message: Some(format!(
                    "{} is supplied by Workshop item {}, but its manifest must be acquired before its dependencies can be resolved",
                    seed.id, seed.workshop_id,
                )),
                warning: false,
                seed: Some(seed_plan_node(seed)),
            });
        }

        if let Some(installed) = installed.as_ref()
            && !installed.enabled
        {
            return Ok(InstallCandidateAvailability::Unavailable {
                message: Some(format!(
                    "{} is installed but disabled, and no compatible source or cached version is available",
                    installed.artifact_id,
                )),
                warning: false,
                seed: None,
            });
        }

        Ok(InstallCandidateAvailability::Unavailable {
            message: Some(format!(
                "required root '{}' is unavailable in the workspace, cache, or enabled installation",
                selector,
            )),
            warning: false,
            seed: None,
        })
    }

    fn select_dependency(
        &mut self,
        parent: &ContentVersionId,
        parent_source: InstallPlanSource,
        binding: &str,
        requirement: &DependencySpec,
    ) -> Result<InstallCandidateAvailability, String> {
        let Some(parent_artifact) = self.records.get(parent) else {
            return Err(format!("resolved parent '{parent}' has no provider record"));
        };
        let Some((_, index)) = binding.rsplit_once('#') else {
            return Err(format!(
                "dependency binding '{binding}' has no source index"
            ));
        };
        let index = index
            .parse::<usize>()
            .map_err(|error| format!("invalid dependency binding '{binding}': {error}"))?;
        let Some(reference) = parent_artifact.dependencies().get(index).cloned() else {
            return Err(format!(
                "dependency binding '{binding}' is not present in its authored artifact"
            ));
        };
        if reference.id() != requirement.id.as_str() {
            return Err(format!(
                "dependency binding '{binding}' does not match its shared identity"
            ));
        }
        let local_closure = parent_source == InstallPlanSource::Workspace;
        let id = requirement.id.as_str();
        let installed = self.index.installed.get(id).cloned();

        if !local_closure
            && let Some(installed) = installed.as_ref()
            && installed.enabled
            && version_satisfies(installed.version.as_deref(), Some(&requirement.version))
        {
            let artifact = read_deployed_manifest(&installed.installed_root)?;
            return self.retain_artifact(artifact, InstallPlanSource::Installed, true);
        }

        if let Some(artifact) = self
            .catalog
            .and_then(|catalog| catalog.satisfying_version(id, Some(&requirement.version)))
            .cloned()
        {
            return self.retain_artifact(artifact, InstallPlanSource::Workspace, false);
        }

        if local_closure {
            return Ok(InstallCandidateAvailability::Unavailable {
                message: Some(format!(
                    "local content requires every descendant to be authored in this workspace, but '{}' has no compatible local source",
                    id,
                )),
                warning: false,
                seed: None,
            });
        }

        let cache_selector = reference.workshop_id().unwrap_or(id);
        if let Some((_, cached)) = self.index.cache_by_selector(cache_selector) {
            let artifact = read_deployed_manifest(&cached.cache_root)?;
            if artifact.id() != id {
                return Ok(InstallCandidateAvailability::Unavailable {
                    message: Some(format!(
                        "Workshop/cache selector for '{}' resolved to '{}'; dependency identity must match exactly",
                        id,
                        artifact.id(),
                    )),
                    warning: false,
                    seed: None,
                });
            }
            if version_satisfies(artifact.version(), Some(&requirement.version)) {
                let already_installed =
                    self.index
                        .installed
                        .get(artifact.id())
                        .is_some_and(|record| {
                            record.enabled && record.version.as_deref() == artifact.version()
                        });
                return self.retain_artifact(artifact, InstallPlanSource::Cache, already_installed);
            }
        }

        if let Some(installed) = installed {
            if !installed.enabled {
                return Ok(InstallCandidateAvailability::Unavailable {
                    message: Some(format!(
                        "{} is installed but disabled, and no compatible source or cached version is available",
                        installed.artifact_id,
                    )),
                    warning: false,
                    seed: None,
                });
            }
            return Ok(InstallCandidateAvailability::Unavailable {
                message: Some(format!(
                    "{} {} does not satisfy required version {}; no compatible source or cached version is available",
                    installed.artifact_id,
                    installed.version.as_deref().unwrap_or("(unversioned)"),
                    reference.version().unwrap_or("any"),
                )),
                warning: false,
                seed: None,
            });
        }

        let seed_selector = reference.workshop_id().unwrap_or(id);
        let seed = match root_content_seed(self.installation, seed_selector)? {
            Some(seed) => Some(seed),
            None if seed_selector != id => root_content_seed(self.installation, id)?,
            None => None,
        };
        if let Some(seed) = seed {
            if seed.id != id {
                return Ok(InstallCandidateAvailability::Unavailable {
                    message: Some(format!(
                        "Workshop seed {} resolves to '{}', but dependency identity must be '{}'",
                        seed.workshop_id, seed.id, id,
                    )),
                    warning: false,
                    seed: None,
                });
            }
            let message = format!(
                "{} is supplied by a Workshop item, but its manifest must be acquired before its dependencies can be resolved",
                id,
            );
            return Ok(InstallCandidateAvailability::Unavailable {
                message: Some(message),
                warning: false,
                seed: Some(seed_plan_node(seed)),
            });
        }

        Ok(InstallCandidateAvailability::Unavailable {
            message: Some(format!(
                "required {} '{}' at {} is unavailable in the workspace, cache, or enabled installation",
                reference.relationship(),
                id,
                reference.version().unwrap_or("any version"),
            )),
            warning: reference.optional(),
            seed: None,
        })
    }
}

fn seed_plan_node(seed: RootContentSeed) -> InstallPlanNode {
    InstallPlanNode {
        id: seed.id,
        version: None,
        kind: seed.kind,
        published_file_id: Some(seed.workshop_id),
        source: InstallPlanSource::RootSeed,
        already_installed: false,
        dependencies: Vec::new(),
    }
}

struct InstallContext<'a> {
    installation: &'a InstallationPaths,
    paths: Option<&'a EnvironmentPaths>,
    catalog: Option<&'a ContentCatalog>,
    layout: &'a ContentLayout,
    account: Option<&'a str>,
    runtime_targets: &'a [RuntimeTarget],
}

struct InstallSelectorRequest<'a> {
    selector: &'a str,
    expected_id: Option<&'a str>,
    parent_kind: Option<ContentKind>,
    relationship: Option<&'a str>,
    require_workspace: bool,
}

struct InstallState<'a> {
    index: &'a mut ContentIndex,
    visiting: &'a mut BTreeSet<String>,
    reports: &'a mut Vec<InstallReport>,
}

fn version_satisfies(version: Option<&str>, requirement: Option<&semver::VersionReq>) -> bool {
    let Some(requirement) = requirement else {
        return true;
    };
    version
        .and_then(|version| semver::Version::parse(version).ok())
        .is_some_and(|version| requirement.matches(&version))
}

fn install_selector(
    context: &InstallContext<'_>,
    request: InstallSelectorRequest<'_>,
    state: &mut InstallState<'_>,
) -> Result<(), String> {
    let InstallSelectorRequest {
        selector,
        expected_id,
        parent_kind,
        relationship,
        require_workspace,
    } = request;
    if let Some(catalog) = context.catalog
        && let Some(artifact) = catalog.find(selector)
    {
        if expected_id.is_some_and(|expected| artifact.id() != expected) {
            return Err(format!(
                "dependency selector '{selector}' resolved to '{}', expected '{}'",
                artifact.id(),
                expected_id.unwrap_or_default()
            ));
        }
        if let (Some(parent_kind), Some(relationship)) = (parent_kind, relationship) {
            validate_dependency_edge(parent_kind, relationship, artifact)?;
        }
        validate_artifact_dependency_roles(artifact)?;
        if !state.visiting.insert(artifact.id().to_owned()) {
            return Err(format!(
                "content dependency cycle includes {}",
                artifact.id()
            ));
        }
        for dependency in artifact
            .dependencies()
            .iter()
            .filter(|item| !item.optional())
        {
            let requirement = dependency
                .version()
                .map(semver::VersionReq::parse)
                .transpose()
                .map_err(|error| {
                    format!(
                        "{} has invalid version requirement for {}: {error}",
                        artifact.id(),
                        dependency.id()
                    )
                })?;
            if let Some(child) = catalog.satisfying_version(dependency.id(), requirement.as_ref()) {
                let versioned_selector = child
                    .version()
                    .map(|version| format!("{}@{}", child.id(), version))
                    .unwrap_or_else(|| child.id().to_owned());
                if let Some(existing) = state
                    .reports
                    .iter()
                    .find(|report| report.artifact_id() == child.id())
                {
                    if existing.version() == child.version() {
                        validate_dependency_edge(
                            artifact.kind(),
                            dependency.relationship(),
                            child,
                        )?;
                        continue;
                    }
                    return Err(format!(
                        "{} resolves two versions of '{}', but app installation currently stores one version per content ID",
                        artifact.id(),
                        child.id()
                    ));
                }
                install_selector(
                    context,
                    InstallSelectorRequest {
                        selector: &versioned_selector,
                        expected_id: Some(child.id()),
                        parent_kind: Some(artifact.kind()),
                        relationship: Some(dependency.relationship()),
                        require_workspace: true,
                    },
                    state,
                )?;
            } else {
                return Err(format!(
                    "local content '{}' requires every descendant to be authored in the selected workspace, but '{}' has no compatible local source",
                    artifact.id(),
                    dependency.id()
                ));
            }
        }
        for conflict in artifact.conflicts() {
            if state
                .index
                .installed
                .get(conflict.id())
                .is_some_and(|record| record.enabled)
            {
                return Err(format!(
                    "{} conflicts with installed content '{}'",
                    artifact.id(),
                    conflict.id()
                ));
            }
        }
        let package_selector = artifact
            .version()
            .map(|version| format!("{}@{}", artifact.id(), version))
            .unwrap_or_else(|| artifact.id().to_owned());
        let package = package_with_runtime_targets(
            context
                .paths
                .expect("source catalog only exists with source paths"),
            &package_selector,
            false,
            context.runtime_targets.to_vec(),
        )?;
        let report = install_package(
            context.installation,
            context.layout,
            state.index,
            artifact,
            package.root(),
            package.fingerprint().clone(),
            "source-package",
        )?;
        state.reports.push(report);
        state.visiting.remove(artifact.id());
        return Ok(());
    }

    if require_workspace {
        return Err(format!(
            "local content requires every descendant to be authored in the selected workspace, but '{selector}' has no compatible local source"
        ));
    }

    if state.index.cache_by_selector(selector).is_none()
        && let Some(seed) = root_content_seed(context.installation, selector)?
    {
        let acquired = acquire_workshop_item(
            context.installation,
            seed.app_id,
            &seed.workshop_id,
            context.account,
        )?;
        state.index.caches.insert(
            acquired.artifact_id().to_owned(),
            CacheRecord {
                artifact_id: acquired.artifact_id().to_owned(),
                workshop_id: Some(seed.workshop_id),
                version: read_deployed_manifest(acquired.cache_root())?
                    .version()
                    .map(ToOwned::to_owned),
                cache_root: acquired.cache_root().to_path_buf(),
                fingerprint: acquired.fingerprint().clone(),
                acquired_at: now_seconds(),
            },
        );
    }

    let cache = state.index
        .cache_by_selector(selector)
        .map(|(_, cache)| cache.clone())
        .ok_or_else(|| {
            format!(
                "cannot install '{selector}': no matching source artifact, app-owned cache entry, or root content seed exists\nhelp: run `content acquire ARTIFACT`, open a source, or add a [[root.content]] seed"
            )
        })?;
    let pseudo = read_deployed_manifest(&cache.cache_root)?;
    if expected_id.is_some_and(|expected| pseudo.id() != expected) {
        return Err(format!(
            "selector '{selector}' resolved to '{}', expected dependency '{}'",
            pseudo.id(),
            expected_id.unwrap_or_default()
        ));
    }
    if let (Some(parent_kind), Some(relationship)) = (parent_kind, relationship) {
        validate_dependency_edge(parent_kind, relationship, &pseudo)?;
    }
    validate_artifact_dependency_roles(&pseudo)?;
    let pseudo_id = pseudo.id().to_owned();
    if !state.visiting.insert(pseudo_id.clone()) {
        return Err(format!("content dependency cycle includes {}", pseudo.id()));
    }
    let result = (|| {
        for dependency in pseudo.dependencies().iter().filter(|item| !item.optional()) {
            let requirement = dependency
                .version()
                .map(semver::VersionReq::parse)
                .transpose()
                .map_err(|error| {
                    format!(
                        "{} has invalid version requirement for {}: {error}",
                        pseudo.id(),
                        dependency.id()
                    )
                })?;
            if state
                .index
                .installed
                .get(dependency.id())
                .is_some_and(|installed| {
                    installed.enabled
                        && version_satisfies(installed.version.as_deref(), requirement.as_ref())
                })
            {
                let installed = state
                    .index
                    .installed
                    .get(dependency.id())
                    .expect("installed dependency was checked above");
                let installed_artifact = read_deployed_manifest(&installed.installed_root)?;
                validate_dependency_edge(
                    pseudo.kind(),
                    dependency.relationship(),
                    &installed_artifact,
                )?;
                continue;
            }
            if let Some(catalog) = context.catalog
                && let Some(child) =
                    catalog.satisfying_version(dependency.id(), requirement.as_ref())
            {
                let selector = child
                    .version()
                    .map(|version| format!("{}@{}", child.id(), version))
                    .unwrap_or_else(|| child.id().to_owned());
                if let Some(existing) = state
                    .reports
                    .iter()
                    .find(|report| report.artifact_id() == child.id())
                {
                    if existing.version() == child.version() {
                        validate_dependency_edge(pseudo.kind(), dependency.relationship(), child)?;
                        continue;
                    }
                    return Err(format!(
                        "{} resolves two versions of '{}', but app installation currently stores one version per content ID",
                        pseudo.id(),
                        child.id()
                    ));
                }
                install_selector(
                    context,
                    InstallSelectorRequest {
                        selector: &selector,
                        expected_id: Some(child.id()),
                        parent_kind: Some(pseudo.kind()),
                        relationship: Some(dependency.relationship()),
                        require_workspace: false,
                    },
                    state,
                )?;
                continue;
            }
            let cache_selector = dependency.workshop_id().unwrap_or_else(|| dependency.id());
            if let Some((cache_id, cached)) = state.index.cache_by_selector(cache_selector) {
                let cached_artifact = read_deployed_manifest(&cached.cache_root)?;
                if version_satisfies(cached_artifact.version(), requirement.as_ref()) {
                    if cached_artifact.id() != dependency.id() {
                        return Err(format!(
                            "Workshop/cache selector for '{}' resolved to '{}'; dependency identity must match exactly",
                            dependency.id(),
                            cached_artifact.id()
                        ));
                    }
                    let selector = dependency.workshop_id().unwrap_or(&cache_id);
                    install_selector(
                        context,
                        InstallSelectorRequest {
                            selector,
                            expected_id: Some(dependency.id()),
                            parent_kind: Some(pseudo.kind()),
                            relationship: Some(dependency.relationship()),
                            require_workspace: false,
                        },
                        state,
                    )?;
                    continue;
                }
            }
            return Err(format!(
                "{} requires {} '{}' at {}, but no compatible installed, local, or cached version is available",
                pseudo.id(),
                dependency.relationship(),
                dependency.id(),
                dependency.version().unwrap_or("any version")
            ));
        }
        for conflict in pseudo.conflicts() {
            if state
                .index
                .installed
                .get(conflict.id())
                .is_some_and(|record| record.enabled)
            {
                return Err(format!(
                    "{} conflicts with installed content '{}'",
                    pseudo.id(),
                    conflict.id()
                ));
            }
        }
        let report = install_package(
            context.installation,
            context.layout,
            state.index,
            &pseudo,
            &cache.cache_root,
            cache.fingerprint,
            "cache",
        )?;
        state.reports.push(report);
        Ok(())
    })();
    state.visiting.remove(&pseudo_id);
    result
}

fn install_package(
    installation: &InstallationPaths,
    layout: &ContentLayout,
    index: &mut ContentIndex,
    artifact: &ContentArtifact,
    artifact_root: &Path,
    expected_fingerprint: Fingerprint,
    source: &str,
) -> Result<InstallReport, String> {
    let target = layout.installed().join(artifact.id());
    reset_directory(installation.root(), &target)?;
    fs::create_dir_all(&target).map_err(io("create installed content", &target))?;
    copy_tree(artifact_root, &target, artifact_root, &[])?;
    let fingerprint = fingerprint_tree(&target)?;
    if fingerprint != expected_fingerprint {
        return Err(format!(
            "installed fingerprint mismatch for {}\n  expected: {}\n  observed: {}",
            artifact.id(),
            expected_fingerprint.hash(),
            fingerprint.hash()
        ));
    }
    let record = InstalledRecord {
        artifact_id: artifact.id().to_owned(),
        kind: artifact.kind().to_string(),
        version: artifact.version().map(ToOwned::to_owned),
        workshop_id: artifact
            .workshop()
            .published_file_id()
            .map(ToOwned::to_owned),
        enabled: true,
        installed_root: target.clone(),
        fingerprint: fingerprint.clone(),
        dependencies: artifact.dependencies().to_vec(),
        conflicts: artifact.conflicts().to_vec(),
        installed_at: now_seconds(),
        source: source.to_owned(),
    };
    index
        .installed
        .insert(artifact.id().to_owned(), record.clone());
    write_lock(layout, &record)?;
    let receipt = write_receipt(
        installation,
        "install",
        artifact.id(),
        "installed",
        Some(&format!("root={}", target.display())),
    )?;
    Ok(InstallReport {
        artifact_id: artifact.id().to_owned(),
        version: artifact.version().map(ToOwned::to_owned),
        installed_root: target,
        fingerprint,
        receipt,
    })
}

fn move_enabled_state(
    installation: &InstallationPaths,
    selector: &str,
    enable: bool,
) -> Result<InstallReport, String> {
    let layout = ContentLayout::new(installation);
    let mut index = load_index(&layout)?;
    let artifact_id = resolve_installed_selector(&index, selector)?;
    let record = index
        .installed
        .get_mut(&artifact_id)
        .ok_or_else(|| format!("content is not installed: {artifact_id}"))?;
    let (from_root, to_root) = if enable {
        (
            layout.disabled().join(&artifact_id),
            layout.installed().join(&artifact_id),
        )
    } else {
        (
            layout.installed().join(&artifact_id),
            layout.disabled().join(&artifact_id),
        )
    };
    if !from_root.exists() {
        return Err(format!(
            "cannot {} {}: expected artifact root is missing at {}",
            if enable { "enable" } else { "disable" },
            artifact_id,
            from_root.display()
        ));
    }
    if let Some(parent) = to_root.parent() {
        fs::create_dir_all(parent).map_err(io("create content state parent", parent))?;
    }
    if to_root.exists() {
        remove_directory(installation.root(), &to_root)?;
    }
    fs::rename(&from_root, &to_root).map_err(io("move content artifact root", &from_root))?;
    let fingerprint = fingerprint_tree(&to_root)?;
    record.enabled = enable;
    record.installed_root = to_root.clone();
    record.fingerprint = fingerprint.clone();
    let record_clone = record.clone();
    let version = record.version.clone();
    save_index(&layout, &index)?;
    write_lock(&layout, &record_clone)?;
    let receipt = write_receipt(
        installation,
        if enable { "enable" } else { "disable" },
        &artifact_id,
        if enable { "enabled" } else { "disabled" },
        Some(&format!("root={}", to_root.display())),
    )?;
    Ok(InstallReport {
        artifact_id,
        version,
        installed_root: to_root,
        fingerprint,
        receipt,
    })
}

fn verify_one(index: &ContentIndex, artifact_id: &str) -> VerifyReport {
    let Some(record) = index.installed.get(artifact_id) else {
        return VerifyReport {
            artifact_id: artifact_id.to_owned(),
            ok: false,
            expected: None,
            observed: None,
            detail: "not installed".to_owned(),
        };
    };
    if !record.installed_root.exists() {
        return VerifyReport {
            artifact_id: artifact_id.to_owned(),
            ok: false,
            expected: Some(record.fingerprint.clone()),
            observed: None,
            detail: format!(
                "installed artifact root is missing: {}",
                record.installed_root.display()
            ),
        };
    }
    match fingerprint_tree(&record.installed_root) {
        Ok(observed) if observed == record.fingerprint => VerifyReport {
            artifact_id: artifact_id.to_owned(),
            ok: true,
            expected: Some(record.fingerprint.clone()),
            observed: Some(observed),
            detail: "fingerprint matches".to_owned(),
        },
        Ok(observed) => VerifyReport {
            artifact_id: artifact_id.to_owned(),
            ok: false,
            expected: Some(record.fingerprint.clone()),
            observed: Some(observed),
            detail: "fingerprint mismatch".to_owned(),
        },
        Err(error) => VerifyReport {
            artifact_id: artifact_id.to_owned(),
            ok: false,
            expected: Some(record.fingerprint.clone()),
            observed: None,
            detail: error,
        },
    }
}

fn select_artifacts<'a>(
    catalog: &'a ContentCatalog,
    selector: Option<&str>,
) -> Result<Vec<&'a ContentArtifact>, String> {
    if let Some(selector) = selector {
        catalog.find(selector).map(|artifact| vec![artifact]).ok_or_else(|| {
            format!(
                "unknown content artifact '{selector}'\nhelp: inspect source content with `content list`"
            )
        })
    } else {
        Ok(catalog.artifacts().iter().collect())
    }
}

fn root_steam_app_id(installation: &InstallationPaths) -> Result<u32, String> {
    #[derive(Deserialize)]
    struct RootManifest {
        root: Option<RootSection>,
    }
    #[derive(Deserialize)]
    struct RootSection {
        steam: Option<RootSteam>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct RootSteam {
        app_id: Option<u32>,
    }

    let path = installation.root().join(manifest::APP_FILE_NAME);
    let source = fs::read_to_string(&path).map_err(io("read root manifest", &path))?;
    let manifest: RootManifest = toml::from_str(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", path.display()))?;
    manifest
        .root
        .and_then(|root| root.steam)
        .and_then(|steam| steam.app_id)
        .filter(|app_id| *app_id != 0)
        .ok_or_else(|| {
            format!(
                "cannot infer Steam AppID for Workshop download; '{}' needs [root.steam].app-id",
                path.display()
            )
        })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct RootContentSeed {
    id: String,
    kind: String,
    app_id: u32,
    workshop_id: String,
    default_launch: Option<String>,
}

fn root_content_seed(
    installation: &InstallationPaths,
    selector: &str,
) -> Result<Option<RootContentSeed>, String> {
    #[derive(Deserialize)]
    struct RootManifest {
        root: Option<RootSection>,
    }
    #[derive(Deserialize)]
    struct RootSection {
        #[serde(default)]
        content: Vec<RootContentSeed>,
    }

    let path = installation.root().join(manifest::APP_FILE_NAME);
    let source = fs::read_to_string(&path).map_err(io("read root manifest", &path))?;
    let manifest: RootManifest = toml::from_str(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", path.display()))?;
    Ok(manifest
        .root
        .into_iter()
        .flat_map(|root| root.content)
        .find(|seed| root_content_seed_matches(seed, selector)))
}

fn root_content_seed_matches(seed: &RootContentSeed, selector: &str) -> bool {
    // Install selectors may include an exact version (`artifact@1.2.3`). The
    // seed identifies the Workshop artifact; cache resolution below remains
    // responsible for requiring that the downloaded manifest has that version.
    let identity = selector
        .rsplit_once('@')
        .map_or(selector, |(identity, _)| identity);

    seed.id == identity
        || seed
            .id
            .rsplit('/')
            .next()
            .is_some_and(|name| name == identity)
        || seed.workshop_id == selector
        || seed
            .default_launch
            .as_deref()
            .is_some_and(|launch| launch == selector)
}

fn fingerprint_tree(root: &Path) -> Result<Fingerprint, String> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    let mut hash = 0xcbf29ce484222325u64;
    let mut file_count = 0u64;
    let mut byte_count = 0u64;
    for relative in files {
        let path = root.join(&relative);
        update_hash(&mut hash, relative.to_string_lossy().as_bytes());
        update_hash(&mut hash, &[0]);
        let mut file = fs::File::open(&path).map_err(io("open fingerprint input", &path))?;
        let mut buffer = [0u8; 8192];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(io("read fingerprint input", &path))?;
            if read == 0 {
                break;
            }
            update_hash(&mut hash, &buffer[..read]);
            byte_count += read as u64;
        }
        file_count += 1;
    }
    Ok(Fingerprint {
        algorithm: FINGERPRINT_ALGORITHM.to_owned(),
        hash: format!("{hash:016x}"),
        files: file_count,
        bytes: byte_count,
    })
}

fn collect_files(root: &Path, directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(io("read fingerprint directory", directory))? {
        let entry = entry.map_err(|error| format!("failed to read fingerprint entry: {error}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect '{}': {error}", path.display()))?;
        if file_type.is_dir() {
            collect_files(root, &path, output)?;
        } else if file_type.is_file() {
            output.push(
                path.strip_prefix(root)
                    .map_err(|error| format!("failed to relativize '{}': {error}", path.display()))?
                    .to_path_buf(),
            );
        }
    }
    Ok(())
}

fn update_hash(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

fn copy_tree(source: &Path, target: &Path, root: &Path, exclusions: &[&str]) -> Result<(), String> {
    let relative = source.strip_prefix(root).unwrap_or(Path::new(""));
    if exclusions
        .iter()
        .any(|excluded| relative.starts_with(Path::new(excluded)))
    {
        return Ok(());
    }
    let metadata = fs::metadata(source).map_err(io("inspect copy source", source))?;
    if metadata.is_dir() {
        fs::create_dir_all(target).map_err(io("create copy target", target))?;
        for entry in fs::read_dir(source).map_err(io("read copy source", source))? {
            let entry = entry.map_err(|error| format!("failed to read copy entry: {error}"))?;
            copy_tree(
                &entry.path(),
                &target.join(entry.file_name()),
                root,
                exclusions,
            )?;
        }
    } else if metadata.is_file() {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io("create copy target parent", parent))?;
        }
        fs::copy(source, target).map_err(io("copy content file", source))?;
    }
    Ok(())
}

fn default_exclusions() -> &'static [&'static str] {
    &[".git", "target", "output"]
}

fn reset_directory(app_root: &Path, target: &Path) -> Result<(), String> {
    ensure_contained(app_root, target)?;
    if target.exists() {
        remove_directory(app_root, target)?;
    }
    fs::create_dir_all(target).map_err(io("create directory", target))
}

fn remove_directory(app_root: &Path, target: &Path) -> Result<(), String> {
    ensure_contained(app_root, target)?;
    fs::remove_dir_all(target).map_err(io("remove directory", target))
}

fn slug(id: &str) -> String {
    id.chars()
        .map(|character| {
            if character.is_ascii_lowercase()
                || character.is_ascii_uppercase()
                || character.is_ascii_digit()
                || character == '-'
            {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn steam_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn io<'a>(action: &'a str, path: &'a Path) -> impl Fn(std::io::Error) -> String + 'a {
    move |error| format!("failed to {action} '{}': {error}", path.display())
}

#[cfg(test)]
mod fork_manifest_tests {
    use super::*;

    #[test]
    fn fork_manifest_restores_source_identity_and_unpublishes_the_original_item() {
        let deployed = r#"schema = 1

[game-mod]
id = "community/nebula/movement"
name = "Movement"
version = "2.3.0"
runtime = [{ target = "x86_64-unknown-linux-gnu", libraries = ["vapor_community_nebula_movement"] }]

[[game-mod.dependencies]]
id = "community/shared/math"
version = ">=1.0.0"

[game-mod.steam]
app-id = 2122620
published-file-id = "3762162548"
title = "Movement"
"#;

        let source =
            source_manifest_for_fork(deployed, ContentKind::GameMod, "nebula-movement").unwrap();
        let document = source.parse::<toml_edit::DocumentMut>().unwrap();
        assert!(document.get("schema").is_none());
        let role = document.get("game-mod").unwrap().as_table().unwrap();
        assert!(role.get("id").is_none());
        assert!(role.get("runtime").is_none());
        assert_eq!(role.get("name").unwrap().as_str(), Some("nebula-movement"));
        assert_eq!(role.get("version").unwrap().as_str(), Some("2.3.0"));
        assert_eq!(
            role.get("dependencies")
                .unwrap()
                .as_array_of_tables()
                .unwrap()
                .iter()
                .next()
                .unwrap()
                .get("id")
                .unwrap()
                .as_str(),
            Some("community/shared/math")
        );
        let steam = role.get("steam").unwrap().as_table().unwrap();
        assert!(steam.get("published-file-id").is_none());
        assert_eq!(steam.get("app-id").unwrap().as_integer(), Some(2122620));
        assert_eq!(steam.get("title").unwrap().as_str(), Some("Movement"));
    }

    #[test]
    fn root_content_seed_matches_version_qualified_artifact_selectors() {
        let seed = RootContentSeed {
            id: "ghf-studios/loo-cast/loo-cast-packagepack".to_owned(),
            kind: "packagepack".to_owned(),
            app_id: 2122620,
            workshop_id: "3762162548".to_owned(),
            default_launch: Some("default".to_owned()),
        };

        assert!(root_content_seed_matches(
            &seed,
            "ghf-studios/loo-cast/loo-cast-packagepack@0.5.0"
        ));
        assert!(root_content_seed_matches(
            &seed,
            "loo-cast-packagepack@0.5.0"
        ));
        assert!(root_content_seed_matches(&seed, "3762162548"));
        assert!(!root_content_seed_matches(
            &seed,
            "ghf-studios/loo-cast/other-packagepack@0.5.0"
        ));
    }

    #[test]
    fn fork_rewrites_owned_content_and_crate_identity_without_touching_dependencies() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("vapor-fork-test-{stamp}"));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"vapor_author_old_mod\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        fs::write(
            root.join("src/lib.rs"),
            "pub const CONTENT_ID: &str = \"author/old/mod\";\npub use vapor_author_old_mod::run;\n",
        )
        .unwrap();

        rewrite_fork_source(
            &root,
            "author/old/mod",
            "local/workspace/new-mod",
            "vapor_author_old_mod",
            "vapor_local_workspace_new_mod",
        )
        .unwrap();

        let source = fs::read_to_string(root.join("src/lib.rs")).unwrap();
        assert!(source.contains("local/workspace/new-mod"));
        assert!(source.contains("vapor_local_workspace_new_mod::run"));
        assert!(!source.contains("author/old/mod"));
        assert!(
            fs::read_to_string(root.join("Cargo.toml"))
                .unwrap()
                .contains("vapor_local_workspace_new_mod")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cached_source_fork_registers_new_vapor_and_cargo_projects_and_rejects_binary_only_items() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("vapor-fork-integration-{stamp}"));
        let installation_root = root.join("installation");
        let workspace_root = root.join("workspace");
        fs::create_dir_all(installation_root.join("bin")).unwrap();
        fs::create_dir_all(workspace_root.join("fixtures/base")).unwrap();
        fs::write(installation_root.join("bin/vapor"), "").unwrap();
        fs::write(
            installation_root.join(manifest::APP_FILE_NAME),
            "schema = 1\n\n[root]\nname = \"test-app\"\norganization = \"test\"\n",
        )
        .unwrap();
        fs::write(
            workspace_root.join(manifest::WORKSPACE_FILE_NAME),
            "schema = 1\n\n[workspace]\nname = \"workspace\"\norganization = \"author\"\nversion = \"0.1.0\"\nrepository = \"\"\n\n[toolchain]\nchannel = \"nightly\"\nversion = \"nightly-2026-01-30\"\ndate = \"2026-01-30\"\n\n[[project]]\nname = \"base\"\npath = \"fixtures/base\"\n",
        )
        .unwrap();
        fs::write(
            workspace_root.join("Cargo.toml"),
            "[workspace]\nresolver = \"3\"\nmembers = [\"fixtures/base\"]\n",
        )
        .unwrap();
        fs::write(
            workspace_root.join("fixtures/base/Cargo.toml"),
            "[package]\nname = \"base\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();

        let installation = InstallationPaths::from_executable(&installation_root.join("bin/vapor"))
            .expect("test installation is valid");
        let paths = EnvironmentPaths::from_installation_and_source_path(
            installation.clone(),
            &workspace_root,
        )
        .expect("test workspace is valid");
        let layout = ContentLayout::new(&installation);
        let source_id = "publisher/project/movement";
        let cache_root = layout.cache().join("packages/3762162548");
        fs::create_dir_all(cache_root.join("src")).unwrap();
        fs::write(
            cache_root.join(manifest::GAME_MOD_FILE_NAME),
            "schema = 1\n\n[game-mod]\nid = \"publisher/project/movement\"\nname = \"Movement\"\nversion = \"2.3.0\"\n\n[game-mod.steam]\napp-id = 2122620\npublished-file-id = \"3762162548\"\ntitle = \"Movement\"\n",
        )
        .unwrap();
        fs::write(
            cache_root.join("Cargo.toml"),
            "[package]\nname = \"vapor_publisher_project_movement\"\nversion = \"2.3.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::write(
            cache_root.join("src/lib.rs"),
            "pub const CONTENT_ID: &str = \"publisher/project/movement\";\n",
        )
        .unwrap();

        let binary_only_id = "publisher/project/binary-addon";
        let binary_cache = layout.cache().join("packages/3762162549");
        fs::create_dir_all(&binary_cache).unwrap();
        fs::write(
            binary_cache.join(manifest::GAME_MOD_FILE_NAME),
            "schema = 1\n\n[game-mod]\nid = \"publisher/project/binary-addon\"\nname = \"Binary addon\"\nversion = \"1.0.0\"\nbinaries = [\"addon\"]\n",
        )
        .unwrap();
        let mut index = ContentIndex::default();
        for (id, published_file_id, cache_root) in [
            (source_id, "3762162548", cache_root),
            (binary_only_id, "3762162549", binary_cache),
        ] {
            index.caches.insert(
                id.to_owned(),
                CacheRecord {
                    artifact_id: id.to_owned(),
                    workshop_id: Some(published_file_id.to_owned()),
                    version: Some(if id == source_id { "2.3.0" } else { "1.0.0" }.to_owned()),
                    fingerprint: fingerprint_tree(&cache_root).unwrap(),
                    cache_root,
                    acquired_at: 0,
                },
            );
        }
        save_index(&layout, &index).unwrap();

        let manifest_path =
            fork_cached_artifact(&paths, "publisher/project/movement@2.3.0", "movement-copy")
                .expect("source-backed package can be forked");
        let fork_root = manifest_path.parent().unwrap();
        assert!(manifest_path.is_file());
        let source = fs::read_to_string(&manifest_path).unwrap();
        assert!(!source.contains("published-file-id"));
        assert!(!source.contains("id = \"publisher/project/movement\""));
        assert!(source.contains("name = \"movement-copy\""));
        assert!(
            fs::read_to_string(fork_root.join("Cargo.toml"))
                .unwrap()
                .contains("vapor_author_workspace_movement_copy")
        );
        assert!(
            fs::read_to_string(fork_root.join("src/lib.rs"))
                .unwrap()
                .contains("author/workspace/movement-copy")
        );
        assert!(
            fs::read_to_string(workspace_root.join("Cargo.toml"))
                .unwrap()
                .contains("content/movement-copy")
        );
        assert!(
            discover(&paths)
                .unwrap()
                .artifacts()
                .iter()
                .any(|artifact| artifact.id() == "author/workspace/movement-copy")
        );

        let error = fork_cached_artifact(&paths, binary_only_id, "binary-copy").unwrap_err();
        assert!(error.contains("no complete Rust source project"));
        assert!(!workspace_root.join("content/binary-copy").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_collection_removes_only_the_library_copy() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("vapor-cache-collection-{stamp}"));
        let installation_root = root.join("installation");
        fs::create_dir_all(installation_root.join("bin")).unwrap();
        fs::write(installation_root.join("bin/vapor"), "").unwrap();
        fs::write(
            installation_root.join(manifest::APP_FILE_NAME),
            "schema = 1\n\n[root]\nname = \"test-app\"\norganization = \"test\"\n",
        )
        .unwrap();
        let installation = InstallationPaths::from_executable(&installation_root.join("bin/vapor"))
            .expect("test installation is valid");
        let layout = ContentLayout::new(&installation);
        let content_id = "publisher/project/cache-me";
        let cache_root = layout.cache().join("packages/3762162550");
        fs::create_dir_all(&cache_root).unwrap();
        fs::write(cache_root.join("payload.txt"), "cached").unwrap();
        let installed_root = layout.installed().join("packages/3762162550");
        fs::create_dir_all(&installed_root).unwrap();
        fs::write(installed_root.join("payload.txt"), "installed").unwrap();
        let mut index = ContentIndex::default();
        index.caches.insert(
            content_id.to_owned(),
            CacheRecord {
                artifact_id: content_id.to_owned(),
                workshop_id: Some("3762162550".to_owned()),
                version: Some("1.0.0".to_owned()),
                fingerprint: fingerprint_tree(&cache_root).unwrap(),
                cache_root: cache_root.clone(),
                acquired_at: 0,
            },
        );
        save_index(&layout, &index).unwrap();

        assert!(collect_cached_artifact(&installation, content_id).unwrap());
        assert!(!cache_root.exists());
        assert_eq!(
            fs::read_to_string(installed_root.join("payload.txt")).unwrap(),
            "installed"
        );
        assert!(!load_index(&layout).unwrap().caches.contains_key(content_id));
        assert!(!collect_cached_artifact(&installation, content_id).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incompatible_auto_install_download_preserves_the_previous_library_copy() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("vapor-cache-update-{stamp}"));
        let installation_root = root.join("installation");
        let provider_root = root.join("provider");
        fs::create_dir_all(installation_root.join("bin")).unwrap();
        fs::create_dir_all(&provider_root).unwrap();
        fs::write(installation_root.join("bin/vapor"), "").unwrap();
        fs::write(
            installation_root.join(manifest::APP_FILE_NAME),
            "schema = 1\n\n[root]\nname = \"test-app\"\norganization = \"test\"\n",
        )
        .unwrap();
        let installation = InstallationPaths::from_executable(&installation_root.join("bin/vapor"))
            .expect("test installation is valid");
        let layout = ContentLayout::new(&installation);
        let content_id = "publisher/project/versioned";
        let published_file_id = "3762162551";
        let cache_root = layout.cache().join("packages").join(published_file_id);
        fs::create_dir_all(&cache_root).unwrap();
        fs::write(cache_root.join("payload.txt"), "old-version").unwrap();
        let mut index = ContentIndex::default();
        index.caches.insert(
            content_id.to_owned(),
            CacheRecord {
                artifact_id: content_id.to_owned(),
                workshop_id: Some(published_file_id.to_owned()),
                version: Some("1.5.0".to_owned()),
                fingerprint: fingerprint_tree(&cache_root).unwrap(),
                cache_root: cache_root.clone(),
                acquired_at: 1,
            },
        );
        save_index(&layout, &index).unwrap();
        fs::write(
            provider_root.join(manifest::GAME_MOD_FILE_NAME),
            format!(
                "schema = 1\n\n[game-mod]\nid = \"{content_id}\"\nname = \"Versioned\"\nversion = \"2.1.0\"\n"
            ),
        )
        .unwrap();
        fs::write(provider_root.join("payload.txt"), "new-version").unwrap();

        let constraints = BTreeMap::from([(content_id.to_owned(), vec!["^1.0".to_owned()])]);
        let error = import_workshop_item_with_constraints(
            &installation,
            2122620,
            published_file_id,
            &provider_root,
            Some(&constraints),
        )
        .unwrap_err();
        assert!(error.contains("does not satisfy every dependent composition"));
        assert_eq!(
            fs::read_to_string(cache_root.join("payload.txt")).unwrap(),
            "old-version"
        );
        assert_eq!(
            load_index(&layout).unwrap().caches[content_id]
                .version
                .as_deref(),
            Some("1.5.0")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
