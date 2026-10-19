use crate::CompositionDocument;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use vapor_content::VaporId;

/// Whether a cached catalog item can safely use automatic Library updates.
///
/// Auto-Install follows Workshop versions, so it requires a cached, unambiguous
/// runtime artifact with a Workshop identity. Packagepacks and Rust crates use
/// separate lifecycle workflows and are not eligible.
pub fn can_auto_install_library_item(
    kind: &str,
    has_workshop_item: bool,
    is_cached: bool,
    is_unambiguous: bool,
) -> bool {
    let Ok(kind) = kind.parse::<vapor_content::ContentKind>() else {
        return false;
    };
    has_workshop_item
        && is_cached
        && is_unambiguous
        && !matches!(
            kind,
            vapor_content::ContentKind::Packagepack | vapor_content::ContentKind::Crate
        )
}

/// Explicit user intent for automatic Library version following and collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum LibraryAction {
    SetAutoInstall {
        content_id: String,
        enabled: bool,
    },
    Install {
        selector: String,
        account: Option<String>,
        workspace_root: Option<String>,
        #[serde(default)]
        target_triples: Vec<String>,
    },
    Acquire {
        selectors: Vec<String>,
        workspace_root: Option<String>,
    },
    ValidateArtifacts {
        selector: Option<String>,
        workspace_root: Option<String>,
    },
    Update {
        selector: Option<String>,
        workspace_root: Option<String>,
    },
    Verify {
        selector: Option<String>,
    },
    Repair {
        selector: Option<String>,
        workspace_root: Option<String>,
    },
    Disable {
        selector: String,
    },
    Enable {
        selector: String,
    },
    Uninstall {
        selector: String,
    },
}

/// User-owned policy for one non-Packagepack Library item.
///
/// Workshop subscription and composition membership are separate facts. This
/// policy controls whether Vapor follows compatible updates and may collect a
/// local copy when no composition uses the item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LibraryItemPolicy {
    /// Stable content identity this preference applies to.
    pub content_id: VaporId,
    /// Follow newer versions that satisfy every active composition constraint,
    /// and permit collection when the item is no longer needed.
    pub auto_install: bool,
}

/// A composition edge that predates persisted dependency requirements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingLibraryConstraint {
    /// Composition whose resolved graph needs refreshing.
    pub composition_id: String,
    /// Binding whose accepted version range was not persisted.
    pub binding: String,
}

/// Collect every persisted requirement that constrains one Library item.
///
/// A matching legacy edge without its authored range is an error: automatic
/// updates must wait until that composition is resolved again rather than
/// guessing from its previously selected version.
pub fn library_constraints(
    compositions: &CompositionDocument,
    content_id: &VaporId,
) -> Result<Vec<VersionReq>, MissingLibraryConstraint> {
    let mut constraints = Vec::new();
    for composition in &compositions.compositions {
        let Some(graph) = &composition.resolved_graph else {
            continue;
        };
        for node in &graph.nodes {
            for dependency in &node.dependencies {
                if dependency.content.id != content_id.as_str() {
                    continue;
                }
                let Some(requirement) = dependency.version_requirement.as_deref() else {
                    return Err(MissingLibraryConstraint {
                        composition_id: composition.id.clone(),
                        binding: dependency.binding.clone(),
                    });
                };
                let requirement = requirement.parse().map_err(|_| MissingLibraryConstraint {
                    composition_id: composition.id.clone(),
                    binding: dependency.binding.clone(),
                })?;
                constraints.push(requirement);
            }
        }
    }
    Ok(constraints)
}

/// Find the first composition whose authored root or resolved graph uses a
/// Library item. Direct install-state changes must not strand a composition.
pub fn dependent_composition_id<'a>(
    compositions: &'a CompositionDocument,
    content_id: &VaporId,
) -> Option<&'a str> {
    let content_id = content_id.as_str();
    compositions.compositions.iter().find_map(|composition| {
        let root_matches = composition
            .root_package
            .as_ref()
            .is_some_and(|root| root.id == content_id);
        let graph_matches = composition.resolved_graph.as_ref().is_some_and(|graph| {
            graph.root.id == content_id
                || graph.effective_engine.id == content_id
                || graph
                    .effective_game
                    .as_ref()
                    .is_some_and(|game| game.id == content_id)
                || graph.nodes.iter().any(|node| {
                    node.content.id == content_id
                        || node
                            .dependencies
                            .iter()
                            .any(|dependency| dependency.content.id == content_id)
                })
        });
        (root_matches || graph_matches).then_some(composition.id.as_str())
    })
}

/// Select a version according to the item's Library policy and all active
/// composition constraints.
///
/// Auto-Install follows the highest available version accepted by every
/// constraint. A retained item stays on its current version when that version
/// remains compatible; changing it requires an explicit update operation.
pub fn select_library_version<'a>(
    policy: &LibraryItemPolicy,
    retained_version: Option<&'a Version>,
    available_versions: impl IntoIterator<Item = &'a Version>,
    constraints: &[VersionReq],
) -> Option<Version> {
    let satisfies_all = |version: &Version| constraints.iter().all(|req| req.matches(version));

    if policy.auto_install {
        available_versions
            .into_iter()
            .filter(|version| satisfies_all(version))
            .max()
            .cloned()
    } else {
        retained_version
            .filter(|version| satisfies_all(version))
            .cloned()
    }
}

/// Select a Library version using every composition's persisted constraints.
///
/// Returns a constraint error rather than selecting a potentially breaking
/// version when any dependent legacy graph lacks its authored range.
pub fn select_library_version_for_compositions<'a>(
    policy: &LibraryItemPolicy,
    retained_version: Option<&'a Version>,
    available_versions: impl IntoIterator<Item = &'a Version>,
    compositions: &CompositionDocument,
) -> Result<Option<Version>, MissingLibraryConstraint> {
    let constraints = library_constraints(compositions, &policy.content_id)?;
    Ok(select_library_version(
        policy,
        retained_version,
        available_versions,
        &constraints,
    ))
}

/// Whether an item may leave local storage after composition reachability is
/// recomputed.
pub const fn can_collect_library_item(
    policy: &LibraryItemPolicy,
    dependent_compositions: usize,
) -> bool {
    policy.auto_install && dependent_compositions == 0
}

/// Read the durable Auto-Install preference; absence means user-managed retention.
pub fn library_auto_install_enabled(policies: &[LibraryItemPolicy], content_id: &VaporId) -> bool {
    policies
        .iter()
        .find(|policy| &policy.content_id == content_id)
        .is_some_and(|policy| policy.auto_install)
}

/// Set the durable per-item preference, omitting the default retained policy.
pub fn set_library_auto_install(
    policies: &mut Vec<LibraryItemPolicy>,
    content_id: VaporId,
    enabled: bool,
) {
    policies.retain(|policy| policy.content_id != content_id);
    if enabled {
        policies.push(LibraryItemPolicy {
            content_id,
            auto_install: true,
        });
        policies.sort_by(|left, right| left.content_id.cmp(&right.content_id));
    }
}

#[cfg(test)]
mod tests {
    use super::{can_auto_install_library_item, dependent_composition_id};
    use crate::{
        Composition, CompositionDocument, ResolvedCompositionGraph, ResolvedDependencyIdentity,
        ResolvedNodeIdentity,
    };
    use vapor_content::ContentReference;

    #[test]
    fn automatic_updates_require_cached_unambiguous_workshop_runtime_content() {
        assert!(can_auto_install_library_item("game-mod", true, true, true));
        assert!(!can_auto_install_library_item(
            "packagepack",
            true,
            true,
            true
        ));
        assert!(!can_auto_install_library_item("crate", true, true, true));
        assert!(!can_auto_install_library_item(
            "game-mod", false, true, true
        ));
        assert!(!can_auto_install_library_item(
            "game-mod", true, false, true
        ));
        assert!(!can_auto_install_library_item(
            "game-mod", true, true, false
        ));
        assert!(!can_auto_install_library_item("unknown", true, true, true));
    }

    #[test]
    fn identifies_composition_root_as_a_library_dependency() {
        let mut composition = Composition::default_composition();
        composition.root_package = Some(ContentReference::parse("studio:demo", None).unwrap());
        let document = CompositionDocument {
            schema_version: 1,
            compositions: vec![composition],
        };

        let demo = "studio:demo".parse().unwrap();
        let other = "studio:other".parse().unwrap();
        assert_eq!(dependent_composition_id(&document, &demo), Some("default"));
        assert_eq!(dependent_composition_id(&document, &other), None);
    }

    #[test]
    fn identifies_resolved_transitive_dependency_as_library_dependency() {
        let mut composition = Composition::default_composition();
        composition.resolved_graph = Some(ResolvedCompositionGraph {
            root: ContentReference::parse("studio:root", Some("1.0.0".to_owned())).unwrap(),
            effective_engine: ContentReference::parse("studio:engine", Some("1.0.0".to_owned()))
                .unwrap(),
            effective_game: None,
            nodes: vec![ResolvedNodeIdentity {
                content: ContentReference::parse("studio:root", Some("1.0.0".to_owned())).unwrap(),
                kind: "packagepack".to_owned(),
                fingerprint: None,
                source: None,
                dependencies: vec![ResolvedDependencyIdentity {
                    binding: "engine".to_owned(),
                    content: ContentReference::parse("studio:engine", Some("1.0.0".to_owned()))
                        .unwrap(),
                    version_requirement: Some("^1".to_owned()),
                }],
            }],
        });
        let document = CompositionDocument {
            schema_version: 1,
            compositions: vec![composition],
        };

        let engine = "studio:engine".parse().unwrap();
        assert_eq!(
            dependent_composition_id(&document, &engine),
            Some("default")
        );
    }
}
