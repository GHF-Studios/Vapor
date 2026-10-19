//! Provider-neutral dependency graph resolution over exact candidates.

use crate::{
    ContentCandidate, ContentCandidates, ContentKind, ContentVersionId, DependencySpec, VaporId,
    allowed_pack_children,
};
use semver::VersionReq;
use std::collections::{BTreeMap, BTreeSet};

/// One exact resolved content node and its dependency bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedContentNode {
    /// Exact semantic identity.
    pub identity: ContentVersionId,
    /// Canonical artifact role.
    pub kind: ContentKind,
    /// Local binding to an exact dependency identity.
    pub dependencies: BTreeMap<String, ContentVersionId>,
    /// Authored accepted versions for each resolved binding.
    pub dependency_requirements: BTreeMap<String, VersionReq>,
}

/// Exact recursively resolved content graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedContentGraph {
    /// Exact selected graph root.
    pub root: ContentVersionId,
    /// Resolved nodes keyed by exact semantic identity.
    pub nodes: BTreeMap<ContentVersionId, ResolvedContentNode>,
}

impl ResolvedContentGraph {
    /// Find an exact node.
    pub fn node(&self, identity: &ContentVersionId) -> Option<&ResolvedContentNode> {
        self.nodes.get(identity)
    }

    /// Root node, guaranteed to exist in a successfully resolved graph.
    pub fn root_node(&self) -> &ResolvedContentNode {
        self.nodes
            .get(&self.root)
            .expect("resolved content graph root must exist")
    }

    /// Iterate nodes of one canonical role.
    pub fn content_of_kind(&self, kind: ContentKind) -> impl Iterator<Item = &ResolvedContentNode> {
        self.nodes.values().filter(move |node| node.kind == kind)
    }

    /// Visit nodes dependency-first, once per exact identity.
    ///
    /// Cyclic or incomplete graphs still produce a finite ordering; callers
    /// should inspect the resolution report before treating it as installable.
    pub fn dependency_first(&self) -> Vec<&ResolvedContentNode> {
        fn visit<'a>(
            identity: &ContentVersionId,
            graph: &'a ResolvedContentGraph,
            visited: &mut BTreeSet<ContentVersionId>,
            output: &mut Vec<&'a ResolvedContentNode>,
        ) {
            if !visited.insert(identity.clone()) {
                return;
            }
            let Some(node) = graph.nodes.get(identity) else {
                return;
            };
            for dependency in node.dependencies.values() {
                visit(dependency, graph, visited, output);
            }
            output.push(node);
        }

        let mut visited = BTreeSet::new();
        let mut output = Vec::with_capacity(self.nodes.len());
        visit(&self.root, self, &mut visited, &mut output);
        for identity in self.nodes.keys() {
            visit(identity, self, &mut visited, &mut output);
        }
        output
    }
}

/// Packagepack-specific complete playable composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedComposition {
    /// Exact Packagepack root.
    pub root: ContentVersionId,
    /// Exact nodes participating in the composition.
    pub nodes: BTreeMap<ContentVersionId, ResolvedContentNode>,
    /// The unique effective Engine.
    pub effective_engine: ContentVersionId,
    /// The unique effective Game.
    pub effective_game: ContentVersionId,
}

impl ResolvedComposition {
    /// Find an exact node.
    pub fn node(&self, identity: &ContentVersionId) -> Option<&ResolvedContentNode> {
        self.nodes.get(identity)
    }

    /// Root node, guaranteed to exist in a successfully resolved composition.
    pub fn root_node(&self) -> &ResolvedContentNode {
        self.nodes
            .get(&self.root)
            .expect("resolved composition root must exist")
    }
}

/// Validate generic kind relationships and the root kind's pack invariants.
pub fn validate_content_graph(graph: &ResolvedContentGraph) -> Result<(), ResolutionError> {
    validate_content_relationships(graph)?;
    validate_pack_shape(graph)
}

/// Validate graph integrity and content-kind relationships without applying
/// root-pack cardinality rules.
pub fn validate_content_relationships(graph: &ResolvedContentGraph) -> Result<(), ResolutionError> {
    if !graph.nodes.contains_key(&graph.root) {
        return Err(ResolutionError::RootNodeMissing {
            identity: graph.root.clone(),
        });
    }
    for node in graph.nodes.values() {
        for dependency in node.dependencies.values() {
            if !graph.nodes.contains_key(dependency) {
                return Err(ResolutionError::DependencyNodeMissing {
                    parent: Box::new(node.identity.clone()),
                    dependency: Box::new(dependency.clone()),
                });
            }
        }
    }
    validate_member_kinds(&graph.nodes)?;
    validate_target_relationships(&graph.nodes)
}

/// Validate and project a resolved Packagepack into a playable composition.
pub fn resolve_composition(
    graph: ResolvedContentGraph,
) -> Result<ResolvedComposition, ResolutionError> {
    let actual_kind = graph
        .nodes
        .get(&graph.root)
        .ok_or_else(|| ResolutionError::RootNodeMissing {
            identity: graph.root.clone(),
        })?
        .kind;
    if actual_kind != ContentKind::Packagepack {
        return Err(ResolutionError::RootNotPackagepack {
            identity: graph.root,
            actual_kind,
        });
    }
    validate_content_graph(&graph)?;
    let effective_engine = graph
        .content_of_kind(ContentKind::Engine)
        .next()
        .expect("validated Packagepack contains one Engine")
        .identity
        .clone();
    let effective_game = graph
        .content_of_kind(ContentKind::Game)
        .next()
        .expect("validated Packagepack contains one Game")
        .identity
        .clone();
    Ok(ResolvedComposition {
        root: graph.root,
        nodes: graph.nodes,
        effective_engine,
        effective_game,
    })
}

fn validate_member_kinds(
    nodes: &BTreeMap<ContentVersionId, ResolvedContentNode>,
) -> Result<(), ResolutionError> {
    for parent in nodes.values() {
        let Some(allowed_children) = allowed_pack_children(parent.kind) else {
            continue;
        };
        for child_identity in parent.dependencies.values() {
            let child = nodes
                .get(child_identity)
                .expect("resolved dependency nodes must exist in the graph");
            if !allowed_children.contains(&child.kind) {
                return Err(ResolutionError::PackChildKindNotAllowed {
                    parent: Box::new(parent.identity.clone()),
                    parent_kind: parent.kind,
                    child: Box::new(child.identity.clone()),
                    child_kind: child.kind,
                });
            }
        }
    }
    Ok(())
}

fn validate_pack_shape(graph: &ResolvedContentGraph) -> Result<(), ResolutionError> {
    match graph.root_node().kind {
        ContentKind::Packagepack => {
            require_exactly_one(graph, ContentKind::Engine)?;
            require_exactly_one(graph, ContentKind::Game)?;
        }
        ContentKind::Enginepack => require_exactly_one(graph, ContentKind::Engine)?,
        ContentKind::Gamepack => require_exactly_one(graph, ContentKind::Game)?,
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
        _ => unreachable!("exact-one validation applies only to Engine and Game"),
    }
}

fn validate_target_relationships(
    nodes: &BTreeMap<ContentVersionId, ResolvedContentNode>,
) -> Result<(), ResolutionError> {
    for node in nodes.values() {
        let target_count = |kind| {
            node.dependencies
                .values()
                .filter(|dependency| {
                    nodes
                        .get(*dependency)
                        .is_some_and(|target| target.kind == kind)
                })
                .count()
        };
        match node.kind {
            ContentKind::Game if target_count(ContentKind::Engine) != 1 => {
                return Err(ResolutionError::GameEngineTargetCount {
                    game: node.identity.clone(),
                    count: target_count(ContentKind::Engine),
                });
            }
            ContentKind::EngineMod if target_count(ContentKind::Engine) != 1 => {
                return Err(ResolutionError::EngineModEngineTargetCount {
                    engine_mod: node.identity.clone(),
                    count: target_count(ContentKind::Engine),
                });
            }
            ContentKind::GameMod if target_count(ContentKind::Game) != 1 => {
                return Err(ResolutionError::GameModGameTargetCount {
                    game_mod: node.identity.clone(),
                    count: target_count(ContentKind::Game),
                });
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

fn is_mod(kind: ContentKind) -> bool {
    matches!(
        kind,
        ContentKind::EngineMod | ContentKind::GameMod | ContentKind::ExtensionMod
    )
}

/// Resolve a stable content ID using the highest available versions.
pub fn resolve_latest(
    candidates: &ContentCandidates,
    id: &VaporId,
) -> Result<ResolvedContentGraph, ResolutionError> {
    let root = candidates
        .latest(id)
        .ok_or_else(|| ResolutionError::RootNotFound { id: id.clone() })?
        .identity
        .clone();
    resolve_exact(candidates, &root)
}

/// Resolve an exact root. Dependencies select their highest satisfying
/// candidates; global backtracking/version unification is deliberately not
/// implied by this operation.
pub fn resolve_exact(
    candidates: &ContentCandidates,
    root: &ContentVersionId,
) -> Result<ResolvedContentGraph, ResolutionError> {
    let root_candidate =
        candidates
            .get(root)
            .cloned()
            .ok_or_else(|| ResolutionError::RootNotFound {
                id: root.id.clone(),
            })?;
    resolve_from(root_candidate, |_, _, dependency| {
        candidates
            .latest_matching(&dependency.id, &dependency.version)
            .cloned()
    })
}

/// Resolve required edges from a host-selected root while keeping graph
/// traversal, exact-edge validation, convergence, and cycle detection shared.
///
/// The host selector decides which eligible exact artifact supplies each edge
/// (for example, an installed copy or a workspace candidate). It cannot bypass
/// the authored ID or version requirement. Provider availability and source
/// provenance remain outside the resolved graph.
pub fn resolve_from<F>(
    root: ContentCandidate,
    select_dependency: F,
) -> Result<ResolvedContentGraph, ResolutionError>
where
    F: FnMut(&ContentCandidate, &str, &DependencySpec) -> Option<ContentCandidate>,
{
    resolve_report_from(root, select_dependency).into_result()
}

/// Resolve as much of a graph as possible and retain missing-edge or cycle
/// diagnostics alongside the known nodes. This is suitable for install
/// previews that need to show available content even when blocked.
pub fn resolve_report_from<F>(root: ContentCandidate, select_dependency: F) -> ResolutionReport
where
    F: FnMut(&ContentCandidate, &str, &DependencySpec) -> Option<ContentCandidate>,
{
    let root_identity = root.identity.clone();
    let mut resolver = Resolver {
        select_dependency,
        nodes: BTreeMap::new(),
        stack: Vec::new(),
        issues: Vec::new(),
    };
    resolver.resolve_candidate(root);
    ResolutionReport {
        graph: ResolvedContentGraph {
            root: root_identity,
            nodes: resolver.nodes,
        },
        issues: resolver.issues,
    }
}

/// A partial graph and diagnostics produced by provider-aware resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionReport {
    /// Nodes whose required dependencies were found.
    pub graph: ResolvedContentGraph,
    /// Missing required edges or cycles that prevented a complete graph.
    pub issues: Vec<ResolutionError>,
}

impl ResolutionReport {
    /// Whether every required edge resolved and no cycle was found.
    pub fn is_complete(&self) -> bool {
        self.issues.is_empty()
    }

    /// Return the complete graph or its first resolution issue.
    pub fn into_result(self) -> Result<ResolvedContentGraph, ResolutionError> {
        match self.issues.into_iter().next() {
            Some(issue) => Err(issue),
            None => Ok(self.graph),
        }
    }
}

struct Resolver<F> {
    select_dependency: F,
    nodes: BTreeMap<ContentVersionId, ResolvedContentNode>,
    stack: Vec<ContentVersionId>,
    issues: Vec<ResolutionError>,
}

impl<F> Resolver<F>
where
    F: FnMut(&ContentCandidate, &str, &DependencySpec) -> Option<ContentCandidate>,
{
    fn resolve_candidate(&mut self, candidate: ContentCandidate) {
        let identity = candidate.identity.clone();
        if let Some(start) = self.stack.iter().position(|active| active == &identity) {
            let mut cycle = self.stack[start..].to_vec();
            cycle.push(identity);
            self.issues.push(ResolutionError::DependencyCycle { cycle });
            return;
        }
        if self.nodes.contains_key(&identity) {
            return;
        }
        self.stack.push(identity.clone());

        let mut resolved_dependencies = BTreeMap::new();
        let mut resolved_requirements = BTreeMap::new();
        for (binding, dependency) in &candidate.dependencies {
            // Optional content is available for an explicit later choice but
            // does not participate in the default resolved composition.
            if dependency.optional {
                continue;
            }
            let selected =
                (self.select_dependency)(&candidate, binding, dependency).filter(|selected| {
                    selected.identity.id == dependency.id
                        && dependency.version.matches(&selected.identity.version)
                });
            let Some(selected) = selected else {
                self.issues.push(ResolutionError::DependencyNotFound {
                    depender: identity.clone(),
                    binding: binding.clone(),
                    id: dependency.id.clone(),
                    requirement: Box::new(dependency.version.clone()),
                });
                continue;
            };
            let selected_identity = selected.identity.clone();
            self.resolve_candidate(selected);
            if self.nodes.contains_key(&selected_identity) {
                resolved_dependencies.insert(binding.clone(), selected_identity);
                resolved_requirements.insert(binding.clone(), dependency.version.clone());
            }
        }

        let popped = self.stack.pop();
        debug_assert_eq!(popped.as_ref(), Some(&identity));
        self.nodes.insert(
            identity.clone(),
            ResolvedContentNode {
                identity,
                kind: candidate.kind,
                dependencies: resolved_dependencies,
                dependency_requirements: resolved_requirements,
            },
        );
    }
}

/// Failure to resolve a dependency graph from the supplied candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionError {
    /// No candidate exists for the root identity.
    RootNotFound { id: VaporId },
    /// No candidate satisfies one required dependency.
    DependencyNotFound {
        depender: ContentVersionId,
        binding: String,
        id: VaporId,
        requirement: Box<VersionReq>,
    },
    /// One exact version recursively depends on itself.
    DependencyCycle { cycle: Vec<ContentVersionId> },
    /// The requested playable root is not a Packagepack.
    RootNotPackagepack {
        identity: ContentVersionId,
        actual_kind: ContentKind,
    },
    /// A graph references a root identity with no node.
    RootNodeMissing { identity: ContentVersionId },
    /// A graph edge references a node absent from the graph.
    DependencyNodeMissing {
        parent: Box<ContentVersionId>,
        dependency: Box<ContentVersionId>,
    },
    /// A graph root has a different kind than requested.
    RootKindMismatch {
        identity: ContentVersionId,
        expected_kind: ContentKind,
        actual_kind: ContentKind,
    },
    /// A pack has a child kind outside its canonical vocabulary.
    PackChildKindNotAllowed {
        parent: Box<ContentVersionId>,
        parent_kind: ContentKind,
        child: Box<ContentVersionId>,
        child_kind: ContentKind,
    },
    /// The graph does not contain exactly one effective Engine.
    EffectiveEngineCount {
        count: usize,
        engines: Vec<ContentVersionId>,
    },
    /// The graph does not contain exactly one effective Game.
    EffectiveGameCount {
        count: usize,
        games: Vec<ContentVersionId>,
    },
    /// A Modpack resolves no mod content.
    ModpackContainsNoMods { modpack: ContentVersionId },
    /// A Game does not target exactly one Engine.
    GameEngineTargetCount {
        game: ContentVersionId,
        count: usize,
    },
    /// An Engine Mod does not target exactly one Engine.
    EngineModEngineTargetCount {
        engine_mod: ContentVersionId,
        count: usize,
    },
    /// A Game Mod does not target exactly one Game.
    GameModGameTargetCount {
        game_mod: ContentVersionId,
        count: usize,
    },
    /// An Extension Mod does not target exactly one other Mod.
    ExtensionModTargetCount {
        extension_mod: ContentVersionId,
        count: usize,
    },
}

impl std::fmt::Display for ResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootNotFound { id } => write!(
                formatter,
                "no local Vapor Content version is available for `{id}`"
            ),
            Self::DependencyNotFound {
                depender,
                binding,
                id,
                requirement,
            } => write!(
                formatter,
                "`{depender}` dependency `{binding}` requires `{id}` `{requirement}`, but no matching local version is available"
            ),
            Self::DependencyCycle { cycle } => {
                write!(formatter, "content dependency cycle: {cycle:?}")
            }
            Self::RootNotPackagepack {
                identity,
                actual_kind,
            } => write!(
                formatter,
                "resolution root `{identity}` is `{actual_kind}`, not a Packagepack"
            ),
            Self::RootNodeMissing { identity } => {
                write!(formatter, "resolved graph has no root node for {identity}")
            }
            Self::DependencyNodeMissing { parent, dependency } => write!(
                formatter,
                "resolved node {parent} references missing dependency {dependency}"
            ),
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
            Self::PackChildKindNotAllowed {
                parent,
                parent_kind,
                child,
                child_kind,
            } => write!(
                formatter,
                "{parent_kind} `{parent}` cannot contain {child_kind} `{child}`"
            ),
            Self::EffectiveEngineCount { count, .. } => write!(
                formatter,
                "resolved pack must contain exactly one effective Engine, but found {count}"
            ),
            Self::EffectiveGameCount { count, .. } => write!(
                formatter,
                "resolved pack must contain exactly one effective Game, but found {count}"
            ),
            Self::ModpackContainsNoMods { modpack } => {
                write!(
                    formatter,
                    "resolved Modpack `{modpack}` does not contain any Mods"
                )
            }
            Self::GameEngineTargetCount { game, count } => write!(
                formatter,
                "Game `{game}` must directly target exactly one Engine, but found {count}"
            ),
            Self::EngineModEngineTargetCount { engine_mod, count } => write!(
                formatter,
                "Engine Mod `{engine_mod}` must directly target exactly one Engine, but found {count}"
            ),
            Self::GameModGameTargetCount { game_mod, count } => write!(
                formatter,
                "Game Mod `{game_mod}` must directly target exactly one Game, but found {count}"
            ),
            Self::ExtensionModTargetCount {
                extension_mod,
                count,
            } => write!(
                formatter,
                "Extension Mod `{extension_mod}` must directly target exactly one Mod, but found {count}"
            ),
        }
    }
}

impl std::error::Error for ResolutionError {}
