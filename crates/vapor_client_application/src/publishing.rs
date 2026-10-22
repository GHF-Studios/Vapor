use std::{
    io::Write,
    path::{Path, PathBuf},
};
pub use vapor_client_provider::workspace_build::{BuildMode, BuildReport};

use vapor_client_provider::{
    app_root, content,
    discovery::{EnvironmentPaths, InstallationPaths},
    distribution::{DistributionManifest, StageOptions},
    documentation,
    steam::{self, PublishOptions},
    workspace::{SourceRootKind, WorkspaceManifest},
};

pub const MAX_WORKSHOP_REVIEW_BATCH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopPackagePreview {
    pub id: String,
    pub version: Option<String>,
    pub app_id: Option<u32>,
    pub published_file_id: Option<String>,
    pub title: Option<String>,
    pub visibility: Option<String>,
    pub dependencies: Vec<String>,
    pub root: PathBuf,
    pub target: String,
    pub fingerprint: String,
    pub files: u64,
    pub bytes: u64,
    pub file_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopPublication {
    pub id: String,
    pub published_file_id: Option<String>,
    pub receipt: PathBuf,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopBatchReview {
    pub selectors: Vec<String>,
    pub items: Vec<WorkshopPackagePreview>,
}

/// Reviewed target matrix and exact app-root payload prepared for SteamPipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationReleaseReview {
    pub app_id: u32,
    pub depot_ids: Vec<u32>,
    pub runtime_targets: Vec<String>,
    pub branch: String,
    pub account: String,
    pub description: String,
    pub release_build_validated: bool,
    pub package_root: PathBuf,
    pub build_script: PathBuf,
    pub files: usize,
    pub fingerprint: String,
}

/// Successful completion of an explicitly reviewed SteamPipe release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationReleaseResult {
    pub review: ApplicationReleaseReview,
    pub uploaded: bool,
}

/// Build an authored source workspace with Vapor-managed Cargo and target tools.
pub fn build_authored_workspace(
    application_executable: &Path,
    workspace_root: &Path,
    runtime_targets: &[String],
    mode: BuildMode,
) -> Result<BuildReport, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    vapor_client_provider::workspace_build::run(&paths, mode, runtime_targets)
}

/// Resolve an omitted runtime matrix from the opened workspace manifest.
///
/// Workflows that accept explicit target subsets keep those targets unchanged.
/// An empty selection means the workspace's declared matrix, or Cargo's host
/// default when the workspace declares no runtime targets.
pub fn resolve_workspace_runtime_targets(
    application_executable: &Path,
    workspace_root: &Path,
    requested_targets: &[String],
) -> Result<Vec<String>, String> {
    if !requested_targets.is_empty() {
        return Ok(requested_targets.to_vec());
    }
    let paths = workspace_tool_environment(application_executable, workspace_root)?;
    let manifest = WorkspaceManifest::load(&paths)?;
    Ok(select_runtime_target_matrix(
        requested_targets,
        manifest.runtime_targets(),
    ))
}

fn select_runtime_target_matrix(requested: &[String], declared: &[String]) -> Vec<String> {
    if requested.is_empty() {
        declared.to_vec()
    } else {
        requested.to_vec()
    }
}

/// Build an authored workspace and package one selected content artifact.
///
/// The build and package steps share one operation context so callers can
/// report managed Cargo progress and cancellation through a single workflow.
pub fn build_and_package_authored_content_with_context(
    application_executable: &Path,
    workspace_root: &Path,
    selector: &str,
    runtime_targets: &[String],
    dry_run: bool,
    context: &vapor_core::OperationContext,
) -> Result<content::PackageReport, String> {
    let runtime_targets =
        resolve_workspace_runtime_targets(application_executable, workspace_root, runtime_targets)?;
    run_workspace_tool_with_context(
        application_executable,
        workspace_root,
        vapor_client_domain::WorkspaceToolOperation::Build,
        &vapor_client_domain::WorkspaceProjectSelection::All,
        &runtime_targets,
        context,
    )?;
    context.set_phase(vapor_core::OperationPhase::Staging);
    crate::package_authored_content(
        application_executable,
        workspace_root,
        selector,
        &runtime_targets,
        dry_run,
    )
}

/// Run a Studio workspace tool through Vapor's managed Cargo provider.
pub fn run_workspace_tool(
    application_executable: &Path,
    workspace_root: &Path,
    operation: vapor_client_domain::WorkspaceToolOperation,
    project_selection: &vapor_client_domain::WorkspaceProjectSelection,
    target_triples: &[String],
) -> Result<(), String> {
    run_workspace_tool_with_progress(
        application_executable,
        workspace_root,
        operation,
        project_selection,
        target_triples,
        |_, _, _| {},
    )
}

/// Run a Studio workspace tool and report the completed Cargo-step count.
pub fn run_workspace_tool_with_progress(
    application_executable: &Path,
    workspace_root: &Path,
    operation: vapor_client_domain::WorkspaceToolOperation,
    project_selection: &vapor_client_domain::WorkspaceProjectSelection,
    target_triples: &[String],
    report_progress: impl FnMut(usize, usize, &str),
) -> Result<(), String> {
    run_workspace_tool_with_events(
        application_executable,
        workspace_root,
        operation,
        project_selection,
        target_triples,
        report_progress,
        |stream, text| match stream {
            vapor_core::OperationOutputStream::Stdout => {
                let _ = writeln!(std::io::stdout().lock(), "{text}");
            }
            vapor_core::OperationOutputStream::Stderr => {
                let _ = writeln!(std::io::stderr().lock(), "{text}");
            }
        },
    )
}

/// Run a Studio workspace tool and forward its managed Cargo output.
pub fn run_workspace_tool_with_events(
    application_executable: &Path,
    workspace_root: &Path,
    operation: vapor_client_domain::WorkspaceToolOperation,
    project_selection: &vapor_client_domain::WorkspaceProjectSelection,
    target_triples: &[String],
    report_progress: impl FnMut(usize, usize, &str),
    report_output: impl FnMut(vapor_core::OperationOutputStream, &str),
) -> Result<(), String> {
    let mode = workspace_tool_build_mode(operation);
    let paths = workspace_tool_environment(application_executable, workspace_root)?;
    vapor_client_provider::workspace_build::run_selected_with_events_cancellable(
        &paths,
        mode,
        project_selection,
        target_triples,
        report_progress,
        || false,
        report_output,
    )
    .map(|_| ())
}

/// Run a Studio workspace tool through its shared operation context.
pub fn run_workspace_tool_with_context(
    application_executable: &Path,
    workspace_root: &Path,
    operation: vapor_client_domain::WorkspaceToolOperation,
    project_selection: &vapor_client_domain::WorkspaceProjectSelection,
    target_triples: &[String],
    context: &vapor_core::OperationContext,
) -> Result<(), String> {
    let mode = workspace_tool_build_mode(operation);
    let paths = workspace_tool_environment(application_executable, workspace_root)?;
    vapor_client_provider::workspace_build::run_selected_with_events_cancellable(
        &paths,
        mode,
        project_selection,
        target_triples,
        |completed, total, message| {
            context.report_progress(vapor_core::Progress {
                completed: completed as u64,
                total: Some(total as u64),
                unit: "Cargo steps".to_owned(),
                message: Some(message.to_owned()),
            });
        },
        || context.is_cancelled(),
        |stream, text| {
            context.report_output(vapor_core::OperationOutput {
                stream,
                level: vapor_core::ConsoleLevel::Info,
                process: Some("cargo".to_owned()),
                text: text.to_owned(),
            });
        },
    )
    .map(|_| ())
}

/// Clean the managed development outputs for every Cargo project in a workspace.
pub fn clean_workspace_build_cache_with_context(
    application_executable: &Path,
    workspace_root: &Path,
    context: &vapor_core::OperationContext,
) -> Result<(), String> {
    let paths = workspace_tool_environment(application_executable, workspace_root)?;
    let manifest = WorkspaceManifest::load(&paths)?;
    vapor_client_provider::workspace_build::clean_development_cache(
        &paths,
        || context.is_cancelled(),
        |completed, total, project| {
            context.report_progress(vapor_core::Progress {
                completed: completed as u64,
                total: Some(total as u64),
                unit: "Cargo projects".to_owned(),
                message: Some(format!("Cleared development build output for {project}")),
            });
        },
    )?;

    // The installed app-root dev outputs and the authoring user-data targets
    // are separate caches. Derive the latter only from this source root's
    // validated Cargo manifest entries, never from every workspace in the
    // surrounding Superworkspace.
    let workspace_roots = match manifest.kind() {
        SourceRootKind::Root => manifest
            .cargo_projects()
            .iter()
            .map(|project| {
                paths
                    .source()
                    .root()
                    .join(project.manifest())
                    .parent()
                    .map(Path::to_path_buf)
                    .ok_or_else(|| {
                        format!(
                            "Cargo manifest '{}' has no workspace directory",
                            project.manifest().display()
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?,
        SourceRootKind::Workspace => vec![paths.source().root().to_path_buf()],
    };
    if workspace_roots.is_empty() {
        return Err(format!(
            "source root '{}' has no Cargo workspaces to clean",
            manifest.id()
        ));
    }
    for root in workspace_roots {
        vapor_core::run_workspace_operation_at_with_context(
            &root,
            vapor_core::DevelopmentOperation::Clean,
            context,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn workspace_tool_build_mode(operation: vapor_client_domain::WorkspaceToolOperation) -> BuildMode {
    match operation {
        vapor_client_domain::WorkspaceToolOperation::Format => BuildMode::Format,
        vapor_client_domain::WorkspaceToolOperation::Check => BuildMode::Check,
        vapor_client_domain::WorkspaceToolOperation::Test => BuildMode::Test,
        vapor_client_domain::WorkspaceToolOperation::Build => BuildMode::Build,
        vapor_client_domain::WorkspaceToolOperation::BuildRelease => BuildMode::BuildRelease,
        vapor_client_domain::WorkspaceToolOperation::Validate => BuildMode::Validate,
    }
}

fn workspace_tool_environment(
    application_executable: &Path,
    workspace_root: &Path,
) -> Result<EnvironmentPaths, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)
}

/// Validate, build, and promote the source workspace's complete release matrix.
pub fn build_application_release(
    application_executable: &Path,
    workspace_root: &Path,
    runtime_targets: &[String],
) -> Result<vapor_client_provider::release::ReleaseBuildReport, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    vapor_client_provider::release::build_application_release(&paths, runtime_targets)
}

/// Build a release while forwarding shared operation progress and output.
pub fn build_application_release_with_events(
    application_executable: &Path,
    workspace_root: &Path,
    runtime_targets: &[String],
    report_progress: impl FnMut(usize, usize, &str),
    report_output: impl FnMut(vapor_core::OperationOutputStream, &str),
) -> Result<vapor_client_provider::release::ReleaseBuildReport, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    vapor_client_provider::release::build_application_release_with_events(
        &paths,
        runtime_targets,
        report_progress,
        report_output,
    )
}

/// Validate an app-release request before starting its potentially long build.
pub fn validate_application_release_request(
    application_executable: &Path,
    workspace_root: &Path,
    runtime_targets: &[String],
    branch: Option<&str>,
    description: &str,
    account: &str,
) -> Result<(), String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    let manifest = DistributionManifest::load(&paths)?;
    let branch = branch
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .unwrap_or_else(|| manifest.application().development_branch());
    validate_release_inputs(account, branch, description, runtime_targets)
}

/// Stage a source workspace's declared app distribution for review without uploading.
pub fn stage_application_release(
    application_executable: &Path,
    workspace_root: &Path,
    runtime_targets: &[String],
    branch: Option<&str>,
    description: &str,
    account: &str,
    release_build_validated: bool,
) -> Result<ApplicationReleaseReview, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    let manifest = DistributionManifest::load(&paths)?;
    let workspace_manifest = WorkspaceManifest::load(&paths)?;
    let branch = branch
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .unwrap_or_else(|| manifest.application().development_branch());
    validate_release_inputs(account, branch, description, runtime_targets)?;
    documentation::build(&paths, &workspace_manifest)?;
    app_root::sync_release_assets(&paths, runtime_targets)?;
    let mut review = stage_application_release_with_manifest(
        &paths,
        &manifest,
        runtime_targets,
        branch,
        description,
        account,
        None,
    )?;
    review.release_build_validated = release_build_validated;
    Ok(review)
}

/// Upload only if the freshly staged app payload still matches its service review.
pub fn publish_reviewed_application_release(
    application_executable: &Path,
    workspace_root: &Path,
    reviewed: &ApplicationReleaseReview,
) -> Result<ApplicationReleaseResult, String> {
    if !reviewed.release_build_validated {
        return Err("Steam upload requires a service-validated release build".to_owned());
    }
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    let manifest = DistributionManifest::load(&paths)?;
    validate_release_inputs(
        &reviewed.account,
        &reviewed.branch,
        &reviewed.description,
        &reviewed.runtime_targets,
    )?;
    let mut current = stage_application_release_with_manifest(
        &paths,
        &manifest,
        &reviewed.runtime_targets,
        &reviewed.branch,
        &reviewed.description,
        &reviewed.account,
        None,
    )?;
    current.release_build_validated = true;
    if &current != reviewed {
        return Err(
            "the Steam app release changed after review; stage and confirm it again".to_owned(),
        );
    }
    let mut uploaded = stage_application_release_with_manifest(
        &paths,
        &manifest,
        &reviewed.runtime_targets,
        &reviewed.branch,
        &reviewed.description,
        &reviewed.account,
        Some(&reviewed.fingerprint),
    )?;
    uploaded.release_build_validated = true;
    Ok(ApplicationReleaseResult {
        review: uploaded,
        uploaded: true,
    })
}

fn stage_application_release_with_manifest(
    paths: &EnvironmentPaths,
    manifest: &DistributionManifest,
    runtime_targets: &[String],
    branch: &str,
    description: &str,
    account: &str,
    expected_fingerprint: Option<&str>,
) -> Result<ApplicationReleaseReview, String> {
    let options = StageOptions::runtime().with_runtime_targets(runtime_targets.to_vec());
    let report = steam::publish(
        paths,
        manifest,
        PublishOptions {
            account,
            branch: Some(branch),
            description,
            stage_options: options,
            dry_run: expected_fingerprint.is_none(),
            confirmed: expected_fingerprint.is_some(),
            expected_fingerprint,
        },
    )?;
    Ok(ApplicationReleaseReview {
        app_id: manifest.application().app_id(),
        depot_ids: report
            .stage()
            .depots()
            .iter()
            .map(|depot| manifest.application().depot_id(depot.kind()))
            .collect(),
        runtime_targets: runtime_targets.to_vec(),
        branch: branch.to_owned(),
        account: account.to_owned(),
        description: description.to_owned(),
        release_build_validated: false,
        package_root: report.stage().root().to_path_buf(),
        build_script: report.script().to_path_buf(),
        files: report.stage().files(),
        fingerprint: report.fingerprint().to_owned(),
    })
}

fn validate_release_inputs(
    account: &str,
    branch: &str,
    description: &str,
    runtime_targets: &[String],
) -> Result<(), String> {
    if account.trim().is_empty() || account.len() > 128 {
        return Err("Steamworks account must contain 1 to 128 bytes".to_owned());
    }
    if branch.is_empty()
        || branch.len() > 64
        || branch == "default"
        || !branch
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    {
        return Err("Steam release branch must be a valid non-default branch".to_owned());
    }
    if description.trim().is_empty() || description.len() > 256 {
        return Err("Steam release description must contain 1 to 256 bytes".to_owned());
    }
    if runtime_targets.is_empty()
        || runtime_targets.len() > 32
        || runtime_targets.iter().any(|target| {
            target.is_empty()
                || target.len() > 128
                || target.trim() != target
                || !target
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
        })
    {
        return Err("Choose 1 to 32 valid runtime target triples".to_owned());
    }
    let unique = runtime_targets
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    if unique.len() != runtime_targets.len() {
        return Err("Runtime target triples must be unique".to_owned());
    }
    Ok(())
}

/// Stage source content and expose the exact artifact facts a frontend can review.
/// This operation never uploads to Workshop.
pub fn stage_workshop_preview(
    application_executable: &Path,
    workspace_root: &Path,
    selector: &str,
    target_triples: &[String],
) -> Result<WorkshopPackagePreview, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    let catalog = content::discover(&paths)?;
    let artifact = catalog
        .find(selector)
        .ok_or_else(|| format!("unknown content artifact '{selector}'"))?;
    let package = content::package_for_targets(&paths, selector, false, target_triples)?;
    let fingerprint = package.fingerprint();
    let file_paths = staged_files(package.root())?;
    Ok(WorkshopPackagePreview {
        id: artifact.id().to_owned(),
        version: artifact.version().map(ToOwned::to_owned),
        app_id: artifact.workshop().app_id(),
        published_file_id: artifact
            .workshop()
            .published_file_id()
            .map(ToOwned::to_owned),
        title: artifact.workshop().title().map(ToOwned::to_owned),
        visibility: artifact.workshop().visibility().map(ToOwned::to_owned),
        dependencies: artifact
            .dependencies()
            .iter()
            .map(|dependency| {
                format!(
                    "{} {}",
                    dependency.id(),
                    dependency.version().unwrap_or("any version")
                )
            })
            .collect(),
        root: package.root().to_path_buf(),
        target: package.runtime_target().to_owned(),
        fingerprint: format!("{}:{}", fingerprint.algorithm(), fingerprint.hash()),
        files: fingerprint.files(),
        bytes: fingerprint.bytes(),
        file_paths,
    })
}

/// Publish the reviewed source artifact through Shell after confirming the
/// staged content still matches the review shown to the caller.
pub fn publish_reviewed_workshop_content(
    application_executable: &Path,
    workspace_root: &Path,
    selector: &str,
    expected_review: &WorkshopPackagePreview,
    target_triples: &[String],
    account: &str,
    change_note: Option<&str>,
) -> Result<WorkshopPublication, String> {
    let current = stage_workshop_preview(
        application_executable,
        workspace_root,
        selector,
        target_triples,
    )?;
    if &current != expected_review {
        return Err(format!(
            "{} changed after review; stage a fresh Workshop review before uploading",
            current.id
        ));
    }
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    let report = if current.published_file_id.is_some() {
        content::publish_workshop_item_for_targets_reviewed(
            &paths,
            selector,
            &current.fingerprint,
            account,
            target_triples,
            change_note,
            true,
        )?
    } else {
        content::create_workshop_item_for_targets_reviewed(
            &paths,
            selector,
            &current.fingerprint,
            account,
            target_triples,
            true,
        )?
    };
    if !report.uploaded() {
        return Err(format!(
            "Workshop upload for {} did not complete",
            current.id
        ));
    }
    Ok(WorkshopPublication {
        id: report.artifact_id().to_owned(),
        published_file_id: report.published_file_id().map(ToOwned::to_owned),
        receipt: report.receipt().to_path_buf(),
        created: current.published_file_id.is_none(),
    })
}

/// Stage a bounded batch of Workshop packages for one shared review.
pub fn stage_workshop_batch_preview(
    application_executable: &Path,
    workspace_root: &Path,
    selectors: &[String],
    target_triples: &[String],
) -> Result<WorkshopBatchReview, String> {
    if selectors.is_empty() || selectors.len() > MAX_WORKSHOP_REVIEW_BATCH {
        return Err(format!(
            "Workshop review batches must contain 1 to {MAX_WORKSHOP_REVIEW_BATCH} items"
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    if selectors.iter().any(|selector| !seen.insert(selector)) {
        return Err("Workshop review batch contains duplicate content identities".to_owned());
    }
    let items = selectors
        .iter()
        .map(|selector| {
            stage_workshop_preview(
                application_executable,
                workspace_root,
                selector,
                target_triples,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut published_ids = std::collections::BTreeSet::new();
    if items.iter().any(|item| {
        item.published_file_id
            .as_ref()
            .is_some_and(|published_id| !published_ids.insert(published_id))
    }) {
        return Err(
            "Workshop review batch contains multiple artifacts targeting the same published item"
                .to_owned(),
        );
    }
    Ok(WorkshopBatchReview {
        selectors: selectors.to_vec(),
        items,
    })
}

/// Publish a batch only when every staged item still exactly matches review.
pub fn publish_reviewed_workshop_batch(
    application_executable: &Path,
    workspace_root: &Path,
    expected_review: &WorkshopBatchReview,
    target_triples: &[String],
    account: &str,
    change_note: Option<&str>,
) -> Result<Vec<WorkshopPublication>, String> {
    let current = stage_workshop_batch_preview(
        application_executable,
        workspace_root,
        &expected_review.selectors,
        target_triples,
    )?;
    if &current != expected_review {
        let changed = expected_review
            .items
            .iter()
            .zip(&current.items)
            .find(|(reviewed, current)| reviewed != current)
            .map(|(reviewed, _)| reviewed.id.as_str())
            .unwrap_or("Workshop batch");
        return Err(format!(
            "{changed} changed after review; stage a fresh Workshop review before uploading"
        ));
    }
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    let fingerprints = expected_review
        .selectors
        .iter()
        .zip(&expected_review.items)
        .map(|(selector, item)| (selector.clone(), item.fingerprint.clone()))
        .collect::<Vec<_>>();
    let reports = content::publish_workshop_items_for_targets_reviewed(
        &paths,
        &expected_review.selectors,
        &fingerprints,
        account,
        target_triples,
        change_note,
        true,
    )?;
    if reports.len() != expected_review.items.len()
        || reports.iter().any(|report| !report.uploaded())
    {
        return Err("Workshop batch upload did not complete for every reviewed item".to_owned());
    }
    reports
        .into_iter()
        .zip(&expected_review.items)
        .map(|(report, reviewed)| {
            if report.artifact_id() != reviewed.id {
                return Err(
                    "Workshop provider returned batch receipts in a different identity order"
                        .to_owned(),
                );
            }
            Ok(WorkshopPublication {
                id: report.artifact_id().to_owned(),
                published_file_id: report.published_file_id().map(ToOwned::to_owned),
                receipt: report.receipt().to_path_buf(),
                created: reviewed.published_file_id.is_none(),
            })
        })
        .collect()
}

fn staged_files(root: &Path) -> Result<Vec<String>, String> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)
            .map_err(|error| format!("{}: {error}", directory.display()))?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|error| error.to_string())?
                    .display()
                    .to_string();
                files.push(relative);
                if files.len() > 10_000 {
                    return Err("staged package contains more than 10,000 files".to_owned());
                }
            }
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::{select_runtime_target_matrix, validate_release_inputs};

    fn valid_targets() -> Vec<String> {
        vec!["x86_64-unknown-linux-gnu".to_owned()]
    }

    #[test]
    fn application_release_rejects_default_branch_and_duplicate_targets() {
        assert!(
            validate_release_inputs("builder", "default", "release", &valid_targets()).is_err()
        );

        let duplicate_targets = vec![
            "x86_64-unknown-linux-gnu".to_owned(),
            "x86_64-unknown-linux-gnu".to_owned(),
        ];
        assert!(
            validate_release_inputs("builder", "vapor-dev", "release", &duplicate_targets).is_err()
        );
    }

    #[test]
    fn application_release_accepts_a_named_beta_branch_and_target_matrix() {
        assert!(
            validate_release_inputs(
                "builder",
                "vapor-dev",
                "release candidate",
                &valid_targets()
            )
            .is_ok()
        );
    }

    #[test]
    fn omitted_runtime_targets_inherit_the_workspace_matrix() {
        let declared = vec![
            "x86_64-unknown-linux-gnu".to_owned(),
            "x86_64-pc-windows-gnullvm".to_owned(),
        ];

        assert_eq!(select_runtime_target_matrix(&[], &declared), declared);
    }

    #[test]
    fn explicit_runtime_target_subset_overrides_the_workspace_matrix() {
        let declared = vec![
            "x86_64-unknown-linux-gnu".to_owned(),
            "x86_64-pc-windows-gnullvm".to_owned(),
        ];
        let requested = vec!["x86_64-unknown-linux-gnu".to_owned()];

        assert_eq!(
            select_runtime_target_matrix(&requested, &declared),
            requested
        );
    }

    #[test]
    fn empty_workspace_matrix_preserves_host_default_semantics() {
        assert!(select_runtime_target_matrix(&[], &[]).is_empty());
    }
}
