use std::path::{Path, PathBuf};
use vapor_client_domain::LibraryAction;
use vapor_client_provider::{
    content,
    discovery::{EnvironmentPaths, InstallationPaths},
};

/// Stage or preview one authored artifact package through Vapor's content provider.
pub fn package_authored_content(
    application_executable: &Path,
    workspace_root: &Path,
    selector: &str,
    target_triples: &[String],
    dry_run: bool,
) -> Result<content::PackageReport, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    content::package_for_targets(&paths, selector, dry_run, target_triples)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationCheck {
    pub ok: bool,
    pub detail: Vec<String>,
    pub artifacts: Vec<InstalledArtifact>,
    pub graph: Option<crate::ResolvedCompositionGraph>,
    /// Present when the verified content graph cannot launch on this host and
    /// should be refreshed from its current source before play.
    pub update_required: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledArtifact {
    pub id: String,
    pub version: Option<String>,
    pub root: PathBuf,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopAcquisition {
    pub id: String,
    pub version: Option<String>,
    pub kind: String,
    pub cache_root: PathBuf,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopAcquisitionBatch {
    pub items: Vec<WorkshopAcquisition>,
    pub dependencies_acquired: usize,
}

#[derive(Debug, PartialEq, Eq)]
struct WorkshopDependencyRequest {
    content_id: String,
    workshop_id: String,
    version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledComposition {
    pub graph: crate::ResolvedCompositionGraph,
    pub artifacts: Vec<InstalledArtifact>,
}

/// Installed graph plus the outcome of attempting to launch it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionRunResult {
    pub composition: InstalledComposition,
    pub composition_graph_matches: bool,
    pub launch_error: Option<String>,
}

/// Execute one explicit Library lifecycle operation through Vapor's shared
/// provider and return a concise user-facing summary.
pub fn run_library_operation(
    application_executable: &Path,
    workspace_root: Option<&Path>,
    action: &LibraryAction,
) -> Result<String, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let needs_source = matches!(
        action,
        LibraryAction::Install { .. }
            | LibraryAction::Acquire { .. }
            | LibraryAction::ValidateArtifacts { .. }
            | LibraryAction::Update { .. }
            | LibraryAction::Repair { .. }
    );
    let paths = if needs_source {
        workspace_root
            .map(|root| {
                EnvironmentPaths::from_installation_and_source_path(installation.clone(), root)
            })
            .transpose()?
    } else {
        None
    };
    match action {
        LibraryAction::Install {
            selector,
            account,
            target_triples,
            ..
        } => {
            let reports = content::install_with_account_for_targets(
                &installation,
                paths.as_ref(),
                selector,
                account.as_deref(),
                target_triples,
            )?;
            Ok(format_install_summary("Installed", &reports))
        }
        LibraryAction::Acquire { selectors, .. } => {
            let reports = content::download_many(&installation, paths.as_ref(), selectors, None)?;
            let identities = reports
                .iter()
                .map(|report| report.artifact_id())
                .collect::<Vec<_>>();
            Ok(format!("Added {} to the Library.", identities.join(", ")))
        }
        LibraryAction::ValidateArtifacts { selector, .. } => {
            let paths = paths
                .as_ref()
                .ok_or("Open a source workspace before validating content.")?;
            let report = content::validate(paths, selector.as_deref())?;
            let checked = report.checked().join(", ");
            let diagnostics = if report.diagnostics().is_empty() {
                String::new()
            } else {
                format!("\n{}", report.diagnostics().join("\n"))
            };
            Ok(format!(
                "Validated {} content item(s): {checked}.{diagnostics}",
                report.checked().len()
            ))
        }
        LibraryAction::Update { selector, .. } => {
            let reports = content::update(&installation, paths.as_ref(), selector.as_deref())?;
            Ok(format_install_summary("Updated", &reports))
        }
        LibraryAction::Verify { selector } => {
            let reports = content::verify(&installation, selector.as_deref())?;
            if reports.is_empty() {
                return Ok("No installed content to verify.".to_owned());
            }
            Ok(reports
                .iter()
                .map(|report| {
                    format!(
                        "{}: {} ({})",
                        report.artifact_id(),
                        if report.ok() { "verified" } else { "corrupt" },
                        report.detail()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        LibraryAction::Repair { selector, .. } => {
            let reports = content::repair(&installation, paths.as_ref(), selector.as_deref())?;
            if reports.is_empty() {
                Ok("All checked content is already verified.".to_owned())
            } else {
                Ok(format_install_summary("Repaired", &reports))
            }
        }
        LibraryAction::Disable { selector } => {
            let report = content::disable(&installation, selector)?;
            Ok(format!("Disabled {}.", report.artifact_id()))
        }
        LibraryAction::Enable { selector } => {
            let report = content::enable(&installation, selector)?;
            Ok(format!("Enabled {}.", report.artifact_id()))
        }
        LibraryAction::Uninstall { selector } => {
            let report = content::uninstall(&installation, selector)?;
            Ok(format!(
                "Uninstalled {}{}.",
                report.artifact_id(),
                if report.removed() {
                    ""
                } else {
                    " (no local files were present)"
                }
            ))
        }
        LibraryAction::SetAutoInstall { .. } => {
            Err("Auto-Install policy changes are handled by the client state service.".to_owned())
        }
    }
}

fn format_install_summary(action: &str, reports: &[content::InstallReport]) -> String {
    if reports.is_empty() {
        return format!("{action} no content.");
    }
    let identities = reports
        .iter()
        .map(|report| match report.version() {
            Some(version) => format!("{}@{version}", report.artifact_id()),
            None => report.artifact_id().to_owned(),
        })
        .collect::<Vec<_>>();
    format!("{action} {}.", identities.join(", "))
}

/// Compatibility aliases to the shared application install-plan contract.
pub type CompositionPreview = vapor_client_domain::InstallPlan;
pub type CompositionPreviewDependency = vapor_client_domain::InstallPlanDependency;
pub type CompositionPreviewNode = vapor_client_domain::InstallPlanNode;
pub type CompositionPreviewSource = vapor_client_domain::InstallPlanSource;

/// Resolve a composition composition without packaging, downloading, installing, or writing state.
pub fn preview_composition(
    application_executable: &Path,
    workspace_root: Option<&Path>,
    selector: &str,
) -> Result<CompositionPreview, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = workspace_root
        .map(|root| EnvironmentPaths::from_installation_and_source_path(installation.clone(), root))
        .transpose()?;
    content::preview_install(&installation, paths.as_ref(), selector)
}

/// Resolve a root-declared launch alias to its Packagepack content ID.
pub fn default_packagepack_selector(
    application_executable: &Path,
    launch: &str,
) -> Result<String, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    content::default_packagepack_id(&installation, launch)
}

/// Download a public Workshop item by PublishedFileId into Vapor's app-owned cache.
pub fn download_workshop_item(
    application_executable: &Path,
    published_file_id: &str,
) -> Result<WorkshopAcquisition, String> {
    download_workshop_items(application_executable, &[published_file_id.to_owned()])?
        .into_iter()
        .next()
        .ok_or_else(|| "Workshop download returned no artifact".to_owned())
}

/// Download multiple Workshop items through the signed-in Steam client.
///
/// The items are refreshed from Steam for this explicit acquisition without
/// changing the user's subscription list, then copied into Vapor's app-owned
/// cache. This makes Acquire/Subscribe the user's explicit request for the
/// latest published payload, even when Steam considers an installed copy
/// current.
pub fn download_workshop_items(
    application_executable: &Path,
    published_file_ids: &[String],
) -> Result<Vec<WorkshopAcquisition>, String> {
    download_workshop_items_with_constraints(
        application_executable,
        published_file_ids,
        &Default::default(),
    )
    .map(|batch| batch.items)
}

/// Download Workshop items while preserving Auto-Install composition
/// constraints during cache replacement.
pub fn download_workshop_items_with_constraints(
    application_executable: &Path,
    published_file_ids: &[String],
    constraints: &std::collections::BTreeMap<String, Vec<String>>,
) -> Result<WorkshopAcquisitionBatch, String> {
    if published_file_ids.is_empty() {
        return Err("Select at least one Workshop item to import".to_owned());
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut normalized_ids = Vec::new();
    for id in published_file_ids {
        let id = id.trim();
        let parsed = if !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()) {
            id.parse::<u64>().ok().filter(|id| *id != 0)
        } else {
            None
        }
        .ok_or_else(|| "Workshop imports require nonzero numeric PublishedFileIds".to_owned())?;
        let id = parsed.to_string();
        if seen.insert(id.clone()) {
            normalized_ids.push(id);
        }
    }
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let requested = normalized_ids
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let mut pending = normalized_ids.clone();
    let mut visited = std::collections::BTreeSet::new();
    let mut acquisitions = std::collections::BTreeMap::<String, WorkshopAcquisition>::new();
    let mut all_constraints = constraints.clone();
    let mut dependencies_acquired = 0;

    while !pending.is_empty() {
        let mut grouped = std::collections::BTreeMap::<u32, Vec<String>>::new();
        for id in std::mem::take(&mut pending) {
            if !visited.insert(id.clone()) {
                continue;
            }
            let app_id = content::workshop_app_id(&installation, None, &id, &id)?;
            grouped.entry(app_id).or_default().push(id);
        }

        for (app_id, ids) in grouped {
            let numeric_ids = ids
                .iter()
                .map(|id| {
                    id.parse::<u64>()
                        .map_err(|_| format!("Workshop item ID '{id}' is not a valid number"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let imported = vapor_steam::with_refreshed_workshop_items(
                app_id,
                &numeric_ids,
                |published_file_id, root| {
                    let id = published_file_id.to_string();
                    let report = content::import_workshop_item_from_provider_with_constraints(
                        &installation,
                        app_id,
                        &id,
                        root,
                        &all_constraints,
                    )?;
                    let cached = content::cached_artifact(&installation, report.artifact_id())?;
                    Ok((
                        id,
                        WorkshopAcquisition {
                            id: report.artifact_id().to_owned(),
                            version: cached.version().map(ToOwned::to_owned),
                            kind: cached.kind().to_owned(),
                            cache_root: report.cache_root().to_path_buf(),
                            fingerprint: format!(
                                "{}:{}",
                                report.fingerprint().algorithm(),
                                report.fingerprint().hash()
                            ),
                        },
                        cached.dependencies().to_vec(),
                    ))
                },
            )?;

            for (published_file_id, acquisition, dependencies) in imported {
                let acquired_id = acquisition.id.clone();
                if requested.contains(&published_file_id) {
                    acquisitions.insert(published_file_id, acquisition);
                } else {
                    dependencies_acquired += 1;
                }
                let requests = required_workshop_dependency_requests(
                    &acquired_id,
                    &dependencies,
                    |dependency| {
                        if !content::has_cached_artifact(&installation, dependency.id())? {
                            return Ok(false);
                        }
                        content::cached_artifact_satisfies_version(
                            &installation,
                            dependency.id(),
                            dependency.version(),
                        )
                    },
                )?;
                for request in requests {
                    if let Some(requirement) = request.version {
                        all_constraints
                            .entry(request.content_id)
                            .or_default()
                            .push(requirement);
                    }
                    pending.push(request.workshop_id);
                }
            }
        }
    }

    let items = normalized_ids
        .into_iter()
        .map(|id| {
            acquisitions
                .remove(&id)
                .ok_or_else(|| format!("Steam did not return requested Workshop item {id}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(WorkshopAcquisitionBatch {
        items,
        dependencies_acquired,
    })
}

fn required_workshop_dependency_requests(
    parent_id: &str,
    dependencies: &[vapor_client_provider::content::ContentReference],
    mut cache_satisfies: impl FnMut(
        &vapor_client_provider::content::ContentReference,
    ) -> Result<bool, String>,
) -> Result<Vec<WorkshopDependencyRequest>, String> {
    let mut requests = Vec::new();
    for dependency in dependencies {
        if dependency.optional() || dependency.relationship() == "conflict" {
            continue;
        }
        if cache_satisfies(dependency)? {
            continue;
        }
        let Some(workshop_id) = dependency.workshop_id() else {
            return Err(format!(
                "{parent_id} requires {} at version {}, but it is not in the Library and has no Workshop item ID",
                dependency.id(),
                dependency.version().unwrap_or("any")
            ));
        };
        requests.push(WorkshopDependencyRequest {
            content_id: dependency.id().to_owned(),
            workshop_id: workshop_id.to_owned(),
            version: dependency.version().map(ToOwned::to_owned),
        });
    }
    Ok(requests)
}

pub fn verify_installation(
    application_executable: &Path,
    root_selector: &str,
    selectors: &[String],
) -> Result<InstallationCheck, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let mut ok = !selectors.is_empty();
    let mut present = Vec::new();
    let mut detail = Vec::new();
    for selector in selectors {
        match content::verify(&installation, Some(selector)) {
            Ok(reports) => {
                for report in reports {
                    if !report.ok() {
                        ok = false;
                    }
                    detail.push(format!("{}: {}", report.artifact_id(), report.detail()));
                }
                if content::has_installed_artifact(&installation, selector)? {
                    present.push(selector.clone());
                }
            }
            Err(error) => {
                ok = false;
                let installed = content::has_installed_artifact(&installation, selector)?;
                detail.push(if installed {
                    format!("{selector}: verification failed: {error}")
                } else {
                    format!("{selector}: not installed at the selected version")
                });
            }
        }
    }
    let mut artifacts = Vec::new();
    for artifact in content::installed_artifacts(&installation, &present)? {
        if !artifact.enabled() {
            ok = false;
            detail.push(format!("{}: disabled", artifact.artifact_id()));
        }
        artifacts.push(InstalledArtifact {
            id: artifact.artifact_id().to_owned(),
            version: artifact.version().map(ToOwned::to_owned),
            root: artifact.installed_root().to_path_buf(),
            fingerprint: format!(
                "{}:{}",
                artifact.fingerprint().algorithm(),
                artifact.fingerprint().hash()
            ),
        });
    }
    let graph = match content::installed_packagepack_graph(&installation, root_selector) {
        Ok(graph) => Some(snapshot_installed_graph(&graph)),
        Err(error) => {
            ok = false;
            detail.push(format!("composition graph could not be resolved: {error}"));
            None
        }
    };
    let update_required = if graph.is_some() {
        content::prepare_packagepack_launch(&installation, root_selector)
            .err()
            .map(|error| format!("This setup is not ready to play on this device: {error}"))
    } else {
        None
    };
    Ok(InstallationCheck {
        ok,
        detail,
        artifacts,
        graph,
        update_required,
    })
}

pub fn repair_composition(
    application_executable: &Path,
    workspace_root: Option<&Path>,
    selector: &str,
    graph_selectors: &[String],
) -> Result<InstalledComposition, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = workspace_root
        .map(|root| EnvironmentPaths::from_installation_and_source_path(installation.clone(), root))
        .transpose()?;
    acquire_root_seed_closure(&installation, paths.as_ref(), selector)?;
    let identities = if graph_selectors.is_empty() {
        vec![selector.to_owned()]
    } else {
        graph_selectors.to_vec()
    };
    for identity in &identities {
        if content::has_installed_artifact(&installation, identity)? {
            content::repair(&installation, paths.as_ref(), Some(identity))?;
        } else {
            content::install(&installation, paths.as_ref(), identity)?;
        }
    }
    installed_composition(&installation, selector)
}

/// Install one composition root and its dependencies through Vapor Client's
/// app-owned cache, validation, receipts, and installation state.
pub fn install_composition(
    application_executable: &Path,
    workspace_root: Option<&Path>,
    selector: &str,
) -> Result<InstalledComposition, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = workspace_root
        .map(|root| EnvironmentPaths::from_installation_and_source_path(installation.clone(), root))
        .transpose()?;
    acquire_root_seed_closure(&installation, paths.as_ref(), selector)?;
    content::install(&installation, paths.as_ref(), selector)?;
    installed_composition(&installation, selector)
}

fn acquire_root_seed_closure(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
) -> Result<(), String> {
    let mut acquired = std::collections::BTreeSet::new();
    loop {
        let plan = content::preview_install(installation, paths, selector)?;
        let mut pending = std::collections::BTreeMap::<u32, Vec<String>>::new();
        for node in &plan.nodes {
            if node.source != content::InstallPlanSource::RootSeed {
                continue;
            }
            let Some(published_file_id) = node.published_file_id.as_deref() else {
                continue;
            };
            let app_id =
                content::workshop_app_id(installation, paths, &node.id, published_file_id)?;
            let key = (app_id, published_file_id.to_owned());
            if acquired.insert(key) {
                pending
                    .entry(app_id)
                    .or_default()
                    .push(published_file_id.to_owned());
            }
        }
        if pending.is_empty() {
            return Ok(());
        }

        for (app_id, published_file_ids) in pending {
            let numeric_ids = published_file_ids
                .iter()
                .map(|id| {
                    id.parse::<u64>()
                        .map_err(|_| format!("Workshop item ID '{id}' is not a valid number"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            vapor_steam::with_downloaded_workshop_items(
                app_id,
                &numeric_ids,
                |published_file_id, root| {
                    content::import_workshop_item_from_provider(
                        installation,
                        app_id,
                        &published_file_id.to_string(),
                        root,
                    )
                    .map(|_| ())
                },
            )?;
        }
    }
}

/// Update one composition root from its current workspace source or Vapor cache.
pub fn update_composition(
    application_executable: &Path,
    workspace_root: Option<&Path>,
    selector: &str,
) -> Result<InstalledComposition, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = workspace_root
        .map(|root| EnvironmentPaths::from_installation_and_source_path(installation.clone(), root))
        .transpose()?;
    let root_selector = stable_composition_root_selector(selector);
    let graph = content::installed_packagepack_graph(&installation, root_selector)?;
    let identities = graph
        .nodes()
        .iter()
        .map(|node| node.artifact_id().to_owned())
        .collect::<Vec<_>>();
    if paths.is_none() {
        // A player update must refresh seeded Workshop artifacts before
        // reinstalling from the app-owned cache. Otherwise a valid but stale
        // cached Packagepack can remain permanently missing this host's build.
        let mut seeded_items = std::collections::BTreeMap::<u32, Vec<u64>>::new();
        for (app_id, published_file_id) in
            content::root_content_seed_workshop_items(&installation, &identities)?
        {
            seeded_items
                .entry(app_id)
                .or_default()
                .push(published_file_id);
        }
        for (app_id, published_file_ids) in seeded_items {
            vapor_steam::with_refreshed_workshop_items(
                app_id,
                &published_file_ids,
                |published_file_id, provider_root| {
                    content::import_workshop_item_from_provider(
                        &installation,
                        app_id,
                        &published_file_id.to_string(),
                        provider_root,
                    )
                    .map(|_| ())
                },
            )?;
        }
    }

    refresh_composition_workshop_items(&installation, paths.as_ref(), &graph)?;

    // Explicit Update is the point where a player authorizes new resolution.
    // Reinstall the root by stable ID so its refreshed manifest selects the
    // compatible current dependency graph instead of replaying each old pin.
    // Return by stable ID too: `selector` may still contain the previous exact
    // version, which no longer exists after the update succeeds.
    let root_id = graph.root().artifact_id().to_owned();
    content::install(&installation, paths.as_ref(), &root_id)?;
    installed_composition(&installation, &root_id)
}

fn stable_composition_root_selector(selector: &str) -> &str {
    selector
        .split_once('@')
        .map_or(selector, |(artifact_id, _)| artifact_id)
}

fn refresh_composition_workshop_items(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    graph: &content::InstalledPackagepackGraph,
) -> Result<(), String> {
    let authored_ids = paths
        .map(content::discover)
        .transpose()?
        .map(|catalog| {
            catalog
                .artifacts()
                .iter()
                .map(|artifact| artifact.id().to_owned())
                .collect::<std::collections::BTreeSet<_>>()
        })
        .unwrap_or_default();
    let mut grouped = std::collections::BTreeMap::<u32, Vec<u64>>::new();
    let mut constraints = std::collections::BTreeMap::<String, Vec<String>>::new();

    for node in graph.nodes() {
        for dependency in node.dependencies() {
            if let Some(requirement) = dependency.version() {
                constraints
                    .entry(dependency.id().to_owned())
                    .or_default()
                    .push(requirement.to_owned());
            }
        }
        if authored_ids.contains(node.artifact_id()) {
            continue;
        }
        let Some(published_file_id) = node.workshop_id() else {
            continue;
        };
        let app_id =
            content::workshop_app_id(installation, paths, node.artifact_id(), published_file_id)?;
        let numeric_id = published_file_id
            .parse::<u64>()
            .map_err(|_| format!("Workshop item ID '{published_file_id}' is not a valid number"))?;
        grouped.entry(app_id).or_default().push(numeric_id);
    }

    for (app_id, mut published_file_ids) in grouped {
        published_file_ids.sort_unstable();
        published_file_ids.dedup();
        vapor_steam::with_refreshed_workshop_items(
            app_id,
            &published_file_ids,
            |published_file_id, provider_root| {
                content::import_workshop_item_from_provider_with_constraints(
                    installation,
                    app_id,
                    &published_file_id.to_string(),
                    provider_root,
                    &constraints,
                )
                .map(|_| ())
            },
        )?;
    }

    Ok(())
}

/// Remove one unreferenced cached artifact from the local Library while
/// preserving any installed runtime copy.
pub fn collect_library_item(
    application_executable: &Path,
    content_id: &str,
) -> Result<bool, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    content::collect_cached_artifact(&installation, content_id)
}

fn graph_content_selectors(graph: &crate::ResolvedCompositionGraph) -> Vec<String> {
    graph
        .nodes
        .iter()
        .map(|node| {
            node.content.version.as_deref().map_or_else(
                || node.content.id.clone(),
                |version| format!("{}@{version}", node.content.id),
            )
        })
        .collect()
}

/// Launch one installed composition composition without changing Shell's global selection.
pub fn run_installed_composition(
    application_executable: &Path,
    workspace_root: Option<&Path>,
    selector: &str,
    expected_graph: Option<&crate::ResolvedCompositionGraph>,
    arguments: &[String],
) -> Result<CompositionRunResult, String> {
    run_installed_composition_inner(
        application_executable,
        workspace_root,
        selector,
        expected_graph,
        arguments,
        None,
    )
}

/// Launch one installed composition and forward its process output into the operation log.
pub fn run_installed_composition_with_context(
    application_executable: &Path,
    workspace_root: Option<&Path>,
    selector: &str,
    expected_graph: Option<&crate::ResolvedCompositionGraph>,
    arguments: &[String],
    context: &vapor_core::OperationContext,
) -> Result<CompositionRunResult, String> {
    run_installed_composition_inner(
        application_executable,
        workspace_root,
        selector,
        expected_graph,
        arguments,
        Some(context),
    )
}

fn run_installed_composition_inner(
    application_executable: &Path,
    workspace_root: Option<&Path>,
    selector: &str,
    expected_graph: Option<&crate::ResolvedCompositionGraph>,
    arguments: &[String],
    context: Option<&vapor_core::OperationContext>,
) -> Result<CompositionRunResult, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let installed = content::has_installed_artifact(&installation, selector)?;
    let composition = if installed {
        match installed_composition(&installation, selector) {
            Ok(composition) => composition,
            Err(_error) => {
                if let Some(graph) = expected_graph {
                    let selectors = graph_content_selectors(graph);
                    repair_composition(
                        application_executable,
                        workspace_root,
                        selector,
                        &selectors,
                    )?
                } else {
                    // A root-only install is not launchable. With no saved graph yet,
                    // let Shell install the root's complete dependency closure.
                    install_composition(application_executable, workspace_root, selector)?
                }
            }
        }
    } else if let Some(graph) = expected_graph {
        let selectors = graph_content_selectors(graph);
        repair_composition(application_executable, workspace_root, selector, &selectors)?
    } else {
        install_composition(application_executable, workspace_root, selector)?
    };
    let composition_graph_matches = expected_graph
        .is_none_or(|expected| crate::resolved_graph_matches(expected, &composition.graph));
    let launch_error = if composition_graph_matches {
        content::prepare_packagepack_launch(&installation, selector)
            .and_then(|plan| match context {
                Some(context) => content::run_packagepack_launch_with_output(
                    &plan,
                    "studio-composition",
                    arguments,
                    |stream, text| {
                        context.report_output(vapor_core::OperationOutput {
                            stream,
                            level: vapor_core::ConsoleLevel::Info,
                            process: Some("launched-composition".to_owned()),
                            diagnostic: None,
                            raw_payload: None,
                            text: text.to_owned(),
                        });
                    },
                ),
                None => content::run_packagepack_launch(&plan, "studio-composition", arguments),
            })
            .err()
    } else {
        Some(
            "The installed content no longer matches this composition's saved composition. Preview and update the composition before playing it."
                .to_owned(),
        )
    };
    Ok(CompositionRunResult {
        composition,
        composition_graph_matches,
        launch_error,
    })
}

/// Install source changes for one composition, then launch that exact installed graph.
pub fn run_source_composition(
    application_executable: &Path,
    workspace_root: &Path,
    selector: &str,
    arguments: &[String],
) -> Result<CompositionRunResult, String> {
    let composition = install_composition(application_executable, Some(workspace_root), selector)?;
    run_installed_composition(
        application_executable,
        Some(workspace_root),
        selector,
        Some(&composition.graph),
        arguments,
    )
}

/// Build/install source changes, launch that exact graph, and forward process output.
pub fn run_source_composition_with_context(
    application_executable: &Path,
    workspace_root: &Path,
    selector: &str,
    arguments: &[String],
    context: &vapor_core::OperationContext,
) -> Result<CompositionRunResult, String> {
    let composition = install_composition(application_executable, Some(workspace_root), selector)?;
    run_installed_composition_with_context(
        application_executable,
        Some(workspace_root),
        selector,
        Some(&composition.graph),
        arguments,
        context,
    )
}

fn installed_composition(
    installation: &InstallationPaths,
    selector: &str,
) -> Result<InstalledComposition, String> {
    let graph = content::installed_packagepack_graph(installation, selector)?;
    let graph_snapshot = snapshot_installed_graph(&graph);
    let selectors: Vec<_> = graph
        .nodes()
        .iter()
        .map(|artifact| {
            artifact
                .version()
                .map(|version| format!("{}@{version}", artifact.artifact_id()))
                .unwrap_or_else(|| artifact.artifact_id().to_owned())
        })
        .collect();
    let artifacts = installed_snapshot(installation, &selectors)?;
    Ok(InstalledComposition {
        graph: graph_snapshot,
        artifacts,
    })
}

fn snapshot_installed_graph(
    graph: &content::InstalledPackagepackGraph,
) -> crate::ResolvedCompositionGraph {
    let identity = |artifact: &content::InstalledArtifactSnapshot| crate::ContentReference {
        id: artifact.artifact_id().to_owned(),
        version: artifact.version().map(ToOwned::to_owned),
    };
    let by_id: std::collections::BTreeMap<_, _> = graph
        .nodes()
        .iter()
        .map(|artifact| (artifact.artifact_id(), artifact))
        .collect();
    let nodes = graph
        .nodes()
        .iter()
        .map(|artifact| {
            let dependencies = artifact
                .dependencies()
                .iter()
                .filter(|dependency| dependency.relationship() != "conflict")
                .filter_map(|dependency| {
                    by_id
                        .get(dependency.id())
                        .map(|target| crate::ResolvedDependencyIdentity {
                            binding: dependency.relationship().to_owned(),
                            content: identity(target),
                            version_requirement: dependency.version().map(ToOwned::to_owned),
                        })
                })
                .collect();
            crate::ResolvedNodeIdentity {
                content: identity(artifact),
                kind: artifact.kind().to_owned(),
                fingerprint: Some(format!(
                    "{}:{}",
                    artifact.fingerprint().algorithm(),
                    artifact.fingerprint().hash()
                )),
                source: Some(artifact.source().to_owned()),
                dependencies,
            }
        })
        .collect();
    crate::ResolvedCompositionGraph {
        root: identity(graph.root()),
        effective_engine: identity(graph.engine()),
        effective_game: Some(identity(graph.game())),
        nodes,
    }
}

fn installed_snapshot(
    installation: &InstallationPaths,
    identities: &[String],
) -> Result<Vec<InstalledArtifact>, String> {
    content::installed_artifacts(installation, identities)?
        .into_iter()
        .map(|artifact| {
            if !artifact.enabled() {
                return Err(format!(
                    "{} is installed but disabled; enable it before using this composition",
                    artifact.artifact_id()
                ));
            }
            Ok(InstalledArtifact {
                id: artifact.artifact_id().to_owned(),
                version: artifact.version().map(ToOwned::to_owned),
                root: artifact.installed_root().to_path_buf(),
                fingerprint: format!(
                    "{}:{}",
                    artifact.fingerprint().algorithm(),
                    artifact.fingerprint().hash()
                ),
            })
        })
        .collect()
}

#[cfg(test)]
mod workshop_acquisition_tests {
    use super::{
        WorkshopDependencyRequest, required_workshop_dependency_requests,
        stable_composition_root_selector,
    };
    use vapor_client_provider::content::ContentReference;

    #[test]
    fn explicit_composition_update_discards_the_previous_exact_root_pin() {
        assert_eq!(
            stable_composition_root_selector("ghf-studios/example/game@0.1.1"),
            "ghf-studios/example/game"
        );
        assert_eq!(
            stable_composition_root_selector("ghf-studios/example/game"),
            "ghf-studios/example/game"
        );
    }

    fn dependencies(source: &str) -> Vec<ContentReference> {
        serde_json::from_str(source).expect("valid dependency fixture")
    }

    #[test]
    fn acquisition_queues_only_missing_required_workshop_dependencies() {
        let dependencies = dependencies(
            r#"[
                {"id":"studio.engine","relationship":"engine","optional":false,"version":">=1.2.0","workshop_id":"101"},
                {"id":"studio.tool","relationship":"dependency","optional":true,"workshop_id":"102"},
                {"id":"studio.conflict","relationship":"conflict","optional":false,"workshop_id":"103"},
                {"id":"studio.cached","relationship":"dependency","optional":false,"workshop_id":"104"}
            ]"#,
        );

        let queued = required_workshop_dependency_requests("studio.game", &dependencies, |edge| {
            Ok(edge.id() == "studio.cached")
        })
        .expect("dependency closure is resolvable");

        assert_eq!(
            queued,
            vec![WorkshopDependencyRequest {
                content_id: "studio.engine".to_owned(),
                workshop_id: "101".to_owned(),
                version: Some(">=1.2.0".to_owned()),
            }]
        );
    }

    #[test]
    fn acquisition_explains_required_dependencies_without_a_download_route() {
        let dependencies =
            dependencies(r#"[{"id":"studio.engine","relationship":"engine","optional":false}]"#);

        let error =
            required_workshop_dependency_requests("studio.game", &dependencies, |_| Ok(false))
                .expect_err("missing required content has no acquisition route");

        assert!(error.contains("studio.game requires studio.engine"));
        assert!(error.contains("no Workshop item ID"));
    }
}
