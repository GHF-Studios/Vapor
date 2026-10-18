use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use vapor_content::VaporId;
use crate::CompositionDocument;

/// Explicit user intent for automatic Library version following and collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum LibraryAction {
    SetAutoInstall { content_id: String, enabled: bool },
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
pub fn library_constraints<'a>(
    compositions: &'a CompositionDocument,
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
