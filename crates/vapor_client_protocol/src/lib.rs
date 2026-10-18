//! Stable, typed messages exchanged by Vapor Client frontends and its service.
//!
//! This crate projects shared domain contracts into transport-safe messages.
//! It has no process, UI, filesystem, or provider dependencies, so both sides
//! can evolve behind an explicit protocol version.

mod action;
mod catalog;
mod composition;
mod message;
mod state;

pub use catalog::{CatalogQuery, CatalogSearchSnapshot, CatalogStatus};

pub use action::{
    AuthoringAction, CatalogAction, ClientAction, CompositionAction, CompositionDependency,
    DeveloperAction, LibraryAction, PublishingAction, SessionAction, WorkshopAction,
    WorkshopMetadataDraft, WorkspaceAction,
};
pub use composition::{
    COMPOSITION_SCHEMA_VERSION, Composition, CompositionDocument, ContentReference,
    InstallationState, InstalledArtifactRecord, LaunchSettings, LegacyStorageSeed,
    ResolvedCompositionGraph, ResolvedDependencyIdentity, ResolvedNodeIdentity,
    WorkspaceAssociation,
};
pub use vapor_client_domain::{ClientStateDocument, LibraryItemPolicy};

pub use message::{
    ClientRequest, ClientResponse, ClientResponseKind, PROTOCOL_VERSION, ProtocolFrame, RequestId,
    ServiceError, ServiceErrorCode,
};
pub use state::{
    AuthoringCompositionLink, AuthoringResult, AuthoringResultKind, AvailableAction,
    CatalogConflict, CatalogEntry, CatalogItemAction, CatalogItemRole, CatalogLocation, CatalogLocationKind, ClientEvent,
    ClientRevision, ClientSnapshot, CompositionPreviewDependency, CompositionPreviewNode,
    CompositionPreviewSnapshot, CompositionPreviewSource, DurableStateSnapshot, HostSummary,
    OperationEvent, OperationPhase, OperationStatus, OperationSummary, RootPublishingSnapshot,
    AvailableSuperworkspaceRepositorySnapshot, SuperworkspaceRepositorySnapshot,
    SuperworkspaceSnapshot,
    WorkshopAcquisitionBatchSnapshot, WorkshopAcquisitionSnapshot, WorkshopItemSnapshot,
    WorkshopPackagePreviewSnapshot, WorkshopReviewSnapshot,
    WorkshopSearchSnapshot, WorkshopSnapshot, WorkshopSubscriptionSnapshot,
    WorkspaceContentSnapshot, WorkspaceFileResult, WorkspaceProjectCandidate,
    WorkspaceSourceProject, WorkspaceWorkshopPolicy,
};

/// Maximum serialized service protocol frame, including no line terminator.
pub const MAX_PROTOCOL_FRAME_BYTES: usize = 32 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_round_trip<T>(value: T)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug + PartialEq,
    {
        let encoded = serde_json::to_string(&value).expect("encode protocol value");
        let decoded: T = serde_json::from_str(&encoded).expect("decode protocol value");
        assert_eq!(decoded, value);
    }

    #[test]
    fn every_semantic_action_round_trips() {
        let actions = vec![
            ClientAction::Composition(CompositionAction::Create {
                name: "Test".into(),
            }),
            ClientAction::Composition(CompositionAction::Duplicate {
                composition_id: "p1".into(),
                name: "Copy".into(),
            }),
            ClientAction::Composition(CompositionAction::Rename {
                composition_id: "p1".into(),
                name: "Renamed".into(),
            }),
            ClientAction::Composition(CompositionAction::Remove {
                composition_id: "p1".into(),
            }),
            ClientAction::Composition(CompositionAction::Select {
                composition_id: "p1".into(),
            }),
            ClientAction::Composition(CompositionAction::SetRoot {
                composition_id: "p1".into(),
                content_id: Some("root".into()),
                content_version: Some("1.0.0".into()),
                workspace_root: Some("/workspace".into()),
            }),
            ClientAction::Composition(CompositionAction::ResolveDefaultRoot {
                composition_id: "p1".into(),
            }),
            ClientAction::Composition(CompositionAction::SetLaunchArguments {
                composition_id: "p1".into(),
                arguments: vec!["--safe".into()],
            }),
            ClientAction::Composition(CompositionAction::AdoptVerifiedGraph {
                composition_id: "p1".into(),
            }),
            ClientAction::Composition(CompositionAction::InvalidateContent {
                content_id: "root".into(),
                content_version: Some("1.0.0".into()),
            }),
            ClientAction::Session(SessionAction::CheckpointLocalState {
                editor_drafts: Some("{}".into()),
                operation_history: Some("[]".into()),
            }),
            ClientAction::Catalog(CatalogAction::Refresh),
            ClientAction::Catalog(CatalogAction::DiscoverContentProjects),
            ClientAction::Catalog(CatalogAction::Search {
                query: "moon".into(),
            }),
            ClientAction::Catalog(CatalogAction::Inspect {
                content_id: "moon".into(),
                version: Some("1.0.0".into()),
            }),
            ClientAction::Catalog(CatalogAction::AdoptWorkspaceProject {
                path: "mods/moon".into(),
            }),
            ClientAction::Composition(CompositionAction::Preview {
                composition_id: "p1".into(),
                selector: Some("pack".into()),
            }),
            ClientAction::Composition(CompositionAction::Install {
                composition_id: "p1".into(),
            }),
            ClientAction::Composition(CompositionAction::Verify {
                composition_id: "p1".into(),
            }),
            ClientAction::Composition(CompositionAction::Repair {
                composition_id: "p1".into(),
            }),
            ClientAction::Composition(CompositionAction::Update {
                composition_id: "p1".into(),
            }),
            ClientAction::Composition(CompositionAction::Launch {
                composition_id: "p1".into(),
            }),
            ClientAction::Composition(CompositionAction::RunSource {
                composition_id: "p1".into(),
            }),
            ClientAction::Workspace(WorkspaceAction::Discover { root: ".".into() }),
            ClientAction::Workspace(WorkspaceAction::DiscoverWorkspace { root: ".".into() }),
            ClientAction::Workspace(WorkspaceAction::Open { root: ".".into() }),
            ClientAction::Workspace(WorkspaceAction::Validate),
            ClientAction::Workspace(WorkspaceAction::Remember),
            ClientAction::Workspace(WorkspaceAction::Create {
                root: "/tmp".into(),
                name: "Demo".into(),
            }),
            ClientAction::Workspace(WorkspaceAction::AdoptProject {
                path: "mods/moon".into(),
            }),
            ClientAction::Workspace(WorkspaceAction::UnregisterProject {
                project_id: "moon".into(),
            }),
            ClientAction::Workspace(WorkspaceAction::ListFiles { path: ".".into() }),
            ClientAction::Workspace(WorkspaceAction::OpenFile {
                path: "Vapor.toml".into(),
            }),
            ClientAction::Workspace(WorkspaceAction::SaveFile {
                path: "Vapor.toml".into(),
                expected_fingerprint: "abc".into(),
                contents: "[project]".into(),
            }),
            ClientAction::Workspace(WorkspaceAction::CreateFile {
                path: "new.toml".into(),
                contents: String::new(),
            }),
            ClientAction::Authoring(AuthoringAction::LoadComposition {
                project_id: "moon".into(),
            }),
            ClientAction::Authoring(AuthoringAction::ListRegisteredSources),
            ClientAction::Authoring(AuthoringAction::SaveComposition {
                project_id: "moon".into(),
                expected_fingerprint: "abc".into(),
                dependencies: vec![],
            }),
            ClientAction::Authoring(AuthoringAction::LoadWorkshopMetadata {
                project_id: "moon".into(),
            }),
            ClientAction::Authoring(AuthoringAction::SaveWorkshopMetadata {
                project_id: "moon".into(),
                expected_fingerprint: "abc".into(),
                app_id: Some(1),
                published_file_id: Some("123".into()),
                visibility: Some("public".into()),
                title: Some("Moon".into()),
                description: Some("A moon".into()),
                tags: vec![],
                change_note: None,
            }),
            ClientAction::Authoring(AuthoringAction::LoadRuntimeOutputs {
                project_id: "moon".into(),
            }),
            ClientAction::Authoring(AuthoringAction::ValidateContent {
                project_id: "moon".into(),
            }),
            ClientAction::Authoring(AuthoringAction::SaveRuntimeOutputs {
                project_id: "moon".into(),
                expected_fingerprint: "abc".into(),
                binaries: vec![],
                libraries: vec![],
            }),
            ClientAction::Authoring(AuthoringAction::CreateContent {
                workspace_root: ".".into(),
                kind: "mod".into(),
                id: "moon".into(),
                version: "1.0.0".into(),
                dependencies: vec!["engine@1".into()],
                optional_dependencies: vec![],
                workshop: WorkshopMetadataDraft::default(),
            }),
            ClientAction::Authoring(AuthoringAction::CreateWorkspace {
                parent: "/tmp".into(),
                organization: "ghf".into(),
                name: "Demo".into(),
            }),
            ClientAction::Workshop(WorkshopAction::Search {
                query: "moon".into(),
                page: 1,
            }),
            ClientAction::Workshop(WorkshopAction::LoadSubscriptions),
            ClientAction::Workshop(WorkshopAction::Subscribe {
                item_ids: vec!["123".into()],
            }),
            ClientAction::Workshop(WorkshopAction::Unsubscribe {
                item_ids: vec!["123".into()],
            }),
            ClientAction::Workshop(WorkshopAction::Acquire {
                item_ids: vec!["123".into()],
            }),
            ClientAction::Workshop(WorkshopAction::Stage {
                selector: "moon@1.0.0".into(),
            }),
            ClientAction::Workshop(WorkshopAction::Publish {
                review_id: "review-1".into(),
                account: "publisher".into(),
                confirmation: "confirm".into(),
                change_note: "Updated content".into(),
            }),
            ClientAction::Workshop(WorkshopAction::DiscardReview),
            ClientAction::Publishing(PublishingAction::StageRoot {
                account: "publisher".into(),
                description: "release".into(),
                set_live: None,
            }),
            ClientAction::Publishing(PublishingAction::PublishRoot {
                review_id: "root-review".into(),
                confirmation: "confirm".into(),
            }),
            ClientAction::Publishing(PublishingAction::DiscardRootReview),
            ClientAction::Developer(DeveloperAction::Format),
            ClientAction::Developer(DeveloperAction::Check),
            ClientAction::Developer(DeveloperAction::Test),
            ClientAction::Developer(DeveloperAction::Build),
            ClientAction::Developer(DeveloperAction::Clean {
                confirmation: "clean-1".into(),
            }),
        ];
        assert_eq!(
            actions.len(),
            61,
            "update this catalog when adding/removing action variants"
        );
        for action in actions {
            assert_round_trip(action);
        }
    }

    #[test]
    fn composition_management_uses_the_composition_action_family() {
        let action = ClientAction::Composition(CompositionAction::Create {
            name: "New composition".into(),
        });
        assert_eq!(action.id(), "composition.create");
        let encoded = serde_json::to_value(action).expect("encode composition action");
        assert_eq!(encoded["family"], "composition");
        assert_eq!(encoded["action"]["action"], "create");
    }

    #[test]
    fn request_event_snapshot_and_error_round_trip() {
        assert_round_trip(ClientRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: RequestId("request-1".into()),
            expected_revision: Some(ClientRevision(3)),
            action: ClientAction::Catalog(CatalogAction::Refresh),
        });
        assert_round_trip(ClientEvent::WorkspaceFileResult(Box::new(
            WorkspaceFileResult::Opened {
                operation_id: "operation-1".into(),
                path: "Vapor.toml".into(),
                contents: "[workspace]".into(),
                fingerprint: "fnv1a64:abc:11".into(),
            },
        )));
        assert_round_trip(ClientEvent::WorkspaceFileResult(Box::new(
            WorkspaceFileResult::RememberedSources {
                operation_id: "operation-3".into(),
                roots: vec!["/workspace".into()],
            },
        )));
        assert_round_trip(ClientEvent::WorkspaceFileResult(Box::new(
            WorkspaceFileResult::UnregisteredContent {
                operation_id: "operation-4".into(),
                content_id: "moon".into(),
                version: Some("1.0.0".into()),
            },
        )));
        assert_round_trip(ClientEvent::AuthoringResult(Box::new(AuthoringResult {
            operation_id: "operation-2".into(),
            result: AuthoringResultKind::WorkshopMetadata {
                id: "moon".into(),
                version: Some("1.0.0".into()),
                kind: "game".into(),
                manifest: "game/Vapor.toml".into(),
                source: "[content]".into(),
                policy: WorkshopMetadataDraft {
                    app_id: Some(1),
                    published_file_id: Some("123".into()),
                    visibility: Some("public".into()),
                    title: Some("Moon".into()),
                    description: Some("A moon".into()),
                    tags: vec!["planet".into()],
                    change_note: None,
                },
            },
        })));
        assert_round_trip(ClientEvent::Snapshot(Box::new(ClientSnapshot {
            revision: ClientRevision(3),
            host: HostSummary {
                app_root: Some("/app".into()),
                workspace: None,
                mode: "production".into(),
                platform: "linux".into(),
                steam_client_available: true,
            },
            active_composition_id: None,
            durable_state: DurableStateSnapshot::default(),
            active_operation: Some(OperationSummary {
                id: "operation-1".into(),
                composition_id: Some("p1".into()),
                action: "catalog.refresh".into(),
                phase: OperationPhase::Resolving,
                status: OperationStatus::Running,
                progress: None,
            }),
            composition_preview: None,
            catalog: vec![CatalogEntry {
                id: "moon".into(),
                version: Some("1.0.0".into()),
                kind: "mod".into(),
                published_file_id: None,
                dependencies: vec![],
                conflicts: vec![],
                locations: vec![CatalogLocation {
                    kind: CatalogLocationKind::Workspace,
                    root: "mods/moon".into(),
                    enabled: true,
                    fingerprint: None,
                    install_source: None,
                }],
                role: CatalogItemRole::Other,
                actions: vec![],
            }],
            catalog_status: CatalogStatus::default(),
            catalog_search: CatalogSearchSnapshot::default(),
            workspace_content: WorkspaceContentSnapshot {
                source_projects: vec![WorkspaceSourceProject {
                    id: "loo-cast.game".into(),
                    version: Some("1.0.0".into()),
                    kind: "game".into(),
                    root: "game".into(),
                    manifest: "game/Vapor.toml".into(),
                    binaries: vec!["loo-cast-game".into()],
                    libraries: vec![],
                    dependencies: vec!["required · spacetime-engine".into()],
                    workshop: WorkspaceWorkshopPolicy {
                        app_id: Some(2122620),
                        published_file_id: Some("3762162407".into()),
                        visibility: Some("public".into()),
                        title: Some("Loo-Cast Game".into()),
                        description: Some("Game content".into()),
                        tags: vec!["game".into()],
                        change_note: None,
                    },
                }],
                source_projects_loaded: true,
                source_projects_error: None,
                unregistered_projects: vec![WorkspaceProjectCandidate {
                    path: "mods/terrain".into(),
                    id: "mod-terrain".into(),
                    kind: "game-mod".into(),
                }],
                unregistered_diagnostics: vec!["ignored invalid project".into()],
                unregistered_projects_loaded: true,
                unregistered_projects_error: None,
                workspace_validation_error: Some("metadata needs repair".into()),
            },
            superworkspace: SuperworkspaceSnapshot::default(),
            root_publishing: None,
            workshop: WorkshopSnapshot {
                search: Some(WorkshopSearchSnapshot {
                    revision: 4,
                    query: "moon".into(),
                    page: 2,
                    total_results: 3,
                    items: vec![WorkshopItemSnapshot {
                        published_file_id: "1002".into(),
                        title: "Fixture Moon Pack".into(),
                        description: "Fixture content".into(),
                        preview_url: None,
                        updated_at_unix: 1_797_000_000,
                    }],
                }),
                subscriptions: vec![WorkshopSubscriptionSnapshot {
                    published_file_id: "1002".into(),
                    title: "Fixture Moon Pack".into(),
                    description: "Fixture content".into(),
                    preview_url: None,
                    updated_at_unix: 1_797_000_000,
                    installed: false,
                    needs_update: false,
                    downloading: false,
                    download_progress: None,
                }],
                subscriptions_loaded: true,
                subscriptions_revision: 2,
                acquisition: Some(WorkshopAcquisitionBatchSnapshot {
                    revision: 3,
                    requested: 1,
                    items: vec![WorkshopAcquisitionSnapshot {
                        id: "moon.pack".into(),
                        version: Some("1.0.0".into()),
                        kind: "packagepack".into(),
                        cache_root: "/cache/moon.pack".into(),
                        fingerprint: "sha256:abc".into(),
                    }],
                }),
                review: Some(WorkshopReviewSnapshot {
                    review_id: "review-1".into(),
                    selector: "moon.pack@1.0.0".into(),
                    preview: WorkshopPackagePreviewSnapshot {
                        id: "moon.pack".into(),
                        version: Some("1.0.0".into()),
                        app_id: Some(2122620),
                        published_file_id: None,
                        title: Some("Moon Pack".into()),
                        visibility: Some("public".into()),
                        dependencies: vec!["spacetime-engine >=1.0.0".into()],
                        root: "/stage/moon.pack".into(),
                        target: "x86_64-unknown-linux-gnu".into(),
                        fingerprint: "sha256:def".into(),
                        files: 3,
                        bytes: 1024,
                        file_paths: vec!["Vapor.toml".into()],
                        omitted_file_paths: 2,
                    },
                }),
            },
            selected_content_id: None,
            available_actions: vec![AvailableAction {
                id: "catalog.refresh".into(),
                enabled: true,
                reason: None,
            }],
        })));
        assert_round_trip(ClientEvent::OperationChanged(OperationEvent {
            revision: ClientRevision(4),
            id: "operation-1".into(),
            composition_id: Some("p1".into()),
            action: "catalog.refresh".into(),
            phase: OperationPhase::Complete,
            status: OperationStatus::Succeeded,
            progress: Some(1.0),
            message: Some("Done".into()),
        }));
        assert_round_trip(ServiceError {
            code: ServiceErrorCode::StaleRevision,
            message: "state changed".into(),
            retryable: true,
        });
        assert_round_trip(ProtocolFrame::Event(ClientEvent::ServiceReady {
            protocol_version: PROTOCOL_VERSION,
        }));
    }

    #[test]
    fn snapshots_from_the_previous_protocol_shape_default_workspace_discovery() {
        let prior_snapshot = r#"{
            "revision": 0,
            "host": {"app_root": null, "workspace": null, "mode": "production"},
            "active_composition_id": null,
            "active_operation": null,
            "catalog": [],
            "selected_content_id": null,
            "available_actions": []
        }"#;

        let snapshot: ClientSnapshot = serde_json::from_str(prior_snapshot)
            .expect("older snapshot omitting additive workspace state remains readable");
        assert_eq!(
            snapshot.workspace_content,
            WorkspaceContentSnapshot::default()
        );
        assert_eq!(snapshot.workshop, WorkshopSnapshot::default());
    }

    #[test]
    fn composition_runtime_installation_state_round_trips_over_the_protocol() {
        let mut composition = Composition::default_composition();
        composition.installation = InstallationState::NotInstalled;
        assert_round_trip(CompositionDocument {
            schema_version: COMPOSITION_SCHEMA_VERSION,
            compositions: vec![composition],
        });
    }

    #[test]
    fn composition_wire_payloads_use_composition_names() {
        let document = serde_json::to_value(CompositionDocument::default())
            .expect("encode composition document");
        assert!(document.get("compositions").is_some());
        assert!(document.get("profiles").is_none());

        let action = serde_json::to_value(ClientAction::Composition(CompositionAction::Select {
            composition_id: "default".into(),
        }))
        .expect("encode composition action");
        assert_eq!(action["action"]["composition_id"], "default");
        assert!(action["action"].get("profile_id").is_none());
    }
}
