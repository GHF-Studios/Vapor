use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
use vapor_content::{
    ContentKind, ContentReference, ContentVersionId, ResolvedComposition, ResolvedContentNode,
};

pub const COMPOSITION_SCHEMA_VERSION: u32 = 1;
pub const CURRENT_SCHEMA_VERSION: u32 = COMPOSITION_SCHEMA_VERSION;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedDependencyIdentity {
    pub binding: String,
    pub content: ContentReference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_requirement: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedNodeIdentity {
    pub content: ContentReference,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub dependencies: Vec<ResolvedDependencyIdentity>,
}

/// Durable serialization projection of a shared resolver result plus
/// installation observations. Selection and compatibility policy live in
/// `vapor_content`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCompositionGraph {
    pub root: ContentReference,
    pub effective_engine: ContentReference,
    #[serde(default)]
    pub effective_game: Option<ContentReference>,
    pub nodes: Vec<ResolvedNodeIdentity>,
}

impl From<&ResolvedComposition> for ResolvedCompositionGraph {
    /// Project shared resolver output into the stable wire/storage shape.
    fn from(composition: &ResolvedComposition) -> Self {
        let reference = |identity: &ContentVersionId| ContentReference::from(identity.clone());
        Self {
            root: reference(&composition.root),
            effective_engine: reference(&composition.effective_engine),
            effective_game: Some(reference(&composition.effective_game)),
            nodes: composition
                .nodes
                .values()
                .map(|node| ResolvedNodeIdentity {
                    content: reference(&node.identity),
                    kind: node.kind.to_string(),
                    fingerprint: None,
                    source: None,
                    dependencies: node
                        .dependencies
                        .iter()
                        .map(|(binding, identity)| ResolvedDependencyIdentity {
                            binding: binding.clone(),
                            content: reference(identity),
                            version_requirement: node
                                .dependency_requirements
                                .get(binding)
                                .map(ToString::to_string),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

impl TryFrom<&ResolvedCompositionGraph> for ResolvedComposition {
    type Error = String;

    /// Recover the resolver-owned graph, ignoring installation observations.
    fn try_from(snapshot: &ResolvedCompositionGraph) -> Result<Self, Self::Error> {
        let exact = |reference: &ContentReference| {
            reference
                .exact_version_id()?
                .ok_or_else(|| format!("content reference '{}' has no exact version", reference.id))
        };
        let root = exact(&snapshot.root)?;
        let effective_engine = exact(&snapshot.effective_engine)?;
        let effective_game = snapshot
            .effective_game
            .as_ref()
            .map(exact)
            .transpose()?
            .ok_or_else(|| "resolved composition snapshot has no effective Game".to_owned())?;
        let mut nodes = BTreeMap::new();
        for node in &snapshot.nodes {
            let identity = exact(&node.content)?;
            let kind = ContentKind::ALL
                .into_iter()
                .find(|kind| kind.as_str() == node.kind.as_str())
                .ok_or_else(|| {
                    format!(
                        "unknown content kind '{}' in composition snapshot",
                        node.kind
                    )
                })?;
            let mut dependencies = BTreeMap::new();
            let mut dependency_requirements = BTreeMap::new();
            for dependency in &node.dependencies {
                let target = exact(&dependency.content)?;
                if let Some(requirement) = dependency.version_requirement.as_deref() {
                    let requirement: semver::VersionReq = requirement.parse().map_err(|error| {
                        format!(
                            "invalid version requirement '{requirement}' for '{}': {error}",
                            dependency.content.id
                        )
                    })?;
                    if !requirement.matches(&target.version) {
                        return Err(format!(
                            "resolved dependency '{}' does not satisfy its saved requirement '{requirement}'",
                            dependency.content.id
                        ));
                    }
                    dependency_requirements.insert(dependency.binding.clone(), requirement);
                }
                if dependencies
                    .insert(dependency.binding.clone(), target)
                    .is_some()
                {
                    return Err(format!(
                        "duplicate dependency binding '{}' on '{}'",
                        dependency.binding, node.content.id
                    ));
                }
            }
            let resolved = ResolvedContentNode {
                identity: identity.clone(),
                kind,
                dependencies,
                dependency_requirements,
            };
            if nodes.insert(identity.clone(), resolved).is_some() {
                return Err(format!("duplicate resolved node '{identity}'"));
            }
        }
        for node in nodes.values() {
            for dependency in node.dependencies.values() {
                if !nodes.contains_key(dependency) {
                    return Err(format!(
                        "resolved node '{}' references missing dependency '{dependency}'",
                        node.identity
                    ));
                }
            }
        }
        if !nodes.contains_key(&root) {
            return Err(format!("resolved composition root '{root}' is absent"));
        }
        if nodes.get(&root).map(|node| node.kind) != Some(ContentKind::Packagepack) {
            return Err(format!(
                "resolved composition root '{root}' is not a Packagepack"
            ));
        }
        if nodes.get(&effective_engine).map(|node| node.kind) != Some(ContentKind::Engine) {
            return Err(format!(
                "effective Engine '{effective_engine}' is absent or has the wrong content kind"
            ));
        }
        if nodes.get(&effective_game).map(|node| node.kind) != Some(ContentKind::Game) {
            return Err(format!(
                "effective Game '{effective_game}' is absent or has the wrong content kind"
            ));
        }
        Ok(Self {
            root,
            nodes,
            effective_engine,
            effective_game,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceAssociation {
    /// Source tree explicitly associated with this composition for source-run
    /// operations. The active authoring workspace is stored separately in the
    /// durable client snapshot.
    pub root: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LaunchSettings {
    pub arguments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledArtifactRecord {
    pub id: String,
    #[serde(default)]
    pub version: Option<String>,
    pub root: PathBuf,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum InstallationState {
    #[default]
    Unknown,
    NotInstalled,
    Installed {
        artifacts: Vec<InstalledArtifactRecord>,
        graph_fingerprint: Option<String>,
    },
    OutOfDate {
        detail: String,
        actual_graph: ResolvedCompositionGraph,
        artifacts: Vec<InstalledArtifactRecord>,
        #[serde(default)]
        verified: bool,
    },
    Partial {
        detail: String,
    },
    NeedsRepair {
        detail: String,
        artifacts: Vec<InstalledArtifactRecord>,
    },
}

/// User intent over the durable playable-composition aggregate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum CompositionAction {
    Create {
        name: String,
    },
    Duplicate {
        composition_id: String,
        name: String,
    },
    Rename {
        composition_id: String,
        name: String,
    },
    Remove {
        composition_id: String,
    },
    Select {
        composition_id: String,
    },
    SetRoot {
        composition_id: String,
        content_id: Option<String>,
        content_version: Option<String>,
        workspace_root: Option<String>,
    },
    ResolveDefaultRoot {
        composition_id: String,
    },
    SetLaunchArguments {
        composition_id: String,
        arguments: Vec<String>,
    },
    AdoptVerifiedGraph {
        composition_id: String,
    },
    InvalidateContent {
        content_id: String,
        content_version: Option<String>,
    },
    Preview {
        composition_id: String,
        #[serde(default)]
        selector: Option<String>,
    },
    Install {
        composition_id: String,
    },
    Verify {
        composition_id: String,
    },
    Repair {
        composition_id: String,
    },
    Update {
        composition_id: String,
    },
    Launch {
        composition_id: String,
    },
    RunSource {
        composition_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Composition {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub root_package: Option<ContentReference>,
    #[serde(default)]
    pub resolved_graph: Option<ResolvedCompositionGraph>,
    /// Optional source binding for this composition, not the active editor
    /// workspace. Opening an authoring workspace must not change this value.
    #[serde(default)]
    pub workspace: Option<WorkspaceAssociation>,
    #[serde(default)]
    pub launch: LaunchSettings,
    #[serde(default)]
    pub installation: InstallationState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositionDocument {
    pub schema_version: u32,
    #[serde(alias = "profiles")]
    pub compositions: Vec<Composition>,
}

impl Default for CompositionDocument {
    fn default() -> Self {
        Self {
            schema_version: COMPOSITION_SCHEMA_VERSION,
            compositions: vec![Composition::default_composition()],
        }
    }
}

impl Composition {
    pub fn default_composition() -> Self {
        Self {
            id: "default".to_owned(),
            name: "Default".to_owned(),
            root_package: None,
            resolved_graph: None,
            workspace: None,
            launch: LaunchSettings::default(),
            installation: InstallationState::Unknown,
        }
    }
}

/// Stable fingerprint for an installed graph with exact payload and source
/// records for every node. Returns `None` for legacy or partial snapshots.
pub fn fingerprint_resolved_graph(graph: &ResolvedCompositionGraph) -> Option<String> {
    if graph.nodes.is_empty()
        || graph.nodes.iter().any(|node| {
            node.fingerprint.is_none() || node.source.as_deref().is_none_or(str::is_empty)
        })
    {
        return None;
    }
    let identity = |content: &ContentReference| {
        let version = content.version.as_deref().map_or_else(
            || "0".to_owned(),
            |version| format!("1{}:{}", version.len(), version),
        );
        format!("{}:{}{version}", content.id.len(), content.id)
    };
    let mut canonical = String::new();
    canonical.push_str(&format!("root={}\n", identity(&graph.root)));
    canonical.push_str(&format!("engine={}\n", identity(&graph.effective_engine)));
    canonical.push_str(&format!(
        "game={}\n",
        graph
            .effective_game
            .as_ref()
            .map(identity)
            .unwrap_or_default()
    ));
    let nodes: BTreeMap<_, _> = graph
        .nodes
        .iter()
        .map(|node| (identity(&node.content), node))
        .collect();
    if nodes.len() != graph.nodes.len() {
        return None;
    }
    for (content, node) in nodes {
        canonical.push_str(&format!("node={content}\n"));
        canonical.push_str(&format!("kind={}:{}\n", node.kind.len(), node.kind));
        let source = node.source.as_deref().unwrap_or_default();
        canonical.push_str(&format!("source={}:{}\n", source.len(), source));
        let fingerprint = node.fingerprint.as_deref().unwrap_or_default();
        canonical.push_str(&format!("payload={}:{}\n", fingerprint.len(), fingerprint));
        let mut dependencies = node
            .dependencies
            .iter()
            .map(|dependency| {
                let target = identity(&dependency.content);
                format!(
                    "{}:{}{}:{}{}:{}",
                    dependency.binding.len(),
                    dependency.binding,
                    target.len(),
                    target,
                    dependency
                        .version_requirement
                        .as_deref()
                        .unwrap_or("")
                        .len(),
                    dependency.version_requirement.as_deref().unwrap_or("")
                )
            })
            .collect::<Vec<_>>();
        dependencies.sort();
        for dependency in dependencies {
            canonical.push_str(&format!("edge={content}{dependency}\n"));
        }
    }
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in canonical.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Some(format!("vapor-graph-fnv1a-64-v2:{hash:016x}"))
}

/// Check that an installed graph still satisfies the identities saved by a composition.
/// Missing legacy payload/source facts are tolerated once so a successful launch can
/// upgrade that composition to a complete lock. Facts already present must match exactly.
pub fn resolved_graph_matches(
    expected: &ResolvedCompositionGraph,
    actual: &ResolvedCompositionGraph,
) -> bool {
    if expected.root != actual.root
        || expected.effective_engine != actual.effective_engine
        || expected.effective_game != actual.effective_game
    {
        return false;
    }
    let Some(expected_nodes) = canonical_resolved_nodes(expected) else {
        return false;
    };
    let Some(actual_nodes) = canonical_resolved_nodes(actual) else {
        return false;
    };
    if expected_nodes.len() != actual_nodes.len() {
        return false;
    }
    expected_nodes.iter().all(|(identity, expected)| {
        let Some(actual) = actual_nodes.get(identity) else {
            return false;
        };
        expected.kind == actual.kind
            && expected.dependencies.len() == actual.dependencies.len()
            && expected.dependencies.iter().zip(&actual.dependencies).all(
                |(
                    (expected_binding, expected_content, expected_requirement),
                    (actual_binding, actual_content, actual_requirement),
                )| {
                    expected_binding == actual_binding
                        && expected_content == actual_content
                        && expected_requirement.as_ref().is_none_or(|requirement| {
                            actual_requirement.as_ref() == Some(requirement)
                        })
                },
            )
            && expected
                .fingerprint
                .is_none_or(|fingerprint| actual.fingerprint == Some(fingerprint))
            && expected
                .source
                .is_none_or(|source| actual.source == Some(source))
    })
}

struct CanonicalResolvedNode<'a> {
    kind: &'a str,
    fingerprint: Option<&'a str>,
    source: Option<&'a str>,
    dependencies: Vec<(String, ContentReference, Option<String>)>,
}

fn canonical_resolved_nodes(
    graph: &ResolvedCompositionGraph,
) -> Option<BTreeMap<ContentReference, CanonicalResolvedNode<'_>>> {
    let mut nodes = BTreeMap::new();
    for node in &graph.nodes {
        let mut bindings = BTreeSet::new();
        let mut dependencies = node
            .dependencies
            .iter()
            .map(|dependency| {
                if !bindings.insert(dependency.binding.clone()) {
                    return None;
                }
                Some((
                    dependency.binding.clone(),
                    dependency.content.clone(),
                    dependency.version_requirement.clone(),
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        dependencies.sort();
        if nodes
            .insert(
                node.content.clone(),
                CanonicalResolvedNode {
                    kind: &node.kind,
                    fingerprint: node.fingerprint.as_deref(),
                    source: node.source.as_deref(),
                    dependencies,
                },
            )
            .is_some()
        {
            return None;
        }
    }
    if !nodes.contains_key(&graph.root)
        || !nodes.contains_key(&graph.effective_engine)
        || graph
            .effective_game
            .as_ref()
            .is_some_and(|game| !nodes.contains_key(game))
        || nodes.values().any(|node| {
            node.dependencies
                .iter()
                .any(|(_, dependency, _)| !nodes.contains_key(dependency))
        })
    {
        return None;
    }
    Some(nodes)
}

/// Add an empty composition with no root package or source-workspace binding.
pub fn add_composition(compositions: &mut Vec<Composition>, name: &str) -> Result<usize, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a non-empty composition name.".to_owned());
    }
    if compositions
        .iter()
        .any(|composition| composition.name.eq_ignore_ascii_case(name))
    {
        return Err("A composition with that name already exists.".to_owned());
    }
    let id = next_composition_id(compositions);
    compositions.push(Composition {
        id,
        name: name.to_owned(),
        root_package: None,
        resolved_graph: None,
        workspace: None,
        launch: LaunchSettings::default(),
        installation: InstallationState::Unknown,
    });
    Ok(compositions.len() - 1)
}

/// Rename one composition while preserving case-insensitive name uniqueness.
pub fn rename_composition(
    compositions: &mut [Composition],
    index: usize,
    name: &str,
) -> Result<(), String> {
    if index >= compositions.len() {
        return Err("The selected composition no longer exists.".to_owned());
    }
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a non-empty composition name.".to_owned());
    }
    if compositions
        .iter()
        .enumerate()
        .any(|(other_index, other)| other_index != index && other.name.eq_ignore_ascii_case(name))
    {
        return Err("A composition with that name already exists.".to_owned());
    }
    compositions[index].name = name.to_owned();
    Ok(())
}

/// Remove one composition without allowing a composition set to become empty.
pub fn remove_composition(
    compositions: &mut Vec<Composition>,
    index: usize,
) -> Result<Composition, String> {
    if compositions.len() <= 1 {
        return Err("Studio must keep at least one composition.".to_owned());
    }
    if index >= compositions.len() {
        return Err("The selected composition no longer exists.".to_owned());
    }
    Ok(compositions.remove(index))
}

/// Duplicate a composition's root, resolved graph, workspace, and launch settings.
/// Installed artifacts remain app-owned and are therefore shared by the copy.
pub fn duplicate_composition(
    compositions: &mut Vec<Composition>,
    index: usize,
) -> Result<usize, String> {
    let source = compositions
        .get(index)
        .cloned()
        .ok_or_else(|| "The selected composition no longer exists.".to_owned())?;
    let mut copy = source.clone();
    copy.id = next_composition_id(compositions);
    let base_name = format!("{} Copy", source.name);
    copy.name = unique_composition_name(compositions, &base_name);
    compositions.push(copy);
    Ok(compositions.len() - 1)
}

/// Apply a state-only composition intent to the shared durable client state.
/// Host-specific default Packagepack discovery is injected only for the one
/// action that requires it; operation-start actions remain service-owned.
pub fn apply_composition_action<F>(
    state: &mut crate::ClientStateDocument,
    action: CompositionAction,
    resolve_default_root: F,
) -> Result<(), String>
where
    F: FnOnce() -> Result<String, String>,
{
    match action {
        CompositionAction::Create { name } => {
            let index = add_composition(&mut state.compositions.compositions, &name)?;
            state.active_composition_id = state.compositions.compositions[index].id.clone();
        }
        CompositionAction::Duplicate {
            composition_id,
            name,
        } => {
            let index = composition_index(state, &composition_id)?;
            let new_index = duplicate_composition(&mut state.compositions.compositions, index)?;
            if !name.trim().is_empty() {
                rename_composition(&mut state.compositions.compositions, new_index, &name)?;
            }
            state.active_composition_id = state.compositions.compositions[new_index].id.clone();
        }
        CompositionAction::Rename {
            composition_id,
            name,
        } => {
            let index = composition_index(state, &composition_id)?;
            rename_composition(&mut state.compositions.compositions, index, &name)?;
        }
        CompositionAction::Remove { composition_id } => {
            let index = composition_index(state, &composition_id)?;
            remove_composition(&mut state.compositions.compositions, index)?;
            if state.active_composition_id == composition_id {
                state.active_composition_id = state.compositions.compositions[0].id.clone();
            }
        }
        CompositionAction::Select { composition_id } => {
            let index = composition_index(state, &composition_id)?;
            state.active_composition_id = state.compositions.compositions[index].id.clone();
        }
        CompositionAction::SetRoot {
            composition_id,
            content_id,
            content_version,
            workspace_root,
        } => {
            let index = composition_index(state, &composition_id)?;
            let composition = &mut state.compositions.compositions[index];
            composition.root_package = content_id.map(|id| ContentReference {
                id,
                version: content_version,
            });
            composition.resolved_graph = None;
            composition.installation = InstallationState::Unknown;
            composition.workspace = workspace_root
                .map(PathBuf::from)
                .map(|root| WorkspaceAssociation { root });
        }
        CompositionAction::ResolveDefaultRoot { composition_id } => {
            let index = composition_index(state, &composition_id)?;
            if state.compositions.compositions[index]
                .root_package
                .is_some()
            {
                return Err("This composition already has a selected composition root.".to_owned());
            }
            let content_id = resolve_default_root().map_err(|error| {
                format!("This app has no default playable composition: {error}")
            })?;
            let composition = &mut state.compositions.compositions[index];
            composition.root_package = Some(ContentReference {
                id: content_id,
                version: None,
            });
            composition.resolved_graph = None;
            composition.installation = InstallationState::Unknown;
        }
        CompositionAction::SetLaunchArguments {
            composition_id,
            arguments,
        } => {
            if arguments.len() > 64 || arguments.iter().any(|argument| argument.len() > 4096) {
                return Err(
                    "A composition can have at most 64 launch arguments, each 4096 bytes or fewer."
                        .to_owned(),
                );
            }
            let index = composition_index(state, &composition_id)?;
            state.compositions.compositions[index].launch.arguments = arguments;
        }
        CompositionAction::AdoptVerifiedGraph { composition_id } => {
            let index = composition_index(state, &composition_id)?;
            let composition = &mut state.compositions.compositions[index];
            let InstallationState::OutOfDate {
                actual_graph,
                artifacts,
                verified: true,
                ..
            } = composition.installation.clone()
            else {
                return Err("Only a verified installed graph can be adopted.".to_owned());
            };
            composition.resolved_graph = Some(actual_graph.clone());
            composition.installation = InstallationState::Installed {
                graph_fingerprint: fingerprint_resolved_graph(&actual_graph),
                artifacts,
            };
        }
        CompositionAction::InvalidateContent {
            content_id,
            content_version,
        } => {
            let matches_content = |identity: &ContentReference| {
                identity.id == content_id
                    && content_version
                        .as_ref()
                        .is_none_or(|version| identity.version.as_ref() == Some(version))
            };
            for composition in &mut state.compositions.compositions {
                let affected = composition
                    .root_package
                    .as_ref()
                    .is_some_and(&matches_content)
                    || composition.resolved_graph.as_ref().is_some_and(|graph| {
                        graph
                            .nodes
                            .iter()
                            .any(|node| matches_content(&node.content))
                    });
                if affected {
                    composition.resolved_graph = None;
                    composition.installation = InstallationState::Unknown;
                }
            }
        }
        _ => return Err("This action starts a composition operation.".to_owned()),
    }
    Ok(())
}

fn composition_index(
    state: &crate::ClientStateDocument,
    composition_id: &str,
) -> Result<usize, String> {
    state
        .compositions
        .compositions
        .iter()
        .position(|composition| composition.id == composition_id)
        .ok_or_else(|| "The selected composition no longer exists.".to_owned())
}

fn next_composition_id(compositions: &[Composition]) -> String {
    (1_u64..)
        .map(|index| format!("composition-{index}"))
        .find(|candidate| {
            compositions
                .iter()
                .all(|composition| composition.id != *candidate)
        })
        .expect("composition ID space exhausted")
}

fn unique_composition_name(compositions: &[Composition], base: &str) -> String {
    if compositions
        .iter()
        .all(|composition| !composition.name.eq_ignore_ascii_case(base))
    {
        return base.to_owned();
    }
    (2_u64..)
        .map(|index| format!("{base} {index}"))
        .find(|candidate| {
            compositions
                .iter()
                .all(|composition| !composition.name.eq_ignore_ascii_case(candidate))
        })
        .expect("composition name space exhausted")
}
