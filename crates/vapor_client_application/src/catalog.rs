use std::path::Path;
pub use vapor_client_domain::{
    CatalogConflict, CatalogContent, CatalogLocation, CatalogLocationKind,
};
use vapor_client_provider::{
    content,
    discovery::{EnvironmentPaths, InstallationPaths},
};

/// Merge workspace-authored, cached, and installed content by exact identity.
/// A missing or invalid workspace does not prevent cache and install discovery
/// when the caller passes no workspace root.
pub fn discover_content_catalog(
    application_executable: &Path,
    workspace_root: Option<&Path>,
) -> Result<Vec<CatalogContent>, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let mut entries = Vec::new();

    if let Some(workspace_root) = workspace_root {
        // Workspace source errors are surfaced through Studio's authored-source
        // discovery result. They must not hide a usable cache/install catalog.
        if let Ok(paths) = EnvironmentPaths::from_installation_and_source_path(
            installation.clone(),
            workspace_root,
        ) && let Ok(source_catalog) = content::discover(&paths)
        {
            for artifact in source_catalog.artifacts() {
                entries.push(CatalogContent {
                    id: artifact.id().to_owned(),
                    name: Some(artifact.name().to_owned()),
                    version: artifact.version().map(ToOwned::to_owned),
                    kind: artifact.kind().to_string(),
                    published_file_id: artifact
                        .workshop()
                        .published_file_id()
                        .map(ToOwned::to_owned),
                    dependencies: artifact
                        .dependencies()
                        .iter()
                        .map(dependency_description)
                        .collect(),
                    locations: vec![CatalogLocation {
                        kind: CatalogLocationKind::Workspace,
                        root: artifact.root().to_path_buf(),
                        enabled: true,
                        fingerprint: None,
                        install_source: None,
                    }],
                    conflicts: Vec::new(),
                });
            }
        }
    }

    for selector in content::cached_index(&installation)? {
        let artifact = content::cached_artifact(&installation, &selector)?;
        entries.push(CatalogContent {
            id: artifact.artifact_id().to_owned(),
            name: artifact.name().map(ToOwned::to_owned),
            version: artifact.version().map(ToOwned::to_owned),
            kind: artifact.kind().to_owned(),
            published_file_id: artifact.workshop_id().map(ToOwned::to_owned),
            dependencies: artifact
                .dependencies()
                .iter()
                .map(dependency_description)
                .collect(),
            locations: vec![CatalogLocation {
                kind: CatalogLocationKind::Cache,
                root: artifact.installed_root().to_path_buf(),
                enabled: true,
                fingerprint: Some(format!(
                    "{}:{}",
                    artifact.fingerprint().algorithm(),
                    artifact.fingerprint().hash()
                )),
                install_source: None,
            }],
            conflicts: Vec::new(),
        });
    }

    for selector in content::installed_index(&installation)? {
        let artifacts = content::installed_artifacts(&installation, &[selector])?;
        for artifact in artifacts {
            entries.push(CatalogContent {
                id: artifact.artifact_id().to_owned(),
                name: artifact.name().map(ToOwned::to_owned),
                version: artifact.version().map(ToOwned::to_owned),
                kind: artifact.kind().to_owned(),
                published_file_id: artifact.workshop_id().map(ToOwned::to_owned),
                dependencies: artifact
                    .dependencies()
                    .iter()
                    .map(dependency_description)
                    .collect(),
                locations: vec![CatalogLocation {
                    kind: CatalogLocationKind::Installed,
                    root: artifact.installed_root().to_path_buf(),
                    enabled: artifact.enabled(),
                    fingerprint: Some(format!(
                        "{}:{}",
                        artifact.fingerprint().algorithm(),
                        artifact.fingerprint().hash()
                    )),
                    install_source: Some(artifact.source().to_owned()),
                }],
                conflicts: Vec::new(),
            });
        }
    }

    Ok(vapor_client_domain::merge_catalog_entries(entries))
}

fn dependency_description(dependency: &content::ContentReference) -> String {
    let version = dependency
        .version()
        .map(|version| format!(" @ {version}"))
        .unwrap_or_default();
    let optional = if dependency.optional() {
        " · optional"
    } else {
        ""
    };
    let workshop = dependency
        .workshop_id()
        .map(|id| format!(" · Workshop {id}"))
        .unwrap_or_default();
    let reason = dependency
        .reason()
        .map(|reason| format!(" · {reason}"))
        .unwrap_or_default();
    format!(
        "{} · {}{}{}{}{}",
        dependency.relationship(),
        dependency.id(),
        version,
        optional,
        workshop,
        reason
    )
}
