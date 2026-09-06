//! Resolution of authored Vapor dependency declarations into exact Content.
//!
//! Resolution itself is generic over Vapor Content kinds.
//! Pack-specific semantic validation is layered on top of the resolved graph.
//!
//! Vertical Slice 0 currently resolves only against a `LocalCatalog`.
//! Full Cargo-style global version unification/backtracking remains deferred.

use crate::{ContentKind, ContentVersionId, LocalCatalog, VaporId};
use semver::VersionReq;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedContentNode {
    pub identity: ContentVersionId,
    pub kind: ContentKind,

    /// Local dependency binding -> exact resolved Content identity.
    pub dependencies: BTreeMap<String, ContentVersionId>,
}

/// Exact recursively resolved Vapor Content graph.
///
/// This structure does not imply that the root is a Packagepack or complete
/// Vapor App Composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedContentGraph {
    pub root: ContentVersionId,
    pub nodes: BTreeMap<ContentVersionId, ResolvedContentNode>,
}

impl ResolvedContentGraph {
    pub fn node(&self, identity: &ContentVersionId) -> Option<&ResolvedContentNode> {
        self.nodes.get(identity)
    }

    pub fn root_node(&self) -> &ResolvedContentNode {
        self.nodes
            .get(&self.root)
            .expect("resolved Content graph root must exist")
    }

    pub fn content_of_kind(&self, kind: ContentKind) -> impl Iterator<Item = &ResolvedContentNode> {
        self.nodes.values().filter(move |node| node.kind == kind)
    }
}

/// Packagepack-specific resolved composition.
///
/// Kept separate from `ResolvedContentGraph` because a generic Content graph
/// does not necessarily form a complete runnable composition.
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
    let root_content = catalog
        .latest(content_id)
        .ok_or_else(|| ResolutionError::RootNotFound {
            id: content_id.clone(),
        })?;

    let root = root_content.version_id();

    let mut resolver = LocalResolver {
        catalog,
        nodes: BTreeMap::new(),
        stack: Vec::new(),
    };

    resolver.resolve_identity(root.clone())?;

    Ok(ResolvedContentGraph {
        root,
        nodes: resolver.nodes,
    })
}

/// Resolve arbitrary Vapor Content while requiring one expected root kind.
///
/// Resolution remains generic. This layer only adds:
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

    validate_resolved_content_graph(&graph)?;
    validate_pack_graph(&graph)?;

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
    let actual_kind = graph.root_node().kind;

    if actual_kind != ContentKind::Packagepack {
        return Err(ResolutionError::RootNotPackagepack {
            identity: graph.root,
            actual_kind,
        });
    }

    validate_resolved_content_graph(&graph)?;
    validate_pack_graph(&graph)?;
    composition_from_graph(graph)
}

/// Validate semantic relationships that belong to Content kinds themselves,
/// independently of which pack happens to include them.
pub fn validate_resolved_content_graph(
    graph: &ResolvedContentGraph,
) -> Result<(), ResolutionError> {
    validate_target_relationships(&graph.nodes)
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

fn validate_pack_graph(graph: &ResolvedContentGraph) -> Result<(), ResolutionError> {
    match graph.root_node().kind {
        ContentKind::Packagepack => {
            require_exactly_one(graph, ContentKind::Engine)?;

            require_exactly_one(graph, ContentKind::Game)?;
        }

        ContentKind::Enginepack => {
            require_exactly_one(graph, ContentKind::Engine)?;
        }

        ContentKind::Gamepack => {
            require_exactly_one(graph, ContentKind::Game)?;
        }

        ContentKind::Modpack => {
            let count = graph
                .nodes
                .values()
                .filter(|node| is_mod(node.kind))
                .count();

            if count == 0 {
                return Err(ResolutionError::ModpackContainsNoMods {
                    modpack: graph.root.clone(),
                });
            }
        }

        _ => {}
    }

    Ok(())
}

fn require_exactly_one(
    graph: &ResolvedContentGraph,
    kind: ContentKind,
) -> Result<(), ResolutionError> {
    let identities: Vec<_> = graph
        .content_of_kind(kind)
        .map(|node| node.identity.clone())
        .collect();

    if identities.len() == 1 {
        return Ok(());
    }

    match kind {
        ContentKind::Engine => Err(ResolutionError::EffectiveEngineCount {
            count: identities.len(),
            engines: identities,
        }),

        ContentKind::Game => Err(ResolutionError::EffectiveGameCount {
            count: identities.len(),
            games: identities,
        }),

        _ => unreachable!("exact-one validation currently applies only to Engine and Game"),
    }
}

fn composition_from_graph(
    graph: ResolvedContentGraph,
) -> Result<ResolvedComposition, ResolutionError> {
    let effective_engine = graph
        .content_of_kind(ContentKind::Engine)
        .next()
        .expect("Packagepack validation guarantees one Engine")
        .identity
        .clone();

    let effective_game = graph
        .content_of_kind(ContentKind::Game)
        .next()
        .expect("Packagepack validation guarantees one Game")
        .identity
        .clone();

    Ok(ResolvedComposition {
        root: graph.root,
        nodes: graph.nodes,
        effective_engine,
        effective_game,
    })
}

fn validate_target_relationships(
    nodes: &BTreeMap<ContentVersionId, ResolvedContentNode>,
) -> Result<(), ResolutionError> {
    for node in nodes.values() {
        match node.kind {
            ContentKind::Game => {
                let count = direct_dependency_count_of_kind(nodes, node, ContentKind::Engine);

                if count != 1 {
                    return Err(ResolutionError::GameEngineTargetCount {
                        game: node.identity.clone(),
                        count,
                    });
                }
            }

            ContentKind::EngineMod => {
                let count = direct_dependency_count_of_kind(nodes, node, ContentKind::Engine);

                if count != 1 {
                    return Err(ResolutionError::EngineModEngineTargetCount {
                        engine_mod: node.identity.clone(),
                        count,
                    });
                }
            }

            ContentKind::GameMod => {
                let count = direct_dependency_count_of_kind(nodes, node, ContentKind::Game);

                if count != 1 {
                    return Err(ResolutionError::GameModGameTargetCount {
                        game_mod: node.identity.clone(),
                        count,
                    });
                }
            }

            ContentKind::ExtensionMod => {
                let count = node
                    .dependencies
                    .values()
                    .filter(|dependency| {
                        nodes
                            .get(*dependency)
                            .is_some_and(|target| is_mod(target.kind))
                    })
                    .count();

                if count != 1 {
                    return Err(ResolutionError::ExtensionModTargetCount {
                        extension_mod: node.identity.clone(),
                        count,
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
                .is_some_and(|target| target.kind == kind)
        })
        .count()
}

fn is_mod(kind: ContentKind) -> bool {
    matches!(
        kind,
        ContentKind::EngineMod | ContentKind::GameMod | ContentKind::ExtensionMod
    )
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

    RootKindMismatch {
        identity: ContentVersionId,
        expected_kind: ContentKind,
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

    ModpackContainsNoMods {
        modpack: ContentVersionId,
    },

    GameEngineTargetCount {
        game: ContentVersionId,
        count: usize,
    },

    EngineModEngineTargetCount {
        engine_mod: ContentVersionId,
        count: usize,
    },

    GameModGameTargetCount {
        game_mod: ContentVersionId,
        count: usize,
    },

    ExtensionModTargetCount {
        extension_mod: ContentVersionId,
        count: usize,
    },
}

impl fmt::Display for ResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootNotFound { id } => {
                write!(
                    formatter,
                    "no local Vapor Content version is available for `{id}`"
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

            Self::RootKindMismatch {
                identity,
                expected_kind,
                actual_kind,
            } => {
                write!(
                    formatter,
                    "`{identity}` is `{actual_kind}`, not `{expected_kind}`"
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
                    "resolved pack must contain exactly one effective Engine, but found {count}"
                )
            }

            Self::EffectiveGameCount { count, .. } => {
                write!(
                    formatter,
                    "resolved pack must contain exactly one effective Game, but found {count}"
                )
            }

            Self::ModpackContainsNoMods { modpack } => {
                write!(
                    formatter,
                    "resolved Modpack `{modpack}` does not contain any Mods"
                )
            }

            Self::GameEngineTargetCount { game, count } => {
                write!(
                    formatter,
                    "Game `{game}` must directly target exactly one Engine, but found {count}"
                )
            }

            Self::EngineModEngineTargetCount { engine_mod, count } => {
                write!(
                    formatter,
                    "Engine Mod `{engine_mod}` must directly target exactly one Engine, but found {count}"
                )
            }

            Self::GameModGameTargetCount { game_mod, count } => {
                write!(
                    formatter,
                    "Game Mod `{game_mod}` must directly target exactly one Game, but found {count}"
                )
            }

            Self::ExtensionModTargetCount {
                extension_mod,
                count,
            } => {
                write!(
                    formatter,
                    "Extension Mod `{extension_mod}` must directly target exactly one Mod, but found {count}"
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
