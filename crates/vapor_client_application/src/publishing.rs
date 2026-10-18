use std::path::{Path, PathBuf};
use vapor_client_provider::{
    content,
    discovery::{EnvironmentPaths, InstallationPaths},
};

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

/// Stage source content and expose the exact artifact facts a frontend can review.
/// This operation never uploads to Workshop.
pub fn stage_workshop_preview(
    application_executable: &Path,
    workspace_root: &Path,
    selector: &str,
) -> Result<WorkshopPackagePreview, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    let catalog = content::discover(&paths)?;
    let artifact = catalog
        .find(selector)
        .ok_or_else(|| format!("unknown content artifact '{selector}'"))?;
    let package = content::package(&paths, selector, false)?;
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
    account: &str,
    change_note: Option<&str>,
) -> Result<WorkshopPublication, String> {
    let current = stage_workshop_preview(application_executable, workspace_root, selector)?;
    if &current != expected_review {
        return Err(format!(
            "{} changed after review; stage a fresh Workshop review before uploading",
            current.id
        ));
    }
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    let report = if current.published_file_id.is_some() {
        content::publish_workshop_item_reviewed(
            &paths,
            selector,
            &current.fingerprint,
            account,
            change_note,
            true,
        )?
    } else {
        content::create_workshop_item_reviewed(
            &paths,
            selector,
            &current.fingerprint,
            account,
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
