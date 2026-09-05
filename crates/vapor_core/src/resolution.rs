//! Resolution of authored Vapor dependency declarations into exact content.
//!
//! Vertical Slice 0 currently resolves only against a `LocalCatalog`.
//!
//! The initial algorithm chooses the highest locally available version matching
//! each dependency requirement. Exact versions already chosen by multiple edges
//! naturally share one resolved definition node.
//!
//! Full Cargo-style global version unification/backtracking is intentionally
//! deferred. The public resolved graph model already permits multiple versions
//! of the same Vapor ID.

use crate::{ContentKind, ContentVersionId, LocalCatalog, VaporId};
use semver::VersionReq;
use std::collections::BTreeMap;
use std::fmt;

/// One exact resolved Vapor Content definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedContentNode {
    pub identity: ContentVersionId,
    pub kind: ContentKind,

    /// Local dependency binding -> exact resolved content identity.
    pub dependencies: BTreeMap<String, ContentVersionId>,
}

/// Exact, structurally validated dependency graph produced from a Packagepack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedComposition {
    pub root: ContentVersionId,
    pub nodes: BTreeMap<ContentVersionId, ResolvedContentNode>,
    pub effective_engine: ContentVersionId,
    pub effective_game: ContentVersionId,
}

impl ResolvedComposition {
    pub fn node(&self, identity: &ContentVersionId) -> Option<&ResolvedContentNode> {
        self.nodes.get(identity)
    }

    pub fn root_node(&self) -> &ResolvedContentNode {
        self.nodes
            .get(&self.root)
            .expect("resolved composition root must exist")
    }
}

/// Resolve the latest locally available version of a Packagepack.
///
/// The Packagepack's complete reachable dependency graph is resolved
/// recursively against the supplied local catalog.
pub fn resolve_local_packagepack(
    catalog: &LocalCatalog,
    packagepack_id: &VaporId,
) -> Result<ResolvedComposition, ResolutionError> {
    let root_content =
        catalog
            .latest(packagepack_id)
            .ok_or_else(|| ResolutionError::RootNotFound {
                id: packagepack_id.clone(),
            })?;

    let root = root_content.version_id();

    if root_content.manifest.content.kind != ContentKind::Packagepack {
        return Err(ResolutionError::RootNotPackagepack {
            identity: root,
            actual_kind: root_content.manifest.content.kind,
        });
    }

    let mut resolver = LocalResolver {
        catalog,
        nodes: BTreeMap::new(),
        stack: Vec::new(),
    };

    resolver.resolve_identity(root.clone())?;

    validate_composition(root, resolver.nodes)
}

struct LocalResolver<'a> {
    catalog: &'a LocalCatalog,
    nodes: BTreeMap<ContentVersionId, ResolvedContentNode>,
    stack: Vec<ContentVersionId>,
}

impl LocalResolver<'_> {
    fn resolve_identity(&mut self, identity: ContentVersionId) -> Result<(), ResolutionError> {
        if self.nodes.contains_key(&identity) {
            return Ok(());
        }

        if let Some(start) = self.stack.iter().position(|active| active == &identity) {
            let mut cycle = self.stack[start..].to_vec();
            cycle.push(identity);

            return Err(ResolutionError::DependencyCycle { cycle });
        }

        let content = self
            .catalog
            .get(&identity)
            .expect("resolver identity must originate from local catalog");

        let kind = content.manifest.content.kind;

        let authored_dependencies: Vec<_> = content
            .manifest
            .dependencies
            .iter()
            .map(|(binding, dependency)| (binding.clone(), dependency.clone()))
            .collect();

        self.stack.push(identity.clone());

        let mut resolved_dependencies = BTreeMap::new();

        for (binding, dependency) in authored_dependencies {
            let selected = self
                .catalog
                .latest_matching(&dependency.id, &dependency.version)
                .ok_or_else(|| ResolutionError::DependencyNotFound {
                    depender: identity.clone(),
                    binding: binding.clone(),
                    id: dependency.id.clone(),
                    requirement: dependency.version.clone(),
                })?
                .version_id();

            self.resolve_identity(selected.clone())?;

            resolved_dependencies.insert(binding, selected);
        }

        let popped = self.stack.pop();

        debug_assert_eq!(popped.as_ref(), Some(&identity));

        self.nodes.insert(
            identity.clone(),
            ResolvedContentNode {
                identity,
                kind,
                dependencies: resolved_dependencies,
            },
        );

        Ok(())
    }
}

fn validate_composition(
    root: ContentVersionId,
    nodes: BTreeMap<ContentVersionId, ResolvedContentNode>,
) -> Result<ResolvedComposition, ResolutionError> {
    validate_target_relationships(&nodes)?;

    let engines: Vec<_> = nodes
        .values()
        .filter(|node| node.kind == ContentKind::Engine)
        .map(|node| node.identity.clone())
        .collect();

    let games: Vec<_> = nodes
        .values()
        .filter(|node| node.kind == ContentKind::Game)
        .map(|node| node.identity.clone())
        .collect();

    if engines.len() != 1 {
        return Err(ResolutionError::EffectiveEngineCount {
            count: engines.len(),
            engines,
        });
    }

    if games.len() != 1 {
        return Err(ResolutionError::EffectiveGameCount {
            count: games.len(),
            games,
        });
    }

    Ok(ResolvedComposition {
        root,
        nodes,
        effective_engine: engines
            .into_iter()
            .next()
            .expect("validated exactly one effective Engine"),
        effective_game: games
            .into_iter()
            .next()
            .expect("validated exactly one effective Game"),
    })
}

fn validate_target_relationships(
    nodes: &BTreeMap<ContentVersionId, ResolvedContentNode>,
) -> Result<(), ResolutionError> {
    for node in nodes.values() {
        match node.kind {
            ContentKind::Game => {
                let engine_targets =
                    direct_dependency_count_of_kind(nodes, node, ContentKind::Engine);

                if engine_targets != 1 {
                    return Err(ResolutionError::GameEngineTargetCount {
                        game: node.identity.clone(),
                        count: engine_targets,
                    });
                }
            }

            ContentKind::GameMod => {
                let game_targets = direct_dependency_count_of_kind(nodes, node, ContentKind::Game);

                if game_targets != 1 {
                    return Err(ResolutionError::GameModGameTargetCount {
                        game_mod: node.identity.clone(),
                        count: game_targets,
                    });
                }
            }

            _ => {}
        }
    }

    Ok(())
}

fn direct_dependency_count_of_kind(
    nodes: &BTreeMap<ContentVersionId, ResolvedContentNode>,
    node: &ResolvedContentNode,
    kind: ContentKind,
) -> usize {
    node.dependencies
        .values()
        .filter(|dependency| {
            nodes
                .get(*dependency)
                .is_some_and(|dependency_node| dependency_node.kind == kind)
        })
        .count()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionError {
    RootNotFound {
        id: VaporId,
    },

    RootNotPackagepack {
        identity: ContentVersionId,
        actual_kind: ContentKind,
    },

    DependencyNotFound {
        depender: ContentVersionId,
        binding: String,
        id: VaporId,
        requirement: VersionReq,
    },

    DependencyCycle {
        cycle: Vec<ContentVersionId>,
    },

    EffectiveEngineCount {
        count: usize,
        engines: Vec<ContentVersionId>,
    },

    EffectiveGameCount {
        count: usize,
        games: Vec<ContentVersionId>,
    },

    GameEngineTargetCount {
        game: ContentVersionId,
        count: usize,
    },

    GameModGameTargetCount {
        game_mod: ContentVersionId,
        count: usize,
    },
}

impl fmt::Display for ResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootNotFound { id } => {
                write!(
                    formatter,
                    "no local Vapor Content version is available for Packagepack `{id}`"
                )
            }

            Self::RootNotPackagepack {
                identity,
                actual_kind,
            } => {
                write!(
                    formatter,
                    "resolution root `{identity}` is `{actual_kind}`, not a Packagepack"
                )
            }

            Self::DependencyNotFound {
                depender,
                binding,
                id,
                requirement,
            } => {
                write!(
                    formatter,
                    "`{depender}` dependency `{binding}` requires `{id}` `{requirement}`, but no matching local version is available"
                )
            }

            Self::DependencyCycle { cycle } => {
                formatter.write_str("Vapor dependency cycle detected: ")?;

                for (index, identity) in cycle.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(" -> ")?;
                    }

                    write!(formatter, "{identity}")?;
                }

                Ok(())
            }

            Self::EffectiveEngineCount { count, .. } => {
                write!(
                    formatter,
                    "resolved Packagepack must contain exactly one effective Engine, but found {count}"
                )
            }

            Self::EffectiveGameCount { count, .. } => {
                write!(
                    formatter,
                    "resolved Packagepack must contain exactly one effective Game, but found {count}"
                )
            }

            Self::GameEngineTargetCount { game, count } => {
                write!(
                    formatter,
                    "Game `{game}` must directly target exactly one Engine, but found {count}"
                )
            }

            Self::GameModGameTargetCount { game_mod, count } => {
                write!(
                    formatter,
                    "Game Mod `{game_mod}` must directly target exactly one Game, but found {count}"
                )
            }
        }
    }
}

impl std::error::Error for ResolutionError {}

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
