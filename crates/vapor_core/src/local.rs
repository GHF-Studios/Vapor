//! Discovery and cataloguing of locally available Vapor Content.

use crate::{ContentManifest, ContentVersionId, ManifestError, VaporId, parse_content_manifest};
use semver::VersionReq;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Provisional Vertical Slice 0 filename for a Vapor Content manifest.
///
/// The filename is intentionally generic: artifact kind is expressed by the
/// manifest rather than encoded into the filename.
pub const CONTENT_MANIFEST_FILE_NAME: &str = "Vapor.toml";

/// One locally discovered Vapor Content artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalContent {
    /// Root directory of the Vapor Content project.
    pub root: PathBuf,

    /// Path to the project's `Vapor.toml`.
    pub manifest_path: PathBuf,

    /// Parsed human-authored manifest.
    pub manifest: ContentManifest,
}

impl LocalContent {
    pub fn version_id(&self) -> ContentVersionId {
        self.manifest.content.version_id()
    }
}

/// Locally available exact Vapor Content versions.
///
/// This is deliberately only a local source catalog. It is not the Vapor
/// Registry and it does not perform dependency resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalCatalog {
    contents: BTreeMap<ContentVersionId, LocalContent>,
}

impl LocalCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.contents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.contents.is_empty()
    }

    pub fn get(&self, identity: &ContentVersionId) -> Option<&LocalContent> {
        self.contents.get(identity)
    }

    pub fn iter(&self) -> impl Iterator<Item = &LocalContent> {
        self.contents.values()
    }

    /// Iterate over every locally available version of one Vapor ID.
    pub fn versions<'a>(&'a self, id: &'a VaporId) -> impl Iterator<Item = &'a LocalContent> + 'a {
        self.contents
            .iter()
            .filter(move |(identity, _)| &identity.id == id)
            .map(|(_, content)| content)
    }

    /// Return the highest locally available version of one Vapor ID.
    pub fn latest<'a>(&'a self, id: &'a VaporId) -> Option<&'a LocalContent> {
        self.versions(id).max_by(|left, right| {
            left.manifest
                .content
                .version
                .cmp(&right.manifest.content.version)
        })
    }

    /// Return the highest locally available version satisfying a SemVer
    /// requirement.
    pub fn latest_matching<'a>(
        &'a self,
        id: &'a VaporId,
        requirement: &'a VersionReq,
    ) -> Option<&'a LocalContent> {
        self.versions(id)
            .filter(|content| requirement.matches(&content.manifest.content.version))
            .max_by(|left, right| {
                left.manifest
                    .content
                    .version
                    .cmp(&right.manifest.content.version)
            })
    }

    fn insert(&mut self, content: LocalContent) -> Result<(), LocalDiscoveryError> {
        let identity = content.version_id();

        if let Some(existing) = self.contents.get(&identity) {
            return Err(LocalDiscoveryError::DuplicateContentVersion {
                identity,
                first_manifest: existing.manifest_path.clone(),
                second_manifest: content.manifest_path,
            });
        }

        self.contents.insert(identity, content);

        Ok(())
    }
}

/// Recursively discover local Vapor Content below one source root.
///
/// Discovery currently recognizes `Vapor.toml` files and deliberately ignores
/// generated or repository-internal directories.
pub fn discover_local_content(root: impl AsRef<Path>) -> Result<LocalCatalog, LocalDiscoveryError> {
    let root = root.as_ref();

    let mut catalog = LocalCatalog::new();
    discover_directory(root, &mut catalog)?;

    Ok(catalog)
}

fn discover_directory(
    directory: &Path,
    catalog: &mut LocalCatalog,
) -> Result<(), LocalDiscoveryError> {
    let entries = fs::read_dir(directory).map_err(|source| LocalDiscoveryError::Io {
        path: directory.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| LocalDiscoveryError::Io {
            path: directory.to_path_buf(),
            source,
        })?;

        let path = entry.path();

        let file_type = entry
            .file_type()
            .map_err(|source| LocalDiscoveryError::Io {
                path: path.clone(),
                source,
            })?;

        if file_type.is_dir() {
            if should_skip_directory(&entry.file_name()) {
                continue;
            }

            discover_directory(&path, catalog)?;
            continue;
        }

        if file_type.is_file() && entry.file_name() == OsStr::new(CONTENT_MANIFEST_FILE_NAME) {
            let content = load_local_content(&path)?;
            catalog.insert(content)?;
        }
    }

    Ok(())
}

fn should_skip_directory(name: &OsStr) -> bool {
    matches!(name.to_str(), Some(".git" | ".vapor" | "target"))
}

fn load_local_content(manifest_path: &Path) -> Result<LocalContent, LocalDiscoveryError> {
    let source = fs::read_to_string(manifest_path).map_err(|source| LocalDiscoveryError::Io {
        path: manifest_path.to_path_buf(),
        source,
    })?;

    let manifest =
        parse_content_manifest(&source).map_err(|source| LocalDiscoveryError::Manifest {
            path: manifest_path.to_path_buf(),
            source,
        })?;

    let root = manifest_path
        .parent()
        .unwrap_or(Path::new(""))
        .to_path_buf();

    Ok(LocalContent {
        root,
        manifest_path: manifest_path.to_path_buf(),
        manifest,
    })
}

#[derive(Debug)]
pub enum LocalDiscoveryError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Manifest {
        path: PathBuf,
        source: ManifestError,
    },
    DuplicateContentVersion {
        identity: ContentVersionId,
        first_manifest: PathBuf,
        second_manifest: PathBuf,
    },
}

impl fmt::Display for LocalDiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(
                    formatter,
                    "failed to access local Vapor Content at `{}`: {}",
                    path.display(),
                    source
                )
            }
            Self::Manifest { path, source } => {
                write!(
                    formatter,
                    "failed to load Vapor Content manifest `{}`: {}",
                    path.display(),
                    source
                )
            }
            Self::DuplicateContentVersion {
                identity,
                first_manifest,
                second_manifest,
            } => {
                write!(
                    formatter,
                    "duplicate local Vapor Content version `{}` declared by `{}` and `{}`",
                    identity,
                    first_manifest.display(),
                    second_manifest.display()
                )
            }
        }
    }
}

impl std::error::Error for LocalDiscoveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Manifest { source, .. } => Some(source),
            Self::DuplicateContentVersion { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Self {
            let id = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);

            let path = std::env::temp_dir()
                .join(format!("vapor-core-local-test-{}-{id}", std::process::id()));

            fs::create_dir_all(&path).unwrap();

            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn write_manifest(&self, relative_directory: &str, manifest: &str) -> PathBuf {
            let directory = self.path.join(relative_directory);

            fs::create_dir_all(&directory).unwrap();

            let path = directory.join(CONTENT_MANIFEST_FILE_NAME);
            fs::write(&path, manifest).unwrap();

            path
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn discovers_vapor_content_recursively() {
        let directory = TestDirectory::new();

        directory.write_manifest(
            "engine",
            r#"
[content]
id = "ghf-studios/example/engine"
version = "1.0.0"
kind = "engine"
"#,
        );

        directory.write_manifest(
            "nested/game",
            r#"
[content]
id = "ghf-studios/example/game"
version = "1.0.0"
kind = "game"

[dependencies.engine]
id = "ghf-studios/example/engine"
version = "^1"
"#,
        );

        let catalog = discover_local_content(directory.path()).unwrap();

        assert_eq!(catalog.len(), 2);

        let engine_id: VaporId = "ghf-studios/example/engine".parse().unwrap();

        let engine_versions: Vec<_> = catalog.versions(&engine_id).collect();

        assert_eq!(engine_versions.len(), 1);
        assert_eq!(
            engine_versions[0].manifest.content.kind,
            crate::ContentKind::Engine
        );
    }

    #[test]
    fn returns_highest_matching_version() {
        let directory = TestDirectory::new();

        directory.write_manifest(
            "engine-1-0",
            r#"
[content]
id = "ghf-studios/example/engine"
version = "1.0.0"
kind = "engine"
"#,
        );

        directory.write_manifest(
            "engine-1-4",
            r#"
[content]
id = "ghf-studios/example/engine"
version = "1.4.0"
kind = "engine"
"#,
        );

        directory.write_manifest(
            "engine-2",
            r#"
[content]
id = "ghf-studios/example/engine"
version = "2.0.0"
kind = "engine"
"#,
        );

        let catalog = discover_local_content(directory.path()).unwrap();

        let id: VaporId = "ghf-studios/example/engine".parse().unwrap();

        let requirement = VersionReq::parse("^1").unwrap();

        let selected = catalog.latest_matching(&id, &requirement).unwrap();

        assert_eq!(selected.manifest.content.version.to_string(), "1.4.0");
    }

    #[test]
    fn ignores_generated_and_repository_internal_directories() {
        let directory = TestDirectory::new();

        directory.write_manifest(
            "real",
            r#"
[content]
id = "ghf-studios/example/real"
version = "1.0.0"
kind = "game"
"#,
        );

        directory.write_manifest(
            "target/ignored",
            r#"
[content]
id = "ghf-studios/example/ignored-target"
version = "1.0.0"
kind = "game"
"#,
        );

        directory.write_manifest(
            ".vapor/ignored",
            r#"
[content]
id = "ghf-studios/example/ignored-vapor"
version = "1.0.0"
kind = "game"
"#,
        );

        directory.write_manifest(
            ".git/ignored",
            r#"
[content]
id = "ghf-studios/example/ignored-git"
version = "1.0.0"
kind = "game"
"#,
        );

        let catalog = discover_local_content(directory.path()).unwrap();

        assert_eq!(catalog.len(), 1);
    }

    #[test]
    fn rejects_duplicate_exact_content_versions() {
        let directory = TestDirectory::new();

        let manifest = r#"
[content]
id = "ghf-studios/example/duplicate"
version = "1.2.3"
kind = "game"
"#;

        directory.write_manifest("first", manifest);
        directory.write_manifest("second", manifest);

        let error = discover_local_content(directory.path()).unwrap_err();

        assert!(matches!(
            error,
            LocalDiscoveryError::DuplicateContentVersion { .. }
        ));
    }
}
