pub use vapor_client_domain::{
    CatalogConflict, CatalogContent, CatalogDependency, CatalogLocation, CatalogLocationKind,
};
use vapor_client_provider::{content, discovery::InstallationPaths};

/// Discover acquired content that is usable by player and composition flows.
/// Authored source projects have their own workspace discovery surface and do
/// not enter the local Library catalog.
pub fn discover_local_content_catalog(
    application_executable: &std::path::Path,
) -> Result<Vec<CatalogContent>, String> {
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let mut entries = Vec::new();

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
                .map(catalog_dependency)
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
                    .map(catalog_dependency)
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

fn catalog_dependency(dependency: &content::ContentReference) -> CatalogDependency {
    CatalogDependency {
        id: dependency.id().to_owned(),
        version: dependency.version().map(ToOwned::to_owned),
        relationship: dependency.relationship().to_owned(),
        optional: dependency.optional(),
        workshop_id: dependency.workshop_id().map(ToOwned::to_owned),
        reason: dependency.reason().map(ToOwned::to_owned),
    }
}
