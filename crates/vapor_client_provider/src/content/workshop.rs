//! Workshop item packaging and upload operations.
//!
//! This module owns item publication scripts and SteamCMD transport. Search,
//! subscription, and acquisition orchestration are separate service concerns.

use super::*;
use vapor_content::authored_manifest::AuthoredContentManifest;

/// Write a Workshop create preview receipt.
///
/// Real item creation requires a live SteamUGC provider and remains a manual
/// authority-changing operation.
///
/// # Errors
///
/// Returns discovery or receipt errors.
pub fn create_workshop_item(
    paths: &EnvironmentPaths,
    selector: &str,
    account: Option<&str>,
    dry_run: bool,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    create_workshop_item_for_target(paths, selector, account, None, dry_run, confirmed)
}

/// Write or upload a Workshop create operation with an explicit runtime target.
///
/// # Errors
///
/// Returns discovery, packaging, provider, or receipt errors.
pub fn create_workshop_item_for_target(
    paths: &EnvironmentPaths,
    selector: &str,
    account: Option<&str>,
    target_triple: Option<&str>,
    dry_run: bool,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    let targets = target_triple
        .map(|target| vec![target.to_owned()])
        .unwrap_or_default();
    create_workshop_item_for_targets(paths, selector, account, &targets, dry_run, confirmed)
}

/// Write or upload a Workshop create operation with explicit runtime targets.
///
/// # Errors
///
/// Returns discovery, packaging, provider, or receipt errors.
pub fn create_workshop_item_for_targets(
    paths: &EnvironmentPaths,
    selector: &str,
    account: Option<&str>,
    target_triples: &[String],
    dry_run: bool,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    create_workshop_item_for_targets_checked(
        paths,
        selector,
        account,
        target_triples,
        dry_run,
        confirmed,
        None,
    )
}

/// Create a Workshop item only if the package staged for upload matches the
/// exact fingerprint previously reviewed by the caller.
pub fn create_workshop_item_reviewed(
    paths: &EnvironmentPaths,
    selector: &str,
    expected_fingerprint: &str,
    account: &str,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    create_workshop_item_for_targets_reviewed(
        paths,
        selector,
        expected_fingerprint,
        account,
        &[],
        confirmed,
    )
}

/// Create a Workshop item only if the multi-target package staged for upload
/// matches the exact fingerprint previously reviewed by the caller.
pub fn create_workshop_item_for_targets_reviewed(
    paths: &EnvironmentPaths,
    selector: &str,
    expected_fingerprint: &str,
    account: &str,
    target_triples: &[String],
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    create_workshop_item_for_targets_checked(
        paths,
        selector,
        Some(account),
        target_triples,
        false,
        confirmed,
        Some(expected_fingerprint),
    )
}

fn create_workshop_item_for_targets_checked(
    paths: &EnvironmentPaths,
    selector: &str,
    account: Option<&str>,
    target_triples: &[String],
    dry_run: bool,
    confirmed: bool,
    expected_fingerprint: Option<&str>,
) -> Result<WorkshopOperationReport, String> {
    let catalog = discover(paths)?;
    let artifact = catalog.find(selector).ok_or_else(|| {
        format!("unknown content artifact '{selector}'\nhelp: inspect source content with `content list`")
    })?;
    if artifact.workshop().app_id().is_none() {
        return Err(format!(
            "{} has no [{}.steam].app-id and cannot be created on Workshop",
            artifact.id(),
            artifact.kind()
        ));
    }
    if !dry_run && artifact.workshop().published_file_id().is_some() {
        return Err(format!(
            "{} already has PublishedFileId {}; use `content publish` for updates",
            artifact.id(),
            artifact.workshop().published_file_id().expect("checked")
        ));
    }
    if !dry_run && account.unwrap_or("").trim().is_empty() {
        return Err(
            "real Workshop creation requires --account ACCOUNT after reviewing --dry-run"
                .to_owned(),
        );
    }
    if !dry_run && !confirmed {
        return Err("real Workshop creation requires --yes after reviewing --dry-run".to_owned());
    }

    let package = package_for_targets(paths, artifact.id(), false, target_triples)?;
    let (upload_root, _reviewed_stage, package_fingerprint) =
        if let Some(expected) = expected_fingerprint {
            let layout = ContentLayout::new(paths.installation());
            let staged = TemporaryDirectory::new(
                layout.output_temporary(),
                "vapor-workshop-reviewed",
                artifact.id(),
            )?;
            copy_tree(package.root(), staged.path(), package.root(), &[])?;
            let fingerprint = fingerprint_tree(staged.path())?;
            let fingerprint = format!("{}:{}", fingerprint.algorithm(), fingerprint.hash());
            if fingerprint != *expected {
                return Err(format!(
                    "{} changed after review; stage a fresh Workshop review before uploading",
                    artifact.id()
                ));
            }
            let upload_root = staged.path().to_path_buf();
            (upload_root, Some(staged), fingerprint)
        } else {
            let fingerprint = format!(
                "{}:{}",
                package.fingerprint().algorithm(),
                package.fingerprint().hash()
            );
            (package.root().to_path_buf(), None, fingerprint)
        };
    let script = write_workshop_script(paths, artifact, &upload_root, None, dry_run)?;
    if dry_run {
        let receipt = write_receipt(
            paths.installation(),
            "workshop-create",
            artifact.id(),
            "dry-run",
            Some(&format!(
                "script={}; fingerprint={package_fingerprint}",
                script.display()
            )),
        )?;
        return Ok(WorkshopOperationReport {
            artifact_id: artifact.id().to_owned(),
            script: Some(script),
            receipt,
            uploaded: false,
            published_file_id: None,
        });
    }

    run_steamcmd_workshop_build(paths, account.expect("account checked"), &script)?;
    let published_file_id = read_workshop_script_published_file_id(&script)?;
    if published_file_id == "0" {
        return Err(format!(
            "SteamCMD did not write a PublishedFileId into '{}'",
            script.display()
        ));
    }
    record_published_file_id(artifact, &published_file_id)?;
    let receipt = write_receipt(
        paths.installation(),
        "workshop-create",
        artifact.id(),
        "created",
        Some(&format!(
            "script={}; published-file-id={}; fingerprint={package_fingerprint}",
            script.display(),
            published_file_id
        )),
    )?;
    Ok(WorkshopOperationReport {
        artifact_id: artifact.id().to_owned(),
        script: Some(script),
        receipt,
        uploaded: true,
        published_file_id: Some(published_file_id),
    })
}

/// Publish or preview a Workshop item update.
///
/// Dry-runs write the package and SteamCMD Workshop VDF. Real updates require
/// `account` and `confirmed` and use the controlled SteamCMD provider.
///
/// # Errors
///
/// Returns package, provider, SteamCMD, or authority errors.
pub fn publish_workshop_item(
    paths: &EnvironmentPaths,
    selector: &str,
    account: Option<&str>,
    change_note: Option<&str>,
    dry_run: bool,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    publish_workshop_item_for_target(
        paths,
        selector,
        account,
        None,
        change_note,
        dry_run,
        confirmed,
    )
}

/// Publish or preview a Workshop item update for one runtime target.
///
/// # Errors
///
/// Returns package, provider, SteamCMD, or authority errors.
pub fn publish_workshop_item_for_target(
    paths: &EnvironmentPaths,
    selector: &str,
    account: Option<&str>,
    target_triple: Option<&str>,
    change_note: Option<&str>,
    dry_run: bool,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    let targets = target_triple
        .map(|target| vec![target.to_owned()])
        .unwrap_or_default();
    publish_workshop_item_for_targets(
        paths,
        selector,
        account,
        &targets,
        change_note,
        dry_run,
        confirmed,
    )
}

/// Publish or preview a Workshop item update for runtime targets.
///
/// # Errors
///
/// Returns package, provider, SteamCMD, or authority errors.
pub fn publish_workshop_item_for_targets(
    paths: &EnvironmentPaths,
    selector: &str,
    account: Option<&str>,
    target_triples: &[String],
    change_note: Option<&str>,
    dry_run: bool,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    publish_workshop_items_for_targets(
        paths,
        &[selector.to_owned()],
        account,
        target_triples,
        change_note,
        dry_run,
        confirmed,
    )
    .and_then(|mut reports| {
        reports
            .pop()
            .ok_or_else(|| "no Workshop item was published".to_owned())
    })
}

/// Publish or preview one or more Workshop item updates.
///
/// Multiple real uploads are sent through one SteamCMD process so the authority
/// boundary remains a single interactive provider session.
///
/// # Errors
///
/// Returns discovery, packaging, receipt, or provider errors.
pub fn publish_workshop_items(
    paths: &EnvironmentPaths,
    selectors: &[String],
    account: Option<&str>,
    change_note: Option<&str>,
    dry_run: bool,
    confirmed: bool,
) -> Result<Vec<WorkshopOperationReport>, String> {
    publish_workshop_items_for_target(
        paths,
        selectors,
        account,
        None,
        change_note,
        dry_run,
        confirmed,
    )
}

/// Publish or preview one or more Workshop item updates for one runtime target.
///
/// Multiple real uploads are sent through one SteamCMD process so the authority
/// boundary remains a single interactive provider session.
///
/// # Errors
///
/// Returns discovery, packaging, receipt, or provider errors.
pub fn publish_workshop_items_for_target(
    paths: &EnvironmentPaths,
    selectors: &[String],
    account: Option<&str>,
    target_triple: Option<&str>,
    change_note: Option<&str>,
    dry_run: bool,
    confirmed: bool,
) -> Result<Vec<WorkshopOperationReport>, String> {
    let targets = target_triple
        .map(|target| vec![target.to_owned()])
        .unwrap_or_default();
    publish_workshop_items_for_targets(
        paths,
        selectors,
        account,
        &targets,
        change_note,
        dry_run,
        confirmed,
    )
}

/// Publish or preview one or more Workshop item updates for runtime targets.
///
/// Multiple real uploads are sent through one SteamCMD process so the authority
/// boundary remains a single interactive provider session.
///
/// # Errors
///
/// Returns discovery, packaging, receipt, or provider errors.
pub fn publish_workshop_items_for_targets(
    paths: &EnvironmentPaths,
    selectors: &[String],
    account: Option<&str>,
    target_triples: &[String],
    change_note: Option<&str>,
    dry_run: bool,
    confirmed: bool,
) -> Result<Vec<WorkshopOperationReport>, String> {
    publish_workshop_items_for_targets_checked(
        paths,
        WorkshopPublishOptions {
            selectors,
            account,
            target_triples,
            change_note,
            dry_run,
            confirmed,
            expected_fingerprints: &[],
        },
    )
}

/// Update one Workshop item only if the package staged for upload matches the
/// exact fingerprint previously reviewed by the caller.
pub fn publish_workshop_item_reviewed(
    paths: &EnvironmentPaths,
    selector: &str,
    expected_fingerprint: &str,
    account: &str,
    change_note: Option<&str>,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    publish_workshop_item_for_targets_reviewed(
        paths,
        selector,
        expected_fingerprint,
        account,
        &[],
        change_note,
        confirmed,
    )
}

/// Update one Workshop item only if its multi-target package matches the
/// exact fingerprint previously reviewed by the caller.
pub fn publish_workshop_item_for_targets_reviewed(
    paths: &EnvironmentPaths,
    selector: &str,
    expected_fingerprint: &str,
    account: &str,
    target_triples: &[String],
    change_note: Option<&str>,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    let expected = [(selector.to_owned(), expected_fingerprint.to_owned())];
    let mut reports = publish_workshop_items_for_targets_checked(
        paths,
        WorkshopPublishOptions {
            selectors: &[selector.to_owned()],
            account: Some(account),
            target_triples,
            change_note,
            dry_run: false,
            confirmed,
            expected_fingerprints: &expected,
        },
    )?;
    reports
        .pop()
        .ok_or_else(|| "no Workshop item was published".to_owned())
}

/// Publish a reviewed batch while keeping each package bound to its reviewed
/// fingerprint. All uploads share one SteamCMD session.
pub fn publish_workshop_items_for_targets_reviewed(
    paths: &EnvironmentPaths,
    selectors: &[String],
    expected_fingerprints: &[(String, String)],
    account: &str,
    target_triples: &[String],
    change_note: Option<&str>,
    confirmed: bool,
) -> Result<Vec<WorkshopOperationReport>, String> {
    if selectors.len() != expected_fingerprints.len() {
        return Err("reviewed Workshop batch does not match its staged fingerprints".to_owned());
    }
    let expected = expected_fingerprints
        .iter()
        .map(|(selector, fingerprint)| (selector.as_str(), fingerprint.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let unique_selectors = selectors
        .iter()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    if expected.len() != selectors.len()
        || unique_selectors.len() != selectors.len()
        || selectors
            .iter()
            .any(|selector| !expected.contains_key(selector.as_str()))
    {
        return Err(
            "reviewed Workshop batch identities do not match their staged fingerprints".to_owned(),
        );
    }
    publish_workshop_items_for_targets_checked(
        paths,
        WorkshopPublishOptions {
            selectors,
            account: Some(account),
            target_triples,
            change_note,
            dry_run: false,
            confirmed,
            expected_fingerprints,
        },
    )
}

struct WorkshopPublishOptions<'a> {
    selectors: &'a [String],
    account: Option<&'a str>,
    target_triples: &'a [String],
    change_note: Option<&'a str>,
    dry_run: bool,
    confirmed: bool,
    expected_fingerprints: &'a [(String, String)],
}

fn publish_workshop_items_for_targets_checked(
    paths: &EnvironmentPaths,
    options: WorkshopPublishOptions<'_>,
) -> Result<Vec<WorkshopOperationReport>, String> {
    let WorkshopPublishOptions {
        selectors,
        account,
        target_triples,
        change_note,
        dry_run,
        confirmed,
        expected_fingerprints,
    } = options;
    if selectors.is_empty() {
        return Err("at least one Workshop artifact is required".to_owned());
    }
    let catalog = discover(paths)?;
    if !dry_run && account.unwrap_or("").trim().is_empty() {
        return Err(
            "real Workshop publication requires --account ACCOUNT after reviewing --dry-run"
                .to_owned(),
        );
    }
    if !dry_run && !confirmed {
        return Err(
            "real Workshop publication requires --yes after reviewing --dry-run".to_owned(),
        );
    }

    let mut staged = Vec::new();
    let mut reviewed_stages = Vec::new();
    for selector in selectors {
        let artifact = catalog.find(selector).ok_or_else(|| {
            format!(
                "unknown content artifact '{selector}'\nhelp: inspect source content with `content list`"
            )
        })?;
        if artifact.workshop().app_id().is_none() {
            return Err(format!(
                "{} has no [{}.steam].app-id and cannot be published",
                artifact.id(),
                artifact.kind()
            ));
        }
        if artifact.workshop().published_file_id().is_none() && !dry_run {
            return Err(format!(
                "{} has no PublishedFileId; run `content create {} --dry-run` and create the item manually before a real update",
                artifact.id(),
                artifact.name()
            ));
        }

        let package = package_for_targets(paths, artifact.id(), false, target_triples)?;
        let mut package_fingerprint = format!(
            "{}:{}",
            package.fingerprint().algorithm(),
            package.fingerprint().hash()
        );
        let mut upload_root = package.root().to_path_buf();
        if let Some((_, expected)) = expected_fingerprints
            .iter()
            .find(|(reviewed_selector, _)| selector == reviewed_selector)
        {
            let layout = ContentLayout::new(paths.installation());
            let reviewed = TemporaryDirectory::new(
                layout.output_temporary(),
                "vapor-workshop-reviewed",
                artifact.id(),
            )?;
            copy_tree(package.root(), reviewed.path(), package.root(), &[])?;
            let fingerprint = fingerprint_tree(reviewed.path())?;
            let fingerprint = format!("{}:{}", fingerprint.algorithm(), fingerprint.hash());
            if fingerprint.as_str() != expected.as_str() {
                return Err(format!(
                    "{} changed after review; stage a fresh Workshop review before uploading",
                    artifact.id()
                ));
            }
            package_fingerprint = fingerprint;
            upload_root = reviewed.path().to_path_buf();
            reviewed_stages.push(reviewed);
        }
        let script = write_workshop_script(paths, artifact, &upload_root, change_note, dry_run)?;
        staged.push((
            artifact.id().to_owned(),
            script,
            artifact
                .workshop()
                .published_file_id()
                .map(ToOwned::to_owned),
            package_fingerprint,
        ));
    }

    if !dry_run {
        let scripts = staged
            .iter()
            .map(|(_, script, _, _)| script.clone())
            .collect::<Vec<_>>();
        run_steamcmd_workshop_builds(paths, account.expect("account checked"), &scripts)?;
    }

    let receipt_status = if dry_run { "dry-run" } else { "uploaded" };
    let mut reports = Vec::new();
    for (artifact_id, script, published_file_id, fingerprint) in staged {
        let receipt = write_receipt(
            paths.installation(),
            "workshop-publish",
            &artifact_id,
            receipt_status,
            Some(&format!(
                "script={}; fingerprint={fingerprint}",
                script.display()
            )),
        )?;
        reports.push(WorkshopOperationReport {
            artifact_id,
            script: Some(script),
            receipt,
            uploaded: !dry_run,
            published_file_id,
        });
    }
    Ok(reports)
}

/// Preview a Workshop delete operation.
///
/// Deletion needs a live SteamUGC provider; this build records the authority
/// boundary and refuses real deletion.
///
/// # Errors
///
/// Returns receipt errors or a provider diagnostic for real deletion.
pub fn delete_workshop_item(
    installation: &InstallationPaths,
    selector: &str,
    dry_run: bool,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    if !dry_run && !confirmed {
        return Err("real Workshop deletion requires --yes after reviewing --dry-run".to_owned());
    }
    if !dry_run {
        return Err(
            "real Workshop deletion requires the controlled SteamUGC provider; this build can only preview deletion"
                .to_owned(),
        );
    }
    let receipt = write_receipt(
        installation,
        "workshop-delete",
        selector,
        "dry-run",
        Some("would delete or retire the PublishedFileId through SteamUGC"),
    )?;
    Ok(WorkshopOperationReport {
        artifact_id: selector.to_owned(),
        script: None,
        receipt,
        uploaded: false,
        published_file_id: None,
    })
}

fn write_workshop_script(
    paths: &EnvironmentPaths,
    artifact: &ContentArtifact,
    artifact_root: &Path,
    change_note: Option<&str>,
    preview: bool,
) -> Result<PathBuf, String> {
    let layout = ContentLayout::new(paths.installation());
    fs::create_dir_all(layout.output_scripts())
        .map_err(io("create Workshop script output", layout.output_scripts()))?;
    let script = layout
        .output_scripts()
        .join(format!("workshop_{}.vdf", slug(artifact.id())));
    let policy = artifact.workshop();
    let preview_line = if preview {
        "    \"preview\" \"1\"\n"
    } else {
        ""
    };
    let mut tags = String::new();
    tags.push_str("    \"tags\"\n    {\n");
    let mut all_tags = Vec::new();
    for tag in std::iter::once(artifact.kind().to_string()).chain(policy.tags().iter().cloned()) {
        if !all_tags.contains(&tag) {
            all_tags.push(tag);
        }
    }
    for (index, tag) in all_tags.iter().enumerate() {
        tags.push_str(&format!(
            "        \"{}\" \"{}\"\n",
            index,
            steam_escape(tag)
        ));
    }
    tags.push_str("    }\n");
    let vdf = format!(
        "\"workshopitem\"\n{{\n    \"appid\" \"{}\"\n{}    \"publishedfileid\" \"{}\"\n    \"contentfolder\" \"{}\"\n    \"visibility\" \"{}\"\n    \"title\" \"{}\"\n    \"description\" \"{}\"\n    \"changenote\" \"{}\"\n{tags}}}\n",
        policy.app_id().expect("app id checked"),
        preview_line,
        steam_escape(policy.published_file_id().unwrap_or("0")),
        steam_escape(&artifact_root.display().to_string()),
        steam_escape(policy.visibility().unwrap_or("private")),
        steam_escape(policy.title().unwrap_or(artifact.name())),
        steam_escape(policy.description().unwrap_or("")),
        steam_escape(
            change_note
                .or_else(|| policy.change_note())
                .unwrap_or("Vapor content update")
        ),
    );
    fs::write(&script, vdf).map_err(io("write Workshop script", &script))?;
    Ok(script)
}

fn run_steamcmd_workshop_build(
    paths: &EnvironmentPaths,
    account: &str,
    script: &Path,
) -> Result<(), String> {
    run_steamcmd_workshop_builds(paths, account, &[script.to_path_buf()])
}

fn run_steamcmd_workshop_builds(
    paths: &EnvironmentPaths,
    account: &str,
    scripts: &[PathBuf],
) -> Result<(), String> {
    if scripts.is_empty() {
        return Err("at least one Workshop build script is required".to_owned());
    }
    let steamcmd = steam::executable(paths)?;
    let mut command = vapor_core::managed_process::background_command(&steamcmd);
    command.args(["+login", account]);
    for script in scripts {
        command.arg("+workshop_build_item").arg(script);
    }
    let status = command
        .arg("+quit")
        .current_dir(steamcmd.parent().expect("SteamCMD has a parent"))
        .status()
        .map_err(|error| format!("failed to start SteamCMD: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Steam Workshop build exited with {status}"))
    }
}

pub(super) fn run_steamcmd_workshop_download(
    installation: &InstallationPaths,
    app_id: u32,
    published_file_id: &str,
    account: &str,
) -> Result<PathBuf, String> {
    let mut roots = run_steamcmd_workshop_downloads(
        installation,
        &[WorkshopDownload {
            app_id,
            published_file_id: published_file_id.to_owned(),
        }],
        account,
    )?;
    roots
        .pop()
        .ok_or_else(|| "SteamCMD returned no Workshop download root".to_owned())
}

#[derive(Debug, Clone)]
pub(super) struct WorkshopDownload {
    pub(super) app_id: u32,
    pub(super) published_file_id: String,
}

pub(super) fn run_steamcmd_workshop_downloads(
    installation: &InstallationPaths,
    downloads: &[WorkshopDownload],
    account: &str,
) -> Result<Vec<PathBuf>, String> {
    if downloads.is_empty() {
        return Err("at least one Workshop download is required".to_owned());
    }
    let steamcmd = steam::executable_for_installation(installation)?;
    let mut command = vapor_core::managed_process::background_command(&steamcmd);
    command.args(["+login", account]);
    for download in downloads {
        command
            .arg("+workshop_download_item")
            .arg(download.app_id.to_string())
            .arg(&download.published_file_id);
    }
    let status = command
        .arg("+quit")
        .current_dir(steamcmd.parent().expect("SteamCMD has a parent"))
        .status()
        .map_err(|error| format!("failed to start SteamCMD: {error}"))?;
    if !status.success() {
        return Err(format!("Steam Workshop download exited with {status}"));
    }
    downloads
        .iter()
        .map(|download| {
            resolve_steamcmd_workshop_download_root(
                installation,
                &steamcmd,
                download.app_id,
                &download.published_file_id,
            )
        })
        .collect()
}

fn resolve_steamcmd_workshop_download_root(
    installation: &InstallationPaths,
    steamcmd: &Path,
    app_id: u32,
    published_file_id: &str,
) -> Result<PathBuf, String> {
    let app_id = app_id.to_string();
    let mut candidates = Vec::new();
    if let Some(parent) = steamcmd.parent() {
        candidates.push(
            parent
                .join("steamapps/workshop/content")
                .join(&app_id)
                .join(published_file_id),
        );
    }
    if let Some(common_dir) = installation.root().parent()
        && common_dir
            .file_name()
            .is_some_and(|name| name.to_string_lossy() == "common")
        && let Some(steamapps_dir) = common_dir.parent()
    {
        candidates.push(
            steamapps_dir
                .join("workshop/content")
                .join(&app_id)
                .join(published_file_id),
        );
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(
            home.join(".local/share/Steam/steamapps/workshop/content")
                .join(&app_id)
                .join(published_file_id),
        );
        candidates.push(
            home.join(".steam/steam/steamapps/workshop/content")
                .join(&app_id)
                .join(published_file_id),
        );
    }
    for download_root in &candidates {
        if download_root.exists() {
            return Ok(download_root.clone());
        }
    }
    let attempted = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "SteamCMD reported success but no Workshop content folder exists; checked: {attempted}"
    ))
}

fn read_workshop_script_published_file_id(script: &Path) -> Result<String, String> {
    let source = fs::read_to_string(script).map_err(io("read Workshop script", script))?;
    vdf_value(&source, "publishedfileid")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "Workshop script has no publishedfileid: {}",
                script.display()
            )
        })
}

fn record_published_file_id(
    artifact: &ContentArtifact,
    published_file_id: &str,
) -> Result<(), String> {
    let source = fs::read_to_string(artifact.manifest())
        .map_err(io("read content manifest", artifact.manifest()))?;
    let updated = set_published_file_id(&source, artifact.kind(), published_file_id)?;
    AuthoredContentManifest::parse(&updated).map_err(|error| {
        format!(
            "refusing to write invalid content manifest '{}': {error}",
            artifact.manifest().display()
        )
    })?;
    fs::write(artifact.manifest(), updated)
        .map_err(io("write content manifest", artifact.manifest()))
}

fn set_published_file_id(
    source: &str,
    kind: ContentKind,
    published_file_id: &str,
) -> Result<String, String> {
    let section = format!("[{}.steam]", kind);
    let mut lines = source.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let Some(start) = lines.iter().position(|line| line.trim() == section) else {
        return Err(format!("content manifest has no {section} section"));
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, line)| line.trim_start().starts_with('[').then_some(index))
        .unwrap_or(lines.len());
    let field = format!("published-file-id = \"{}\"", published_file_id);
    if let Some(index) = lines[start + 1..end].iter().position(|line| {
        line.trim_start()
            .strip_prefix("published-file-id")
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    }) {
        lines[start + 1 + index] = field;
    } else {
        let insert_at = lines[start + 1..end]
            .iter()
            .position(|line| {
                line.trim_start()
                    .strip_prefix("app-id")
                    .is_some_and(|rest| rest.trim_start().starts_with('='))
            })
            .map_or(start + 1, |index| start + 2 + index);
        lines.insert(insert_at, field);
    }
    let mut updated = lines.join("\n");
    if source.ends_with('\n') {
        updated.push('\n');
    }
    Ok(updated)
}

fn vdf_value(source: &str, key: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let parts = line.trim().split('"').collect::<Vec<_>>();
        (parts.len() >= 4 && parts[1] == key).then(|| parts[3].to_owned())
    })
}

pub(super) fn is_published_file_id(selector: &str) -> bool {
    !selector.is_empty() && selector.bytes().all(|byte| byte.is_ascii_digit())
}

pub(super) fn resolve_download_app_id(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
) -> Result<u32, String> {
    if let Some(paths) = paths {
        let catalog = discover(paths)?;
        if let Some(artifact) = catalog.find(selector)
            && let Some(app_id) = artifact.workshop().app_id()
        {
            return Ok(app_id);
        }
    }
    root_steam_app_id(installation)
}

/// Resolve the Steam application that owns a Workshop item in an install plan.
///
/// First-party seeds carry their own application ID. Other items use the
/// discovered source metadata or the app's Workshop application.
pub fn workshop_app_id(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    artifact_id: &str,
    published_file_id: &str,
) -> Result<u32, String> {
    if let Some(seed) = root_content_seed(installation, artifact_id)?
        .or(root_content_seed(installation, published_file_id)?)
    {
        return Ok(seed.app_id);
    }
    resolve_download_app_id(installation, paths, published_file_id)
}

pub(super) fn resolve_live_workshop_download(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    index: &ContentIndex,
    selector: &str,
) -> Result<Option<WorkshopDownload>, String> {
    if is_published_file_id(selector) {
        return Ok(Some(WorkshopDownload {
            app_id: resolve_download_app_id(installation, paths, selector)?,
            published_file_id: selector.to_owned(),
        }));
    }

    if let Some(paths) = paths {
        let catalog = discover(paths)?;
        if let Some(artifact) = catalog.find(selector)
            && let (Some(app_id), Some(published_file_id)) = (
                artifact.workshop().app_id(),
                artifact.workshop().published_file_id(),
            )
        {
            return Ok(Some(WorkshopDownload {
                app_id,
                published_file_id: published_file_id.to_owned(),
            }));
        }
    }

    if let Some(seed) = root_content_seed(installation, selector)? {
        return Ok(Some(WorkshopDownload {
            app_id: seed.app_id,
            published_file_id: seed.workshop_id,
        }));
    }

    if let Some((_artifact_id, cache)) = index.cache_by_selector(selector)
        && let Some(published_file_id) = &cache.workshop_id
    {
        let artifact = read_deployed_manifest(&cache.cache_root)?;
        let app_id = artifact
            .workshop()
            .app_id()
            .map_or_else(|| root_steam_app_id(installation), Ok)?;
        return Ok(Some(WorkshopDownload {
            app_id,
            published_file_id: published_file_id.clone(),
        }));
    }

    Ok(None)
}
