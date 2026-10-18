//! Resolution of authored Vapor dependency declarations into exact Content.
//!
//! This layer adapts the local catalog to the provider-neutral resolver.
//! Content and pack validation remain owned by `vapor_content`.
//!
//! Vertical Slice 0 currently resolves only against a `LocalCatalog`.
//! Full Cargo-style global version unification/backtracking remains deferred.

use crate::{ContentKind, ContentVersionId, LocalCatalog, VaporId};
use vapor_content::{ContentCandidate, ContentCandidates, resolve_exact, resolve_latest};
pub use vapor_content::{
    ResolutionError, ResolvedComposition, ResolvedContentGraph, ResolvedContentNode,
};

/// Resolve the latest locally available version of arbitrary Vapor Content.
///
/// This performs structural dependency resolution only:
///
/// - version selection,
/// - recursive traversal,
/// - exact-node convergence,
/// - missing-dependency detection,
/// - cycle detection.
///
/// Semantic validation is deliberately layered separately.
pub fn resolve_local_content(
    catalog: &LocalCatalog,
    content_id: &VaporId,
) -> Result<ResolvedContentGraph, ResolutionError> {
    resolve_latest(&content_candidates(catalog), content_id)
}

/// Resolve arbitrary Vapor Content from one exact root version.
/// Dependencies still obey their declared version requirements and are
/// selected by the local resolver. This keeps a composition's root stable while
/// retaining the existing dependency selection policy.
pub fn resolve_local_content_exact(
    catalog: &LocalCatalog,
    identity: &ContentVersionId,
) -> Result<ResolvedContentGraph, ResolutionError> {
    resolve_exact(&content_candidates(catalog), identity)
}

fn content_candidates(catalog: &LocalCatalog) -> ContentCandidates {
    let mut candidates = ContentCandidates::new();
    for content in catalog.iter() {
        let candidate = ContentCandidate {
            identity: content.version_id(),
            kind: content.manifest.content.kind,
            dependencies: content.manifest.dependencies.clone(),
        };
        candidates
            .insert(candidate)
            .expect("local catalog stores unique exact identities");
    }
    candidates
}

/// Resolve arbitrary Vapor Content while requiring one expected root kind.
///
/// Resolution remains generic. This adapter only adds:
///
/// - root-kind validation,
/// - Content-kind relationship validation,
/// - applicable pack validation.
///
/// Pack validation is a no-op for non-pack roots such as Libraries.
pub fn resolve_local_content_kind(
    catalog: &LocalCatalog,
    content_id: &VaporId,
    expected_kind: ContentKind,
) -> Result<ResolvedContentGraph, ResolutionError> {
    let graph = resolve_local_content(catalog, content_id)?;

    let actual_kind = graph.root_node().kind;

    if actual_kind != expected_kind {
        return Err(ResolutionError::RootKindMismatch {
            identity: graph.root.clone(),
            expected_kind,
            actual_kind,
        });
    }

    vapor_content::validate_content_graph(&graph)?;

    Ok(graph)
}

/// Resolve and validate one pack kind.
///
/// Retained as the pack-oriented semantic entry point while all kinds share
/// the same underlying resolver.
pub fn resolve_local_pack(
    catalog: &LocalCatalog,
    content_id: &VaporId,
    expected_kind: ContentKind,
) -> Result<ResolvedContentGraph, ResolutionError> {
    resolve_local_content_kind(catalog, content_id, expected_kind)
}

/// Resolve a Packagepack into a complete Vapor App Composition.
///
/// This remains as the Packagepack-specific layer consumed by the current
/// Cargo realization implementation.
pub fn resolve_local_packagepack(
    catalog: &LocalCatalog,
    packagepack_id: &VaporId,
) -> Result<ResolvedComposition, ResolutionError> {
    let graph = resolve_local_content(catalog, packagepack_id)?;
    resolve_packagepack_graph(graph)
}

/// Resolve and validate an exact Packagepack version as a launch composition.
pub fn resolve_local_packagepack_exact(
    catalog: &LocalCatalog,
    identity: &ContentVersionId,
) -> Result<ResolvedComposition, ResolutionError> {
    let graph = resolve_local_content_exact(catalog, identity)?;
    resolve_packagepack_graph(graph)
}

fn resolve_packagepack_graph(
    graph: ResolvedContentGraph,
) -> Result<ResolvedComposition, ResolutionError> {
    vapor_content::resolve_composition(graph)
}

/// Validate semantic relationships that belong to Content kinds themselves,
/// independently of which pack happens to include them.
pub fn validate_resolved_content_graph(
    graph: &ResolvedContentGraph,
) -> Result<(), ResolutionError> {
    vapor_content::validate_content_relationships(graph)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CONTENT_MANIFEST_FILE_NAME, discover_local_content};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Self {
            let id = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);

            let path = std::env::temp_dir().join(format!(
                "vapor-core-resolution-test-{}-{id}",
                std::process::id()
            ));

            fs::create_dir_all(&path).unwrap();

            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn write_manifest(&self, relative_directory: &str, manifest: &str) {
            let directory = self.path.join(relative_directory);
            fs::create_dir_all(&directory).unwrap();

            fs::write(directory.join(CONTENT_MANIFEST_FILE_NAME), manifest).unwrap();
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_valid_example(directory: &TestDirectory) {
        directory.write_manifest(
            "engine-0-1-0",
            r#"
[content]
id = "ghf-studios/example/engine"
version = "0.1.0"
kind = "engine"
"#,
        );

        directory.write_manifest(
            "engine-0-1-5",
            r#"
[content]
id = "ghf-studios/example/engine"
version = "0.1.5"
kind = "engine"
"#,
        );

        directory.write_manifest(
            "game",
            r#"
[content]
id = "ghf-studios/example/game"
version = "0.1.0"
kind = "game"

[dependencies.engine]
id = "ghf-studios/example/engine"
version = "^0.1"
"#,
        );

        directory.write_manifest(
            "mod",
            r#"
[content]
id = "ghf-studios/example/mod"
version = "0.1.0"
kind = "game-mod"

[dependencies.game]
id = "ghf-studios/example/game"
version = "^0.1"
"#,
        );

        directory.write_manifest(
            "packagepack",
            r#"
[content]
id = "ghf-studios/example/packagepack"
version = "0.1.0"
kind = "packagepack"

[dependencies.engine]
id = "ghf-studios/example/engine"
version = "^0.1"

[dependencies.game]
id = "ghf-studios/example/game"
version = "^0.1"

[dependencies.mod]
id = "ghf-studios/example/mod"
version = "^0.1"
"#,
        );
    }

    #[test]
    fn resolves_complete_packagepack_graph() {
        let directory = TestDirectory::new();
        write_valid_example(&directory);

        let catalog = discover_local_content(directory.path()).unwrap();

        let packagepack_id: VaporId = "ghf-studios/example/packagepack".parse().unwrap();

        let composition = resolve_local_packagepack(&catalog, &packagepack_id).unwrap();

        assert_eq!(
            composition.root.to_string(),
            "ghf-studios/example/packagepack@0.1.0"
        );

        assert_eq!(
            composition.effective_engine.to_string(),
            "ghf-studios/example/engine@0.1.5"
        );

        assert_eq!(
            composition.effective_game.to_string(),
            "ghf-studios/example/game@0.1.0"
        );

        assert_eq!(composition.nodes.len(), 4);
    }

    #[test]
    fn converging_edges_share_same_exact_definition() {
        let directory = TestDirectory::new();
        write_valid_example(&directory);

        let catalog = discover_local_content(directory.path()).unwrap();

        let packagepack_id: VaporId = "ghf-studios/example/packagepack".parse().unwrap();

        let composition = resolve_local_packagepack(&catalog, &packagepack_id).unwrap();

        let root = composition.root_node();

        let root_engine = root.dependencies.get("engine").unwrap();

        let game_id = root.dependencies.get("game").unwrap();
        let game = composition.node(game_id).unwrap();

        let game_engine = game.dependencies.get("engine").unwrap();

        assert_eq!(root_engine, game_engine);
    }

    #[test]
    fn rejects_missing_dependency() {
        let directory = TestDirectory::new();

        directory.write_manifest(
            "packagepack",
            r#"
[content]
id = "ghf-studios/example/packagepack"
version = "0.1.0"
kind = "packagepack"

[dependencies.missing]
id = "ghf-studios/example/missing"
version = "^1"
"#,
        );

        let catalog = discover_local_content(directory.path()).unwrap();

        let packagepack_id: VaporId = "ghf-studios/example/packagepack".parse().unwrap();

        let error = resolve_local_packagepack(&catalog, &packagepack_id).unwrap_err();

        assert!(matches!(error, ResolutionError::DependencyNotFound { .. }));
    }

    #[test]
    fn rejects_dependency_cycle() {
        let directory = TestDirectory::new();

        directory.write_manifest(
            "packagepack",
            r#"
[content]
id = "ghf-studios/example/packagepack"
version = "0.1.0"
kind = "packagepack"

[dependencies.game]
id = "ghf-studios/example/game"
version = "^0.1"
"#,
        );

        directory.write_manifest(
            "game",
            r#"
[content]
id = "ghf-studios/example/game"
version = "0.1.0"
kind = "game"

[dependencies.packagepack]
id = "ghf-studios/example/packagepack"
version = "^0.1"
"#,
        );

        let catalog = discover_local_content(directory.path()).unwrap();

        let packagepack_id: VaporId = "ghf-studios/example/packagepack".parse().unwrap();

        let error = resolve_local_packagepack(&catalog, &packagepack_id).unwrap_err();

        assert!(matches!(error, ResolutionError::DependencyCycle { .. }));
    }
}
