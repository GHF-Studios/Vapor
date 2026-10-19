use serde::Deserialize;
use std::path::PathBuf;
pub use vapor_client_domain::{
    CURRENT_SCHEMA_VERSION, Composition, CompositionDocument, ContentReference, InstallationState,
    InstalledArtifactRecord, LaunchSettings, ResolvedCompositionGraph, ResolvedDependencyIdentity,
    ResolvedNodeIdentity, WorkspaceAssociation, add_composition, duplicate_composition,
    fingerprint_resolved_graph, remove_composition, rename_composition, resolved_graph_matches,
};

#[cfg(test)]
mod resolved_graph_tests {
    use super::{
        ContentReference, InstallationState, ResolvedCompositionGraph, ResolvedDependencyIdentity,
        ResolvedNodeIdentity, resolved_graph_matches,
    };

    fn identity(id: &str, version: &str) -> ContentReference {
        ContentReference {
            id: id.to_owned(),
            version: Some(version.to_owned()),
        }
    }

    fn graph(engine_fingerprint: &str) -> ResolvedCompositionGraph {
        ResolvedCompositionGraph {
            root: identity("org.game", "1.0.0"),
            effective_engine: identity("org.engine", "2.0.0"),
            effective_game: Some(identity("org.gameplay", "3.0.0")),
            nodes: vec![
                ResolvedNodeIdentity {
                    content: identity("org.engine", "2.0.0"),
                    kind: "Engine".to_owned(),
                    fingerprint: Some(engine_fingerprint.to_owned()),
                    source: Some("workshop:42".to_owned()),
                    dependencies: Vec::new(),
                },
                ResolvedNodeIdentity {
                    content: identity("org.game", "1.0.0"),
                    kind: "Packagepack".to_owned(),
                    fingerprint: Some("sha256:root".to_owned()),
                    source: Some("workspace:/game".to_owned()),
                    dependencies: vec![
                        ResolvedDependencyIdentity {
                            binding: "engine".to_owned(),
                            content: identity("org.engine", "2.0.0"),
                            version_requirement: None,
                        },
                        ResolvedDependencyIdentity {
                            binding: "game".to_owned(),
                            content: identity("org.gameplay", "3.0.0"),
                            version_requirement: None,
                        },
                    ],
                },
                ResolvedNodeIdentity {
                    content: identity("org.gameplay", "3.0.0"),
                    kind: "Game".to_owned(),
                    fingerprint: Some("sha256:game".to_owned()),
                    source: Some("cache".to_owned()),
                    dependencies: Vec::new(),
                },
            ],
        }
    }

    #[test]
    fn resolved_graph_match_ignores_record_order_but_checks_payload_and_edges() {
        let expected = graph("sha256:engine");
        let mut reordered = expected.clone();
        reordered.nodes.reverse();
        reordered.nodes[1].dependencies.reverse();
        assert!(resolved_graph_matches(&expected, &reordered));

        let changed_payload = graph("sha256:changed");
        assert!(!resolved_graph_matches(&expected, &changed_payload));

        let mut changed_source = expected.clone();
        changed_source.nodes[0].source = Some("cache".to_owned());
        assert!(!resolved_graph_matches(&expected, &changed_source));

        let mut changed_engine_version = expected.clone();
        changed_engine_version.effective_engine.version = Some("2.0.1".to_owned());
        assert!(!resolved_graph_matches(&expected, &changed_engine_version));

        let mut changed_edges = expected.clone();
        changed_edges.nodes[1].dependencies[1].content = identity("org.other-game", "3.0.0");
        assert!(!resolved_graph_matches(&expected, &changed_edges));
        assert!(!resolved_graph_matches(&changed_edges, &changed_edges));

        let mut duplicate_binding = expected.clone();
        duplicate_binding.nodes[1].dependencies[1].binding = "engine".to_owned();
        assert!(!resolved_graph_matches(
            &duplicate_binding,
            &duplicate_binding
        ));
    }

    #[test]
    fn verified_installation_state_survives_protocol_serialization() {
        let state = InstallationState::OutOfDate {
            detail: "graph changed".to_owned(),
            actual_graph: graph("sha256:engine"),
            artifacts: Vec::new(),
            verified: true,
        };
        let encoded = serde_json::to_string(&state).unwrap();
        assert!(encoded.contains("\"verified\":true"));
        let decoded: InstallationState = serde_json::from_str(&encoded).unwrap();
        assert!(matches!(
            decoded,
            InstallationState::OutOfDate { verified: true, .. }
        ));
    }

    #[test]
    fn legacy_graph_without_provenance_can_be_upgraded_after_identity_match() {
        let actual = graph("sha256:engine");
        let mut legacy = actual.clone();
        for node in &mut legacy.nodes {
            node.fingerprint = None;
            node.source = None;
        }
        assert!(resolved_graph_matches(&legacy, &actual));

        legacy.nodes[0]
            .dependencies
            .push(ResolvedDependencyIdentity {
                binding: "asset".to_owned(),
                content: identity("org.asset", "1.0.0"),
                version_requirement: None,
            });
        assert!(!resolved_graph_matches(&legacy, &actual));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionRecovery {
    pub message: String,
}

/// Load the versioned document, or migrate the former eframe list shape.
/// Malformed values recover to a usable default composition with a diagnostic.
pub fn decode_compositions(
    value: Option<&str>,
) -> (CompositionDocument, Option<CompositionRecovery>) {
    let Some(value) = value else {
        return (CompositionDocument::default(), None);
    };
    if let Ok(document) = serde_json::from_str::<CompositionDocument>(value) {
        if document.schema_version == CURRENT_SCHEMA_VERSION && !document.compositions.is_empty() {
            return (document, None);
        }
        return (
            CompositionDocument::default(),
            Some(CompositionRecovery {
                message: format!(
                    "Saved composition schema {} is unsupported or empty; a default composition was opened. The saved value was left untouched in eframe storage.",
                    document.schema_version
                ),
            }),
        );
    }
    #[derive(Deserialize)]
    struct LegacyProfile {
        name: String,
        #[serde(default)]
        selected_content: Option<String>,
        #[serde(default)]
        workspace_root: Option<PathBuf>,
    }
    if let Ok(legacy) = serde_json::from_str::<Vec<LegacyProfile>>(value)
        && !legacy.is_empty()
    {
        let profiles = legacy
            .into_iter()
            .enumerate()
            .map(|(index, old)| Composition {
                id: format!("profile-{}", index + 1),
                name: old.name,
                root_package: old
                    .selected_content
                    .map(|id| ContentReference { id, version: None }),
                resolved_graph: None,
                workspace: old.workspace_root.map(|root| WorkspaceAssociation { root }),
                launch: LaunchSettings::default(),
                installation: InstallationState::Unknown,
            })
            .collect();
        return (CompositionDocument { schema_version: CURRENT_SCHEMA_VERSION, compositions: profiles }, Some(CompositionRecovery {
            message: "Migrated existing compositions. Their selected content IDs were retained; exact versions and installation state will be resolved from the content source.".to_owned(),
        }));
    }
    (CompositionDocument::default(), Some(CompositionRecovery {
        message: "Saved compositions could not be read; a default composition was opened. The saved value was left untouched in eframe storage.".to_owned(),
    }))
}
