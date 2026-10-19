use std::path::{Path, PathBuf};
use vapor_client_provider::{
    content,
    discovery::{EnvironmentPaths, InstallationPaths},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationCheck {
    pub ok: bool,
    pub detail: Vec<String>,
    pub artifacts: Vec<InstalledArtifact>,
    pub graph: Option<crate::ResolvedCompositionGraph>,
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
/// The items are downloaded for this operation without changing the user's
/// subscription list, then copied into Vapor's app-owned cache.
pub fn download_workshop_items(
    application_executable: &Path,
    published_file_ids: &[String],
) -> Result<Vec<WorkshopAcquisition>, String> {
    download_workshop_items_with_constraints(application_executable, published_file_ids, &Default::default())
}

/// Download Workshop items while preserving Auto-Install composition
/// constraints during cache replacement.
pub fn download_workshop_items_with_constraints(
    application_executable: &Path,
    published_file_ids: &[String],
    constraints: &std::collections::BTreeMap<String, Vec<String>>,
) -> Result<Vec<WorkshopAcquisition>, String> {
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
    let mut grouped = std::collections::BTreeMap::<u32, Vec<(usize, String)>>::new();
    for (index, id) in normalized_ids.iter().enumerate() {
        let app_id = content::workshop_app_id(&installation, None, id, id)?;
        grouped.entry(app_id).or_default().push((index, id.clone()));
    }
    let mut acquisitions = Vec::with_capacity(normalized_ids.len());
    for (app_id, items) in grouped {
        let ids = items
            .iter()
            .map(|(_, id)| {
                id.parse::<u64>()
                    .map_err(|_| format!("Workshop item ID '{id}' is not a valid number"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let positions = items
            .iter()
            .map(|(index, id)| {
                id.parse::<u64>()
                    .map(|published_file_id| (published_file_id, *index))
                    .map_err(|_| format!("Workshop item ID '{id}' is not a valid number"))
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>();
        let imported = vapor_steam::with_downloaded_workshop_items(
            app_id,
            &ids,
            |published_file_id, root| {
                let id = published_file_id.to_string();
                let report =
                    content::import_workshop_item_from_provider_with_constraints(
                        &installation,
                        app_id,
                        &id,
                        root,
                        constraints,
                    )?;
                let cached = content::cached_artifact(&installation, report.artifact_id())?;
                Ok((
                    *positions.get(&published_file_id).ok_or_else(|| {
                        format!("Steam returned an unexpected Workshop item {id}")
                    })?,
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
                ))
            },
        )?;
        acquisitions.extend(imported);
    }
    acquisitions.sort_by_key(|(index, _)| *index);
    Ok(acquisitions
        .into_iter()
        .map(|(_, acquisition)| acquisition)
        .collect())
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
    Ok(InstallationCheck {
        ok,
        detail,
        artifacts,
        graph,
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
    content::update(&installation, paths.as_ref(), Some(selector))?;
    installed_composition(&installation, selector)
}

/// Remove one unreferenced cached artifact from the local Library while
/// preserving any installed runtime copy.
pub fn collect_library_item(application_executable: &Path, content_id: &str) -> Result<bool, String> {
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
            .and_then(|plan| {
                content::run_packagepack_launch(&plan, "studio-composition", arguments)
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
