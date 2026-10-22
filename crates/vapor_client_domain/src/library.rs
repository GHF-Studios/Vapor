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

/// Auto-Install lifecycle policy for one non-Packagepack Library item.
///
/// Workshop subscription, Packagepack dependency ownership, and composition
/// membership are separate facts. Packagepacks may establish default owners;
/// an explicit user choice overrides that default while an owner exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LibraryItemPolicy {
    /// Stable content identity this preference applies to.
    pub content_id: VaporId,
    /// Follow newer versions that satisfy every active composition constraint,
    /// and permit collection when the item is no longer needed.
    pub auto_install: bool,
    /// Packagepacks whose dependency closure currently owns this item.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packagepack_owners: Vec<VaporId>,
    /// Whether the user explicitly chose the current Auto-Install value.
    #[serde(default)]
    pub user_selected: bool,
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

/// Whether an installed Packagepack composition still owns one content item.
///
/// Uninstalled composition drafts and previews do not retain auto-installed
/// Library entries. Installed and out-of-date compositions do, because their
/// exact content graph is still a local playable installation.
pub fn installed_composition_uses_content(
    compositions: &CompositionDocument,
    content_id: &VaporId,
) -> bool {
    compositions.compositions.iter().any(|composition| {
        let (artifacts, graph) = match &composition.installation {
            crate::InstallationState::Installed { artifacts, .. } => {
                (artifacts.as_slice(), composition.resolved_graph.as_ref())
            }
            crate::InstallationState::OutOfDate {
                actual_graph,
                artifacts,
                ..
            } => (artifacts.as_slice(), Some(actual_graph)),
            crate::InstallationState::NeedsRepair { artifacts, .. } => {
                (artifacts.as_slice(), composition.resolved_graph.as_ref())
            }
            crate::InstallationState::Unknown
            | crate::InstallationState::NotInstalled
            | crate::InstallationState::Partial { .. } => return false,
        };
        artifacts
            .iter()
            .any(|artifact| artifact.id == content_id.as_str())
            || graph.is_some_and(|graph| {
                graph.root.id == content_id.as_str()
                    || graph.effective_engine.id == content_id.as_str()
                    || graph
                        .effective_game
                        .as_ref()
                        .is_some_and(|game| game.id == content_id.as_str())
                    || graph.nodes.iter().any(|node| {
                        node.content.id == content_id.as_str()
                            || node
                                .dependencies
                                .iter()
                                .any(|dependency| dependency.content.id == content_id.as_str())
                    })
            })
    })
}

/// Packagepack roots that still have an installed local composition.
/// Drafts and previews do not own downloaded content; installed, out-of-date,
/// and repairable compositions do.
pub fn installed_packagepack_roots(compositions: &CompositionDocument) -> Vec<String> {
    compositions
        .compositions
        .iter()
        .filter(|composition| {
            matches!(
                composition.installation,
                crate::InstallationState::Installed { .. }
                    | crate::InstallationState::OutOfDate { .. }
                    | crate::InstallationState::NeedsRepair { .. }
            )
        })
        .filter_map(|composition| {
            composition
                .root_package
                .as_ref()
                .map(|root| root.id.clone())
        })
        .collect()
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

/// Add a Packagepack as an automatic lifecycle owner for one dependency.
/// An existing user choice is preserved.
pub fn add_auto_install_owner(
    policies: &mut Vec<LibraryItemPolicy>,
    content_id: VaporId,
    packagepack_id: VaporId,
) {
    let policy = policy_mut_or_insert(policies, content_id);
    if !policy.packagepack_owners.contains(&packagepack_id) {
        policy.packagepack_owners.push(packagepack_id);
        policy.packagepack_owners.sort();
    }
    if !policy.user_selected {
        policy.auto_install = true;
    }
}

/// Remove a Packagepack ownership edge after its subscription or installation ends.
pub fn remove_auto_install_owner(policies: &mut Vec<LibraryItemPolicy>, packagepack_id: &VaporId) {
    policies.retain_mut(|policy| {
        policy
            .packagepack_owners
            .retain(|owner| owner != packagepack_id);
        if !policy.packagepack_owners.is_empty() {
            return true;
        }
        policy.auto_install || policy.user_selected
    });
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
    let policy = policy_mut_or_insert(policies, content_id);
    policy.auto_install = enabled;
    policy.user_selected = true;
}

fn policy_mut_or_insert(
    policies: &mut Vec<LibraryItemPolicy>,
    content_id: VaporId,
) -> &mut LibraryItemPolicy {
    if let Some(index) = policies
        .iter()
        .position(|policy| policy.content_id == content_id)
    {
        return &mut policies[index];
    }
    policies.push(LibraryItemPolicy {
        content_id: content_id.clone(),
        auto_install: false,
        packagepack_owners: Vec::new(),
        user_selected: false,
    });
    policies.sort_by(|left, right| left.content_id.cmp(&right.content_id));
    let index = policies
        .iter()
        .position(|policy| policy.content_id == content_id)
        .expect("inserted Library policy is present");
    &mut policies[index]
}

#[cfg(test)]
mod tests {
    use super::{
        add_auto_install_owner, can_auto_install_library_item, dependent_composition_id,
        installed_composition_uses_content, installed_packagepack_roots, remove_auto_install_owner,
        set_library_auto_install,
    };
    use crate::{
        Composition, CompositionDocument, InstallationState, InstalledArtifactRecord,
        ResolvedCompositionGraph, ResolvedDependencyIdentity, ResolvedNodeIdentity,
    };
    use std::path::PathBuf;
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
    fn packagepack_defaults_are_owned_and_user_opt_out_survives_reacquisition() {
        let mut policies = Vec::new();
        let game: vapor_content::VaporId = "studio/game/main".parse().unwrap();
        let pack: vapor_content::VaporId = "studio/pack/main".parse().unwrap();

        add_auto_install_owner(&mut policies, game.clone(), pack.clone());
        assert!(policies[0].auto_install);
        assert_eq!(
            policies[0].packagepack_owners.as_slice(),
            std::slice::from_ref(&pack)
        );

        set_library_auto_install(&mut policies, game.clone(), false);
        remove_auto_install_owner(&mut policies, &pack);
        add_auto_install_owner(&mut policies, game, pack.clone());

        assert!(!policies[0].auto_install);
        assert!(policies[0].user_selected);
        assert_eq!(policies[0].packagepack_owners, [pack]);
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

    #[test]
    fn auto_install_dependency_is_owned_only_while_a_packagepack_is_installed() {
        let dependency: vapor_content::VaporId = "studio:engine".parse().unwrap();
        let mut composition = Composition::default_composition();
        composition.root_package = Some(ContentReference::parse("studio:pack", None).unwrap());
        composition.resolved_graph = Some(ResolvedCompositionGraph {
            root: ContentReference::parse("studio:pack", Some("1.0.0".to_owned())).unwrap(),
            effective_engine: ContentReference::parse("studio:engine", Some("1.0.0".to_owned()))
                .unwrap(),
            effective_game: None,
            nodes: vec![ResolvedNodeIdentity {
                content: ContentReference::parse("studio:pack", Some("1.0.0".to_owned())).unwrap(),
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
            compositions: vec![composition.clone()],
        };

        assert!(!installed_composition_uses_content(&document, &dependency));

        composition.installation = InstallationState::Installed {
            artifacts: vec![InstalledArtifactRecord {
                id: dependency.to_string(),
                version: Some("1.0.0".to_owned()),
                root: PathBuf::from("/installed/engine"),
                fingerprint: "sha256:engine".to_owned(),
            }],
            graph_fingerprint: Some("sha256:graph".to_owned()),
        };
        let document = CompositionDocument {
            schema_version: 1,
            compositions: vec![composition],
        };
        assert!(installed_composition_uses_content(&document, &dependency));
    }

    #[test]
    fn packagepack_root_ownership_ignores_uninstalled_composition_drafts() {
        let mut composition = Composition::default_composition();
        composition.root_package = Some(ContentReference::parse("studio:pack", None).unwrap());
        let document = CompositionDocument {
            schema_version: 1,
            compositions: vec![composition.clone()],
        };
        assert!(installed_packagepack_roots(&document).is_empty());

        composition.installation = InstallationState::Installed {
            artifacts: Vec::new(),
            graph_fingerprint: Some("sha256:graph".to_owned()),
        };
        let document = CompositionDocument {
            schema_version: 1,
            compositions: vec![composition],
        };
        assert_eq!(installed_packagepack_roots(&document), ["studio:pack"]);
    }
}
