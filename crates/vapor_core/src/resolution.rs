//! Resolved Vapor composition data.
//!
//! This module initially models resolver output only. The resolver algorithm
//! itself comes later.

use crate::{ContentKind, ResolvedContentId};
use std::collections::BTreeMap;

/// One exact resolved Vapor Content definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedContentNode {
    pub identity: ResolvedContentId,
    pub kind: ContentKind,

    /// Local dependency binding -> exact resolved content identity.
    pub dependencies: BTreeMap<String, ResolvedContentId>,
}

/// Exact dependency graph produced from a Packagepack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedComposition {
    pub root: ResolvedContentId,
    pub nodes: BTreeMap<ResolvedContentId, ResolvedContentNode>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VaporId;
    use semver::Version;

    #[test]
    fn graph_can_contain_multiple_versions_of_one_vapor_id() {
        let id: VaporId = "ghf-studios/example/physics".parse().unwrap();

        let v1 = ResolvedContentId {
            id: id.clone(),
            version: Version::parse("1.9.0").unwrap(),
        };

        let v2 = ResolvedContentId {
            id,
            version: Version::parse("2.3.0").unwrap(),
        };

        let mut nodes = BTreeMap::new();

        nodes.insert(
            v1.clone(),
            ResolvedContentNode {
                identity: v1.clone(),
                kind: ContentKind::ExtensionMod,
                dependencies: BTreeMap::new(),
            },
        );

        nodes.insert(
            v2.clone(),
            ResolvedContentNode {
                identity: v2.clone(),
                kind: ContentKind::ExtensionMod,
                dependencies: BTreeMap::new(),
            },
        );

        let composition = ResolvedComposition { root: v1, nodes };

        assert_eq!(composition.nodes.len(), 2);
        assert!(composition.nodes.contains_key(&v2));
    }
}
