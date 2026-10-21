use std::{fs, path::PathBuf, time::Duration};
use vapor_client::{Client, ClientHostConfig};
use vapor_client_protocol::{
    CatalogAction, ClientAction, ClientEvent, ClientResponseKind, CompositionAction,
    CompositionDocument, LegacyStorageSeed, ProtocolFrame, WorkshopAction, WorkspaceAction,
    WorkspaceFileResult, WorkspaceProjectSelection, WorkspaceToolOperation,
};
use vapor_client_service::run_simulator;

fn service_executable() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_vapor-client-service"))
}

fn fixture_client() -> Client {
    let executable = service_executable();
    Client::spawn(
        &executable,
        ClientHostConfig {
            application_executable: executable.clone(),
            workshop_app_id: Some(2122620),
            workspace_root: None,
            active_composition_id: Some("default".to_owned()),
            fixture_mode: true,
            legacy_storage: vapor_client_protocol::LegacyStorageSeed::default(),
            state_directory: None,
        },
    )
    .expect("fixture service starts")
}

#[test]
fn child_process_returns_deterministic_catalog_snapshot_and_shutdowns() {
    let mut client = fixture_client();
    assert_eq!(client.latest_snapshot().host.mode, "fixture");
    assert_eq!(
        client.latest_snapshot().active_composition_id.as_deref(),
        Some("default")
    );
    let revision = client.latest_snapshot().revision;
    client
        .send(
            ClientAction::Catalog(CatalogAction::Refresh),
            Some(revision),
        )
        .expect("send refresh");
    let response = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive response");
    let ProtocolFrame::Response(response) = response else {
        panic!("expected a response frame");
    };
    assert_eq!(response.kind, ClientResponseKind::Applied);
    assert_eq!(response.snapshot.catalog.len(), 1);
    assert_eq!(response.snapshot.catalog[0].id, "fixture.engine");
    client
        .shutdown(Duration::from_secs(2))
        .expect("child shuts down");
}

#[test]
fn fixture_workshop_search_round_trips_through_the_service_process() {
    let mut client = fixture_client();
    client
        .send(
            ClientAction::Workshop(WorkshopAction::Search {
                query: "moon".into(),
                page: 1,
                required_tags: vec!["game".into()],
            }),
            Some(client.latest_snapshot().revision),
        )
        .expect("send Workshop search");
    let response = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive accepted operation");
    let ProtocolFrame::Response(response) = response else {
        panic!("expected operation response");
    };
    assert_eq!(response.kind, ClientResponseKind::OperationAccepted);

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut saw_success = false;
    while std::time::Instant::now() < deadline {
        let frame = client
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("receive Workshop operation update");
        match frame {
            ProtocolFrame::Event(ClientEvent::OperationChanged(event))
                if event.action == "workshop.search"
                    && event.status == vapor_client_protocol::OperationStatus::Succeeded =>
            {
                saw_success = true;
            }
            ProtocolFrame::Event(ClientEvent::Snapshot(snapshot))
                if snapshot.workshop.search.as_ref().is_some_and(|search| {
                    search.query == "moon" && search.page == 1 && search.required_tags == ["game"]
                }) =>
            {
                assert_eq!(
                    snapshot.workshop.search.as_ref().unwrap().items[0].published_file_id,
                    "1002"
                );
                assert_eq!(
                    snapshot.workshop.search.as_ref().unwrap().items[0].tags,
                    ["game"]
                );
                break;
            }
            _ => {}
        }
    }
    assert!(saw_success, "service reports completed Workshop search");
    assert_eq!(
        client
            .latest_snapshot()
            .workshop
            .search
            .as_ref()
            .unwrap()
            .query,
        "moon"
    );
    client
        .shutdown(Duration::from_secs(2))
        .expect("fixture service shuts down");
}

#[test]
fn workspace_file_results_follow_the_terminal_service_event() {
    let root = std::env::temp_dir().join(format!(
        "vapor-client-service-process-workspace-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock follows epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("create fixture workspace");
    fs::write(root.join("Vapor.toml"), "[workspace]").expect("write workspace manifest");
    let executable = service_executable();
    let mut client = Client::spawn(
        &executable,
        ClientHostConfig {
            application_executable: executable.clone(),
            workshop_app_id: None,
            workspace_root: Some(root.clone()),
            active_composition_id: Some("default".to_owned()),
            fixture_mode: true,
            legacy_storage: LegacyStorageSeed::default(),
            state_directory: None,
        },
    )
    .expect("fixture service starts");
    client
        .send(
            ClientAction::Workspace(WorkspaceAction::ListFiles { path: ".".into() }),
            Some(client.latest_snapshot().revision),
        )
        .expect("send workspace file listing");
    let response = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive accepted workspace operation");
    assert!(matches!(
        response,
        ProtocolFrame::Response(response)
            if response.kind == ClientResponseKind::OperationAccepted
    ));

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut saw_success = false;
    let mut saw_file_result = false;
    while std::time::Instant::now() < deadline {
        let frame = client
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("receive workspace operation update");
        match frame {
            ProtocolFrame::Event(ClientEvent::OperationChanged(event))
                if event.action == "workspace.list-files"
                    && event.status == vapor_client_protocol::OperationStatus::Succeeded =>
            {
                saw_success = true;
            }
            ProtocolFrame::Event(ClientEvent::WorkspaceFileResult(result)) => {
                assert!(saw_success, "success event precedes file data");
                assert!(matches!(
                    result.as_ref(),
                    WorkspaceFileResult::Listed { files, .. }
                        if files == &["Vapor.toml"]
                ));
                saw_file_result = true;
                break;
            }
            _ => {}
        }
    }
    assert!(saw_success && saw_file_result);
    client
        .shutdown(Duration::from_secs(2))
        .expect("fixture service shuts down");
    fs::remove_dir_all(root).expect("remove fixture workspace");
}

#[test]
fn managed_workspace_tool_contract_round_trips_and_fixture_process_refuses_execution() {
    let root = std::env::temp_dir().join(format!(
        "vapor-client-service-process-tool-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock follows epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("create fixture workspace");
    fs::write(root.join("Vapor.toml"), "[workspace]").expect("write workspace manifest");
    let executable = service_executable();
    let mut client = Client::spawn(
        &executable,
        ClientHostConfig {
            application_executable: executable.clone(),
            workshop_app_id: None,
            workspace_root: Some(root.clone()),
            active_composition_id: Some("default".to_owned()),
            fixture_mode: true,
            legacy_storage: LegacyStorageSeed::default(),
            state_directory: None,
        },
    )
    .expect("fixture service starts");
    client
        .send(
            ClientAction::Workspace(WorkspaceAction::RunTool {
                operation: WorkspaceToolOperation::BuildRelease,
                project_selection: WorkspaceProjectSelection::Installable,
                target_triples: vec!["x86_64-unknown-linux-gnu".into()],
            }),
            Some(client.latest_snapshot().revision),
        )
        .expect("send managed workspace build");
    let ProtocolFrame::Response(response) = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive accepted workspace operation")
    else {
        panic!("expected a response frame");
    };
    assert_eq!(response.kind, ClientResponseKind::OperationAccepted);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut refused = false;
    while std::time::Instant::now() < deadline {
        if let ProtocolFrame::Event(ClientEvent::OperationChanged(event)) = client
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("receive workspace operation outcome")
            && event.action == "workspace.build-release"
            && let vapor_client_protocol::OperationStatus::Failed { message } = event.status
        {
            assert!(message.contains("unavailable in fixture mode"));
            refused = true;
            break;
        }
    }
    assert!(refused, "fixture service refuses managed tool execution");
    client
        .shutdown(Duration::from_secs(2))
        .expect("fixture service shuts down");
    fs::remove_dir_all(root).expect("remove fixture workspace");
}

#[test]
fn fixture_service_rejects_workspace_run_configuration_requests() {
    let root = std::env::temp_dir().join(format!(
        "vapor-client-service-process-run-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock follows epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("create fixture workspace");
    let executable = service_executable();
    let mut client = Client::spawn(
        &executable,
        ClientHostConfig {
            application_executable: executable.clone(),
            workshop_app_id: None,
            workspace_root: Some(root.clone()),
            active_composition_id: Some("default".to_owned()),
            fixture_mode: true,
            legacy_storage: LegacyStorageSeed::default(),
            state_directory: None,
        },
    )
    .expect("fixture service starts");
    client
        .send(
            ClientAction::Workspace(WorkspaceAction::RunConfiguration {
                workspace_root: root.display().to_string(),
                configuration: "default".into(),
                profiling: false,
            }),
            Some(client.latest_snapshot().revision),
        )
        .expect("send workspace Run Configuration");
    let ProtocolFrame::Response(response) = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive fixture policy response")
    else {
        panic!("expected a service response");
    };
    assert_eq!(response.kind, ClientResponseKind::Rejected);
    assert!(
        response
            .error
            .is_some_and(|error| error.message.contains("cannot run source workspaces"))
    );
    client
        .send(
            ClientAction::Workspace(WorkspaceAction::BuildDocumentation),
            Some(client.latest_snapshot().revision),
        )
        .expect("send fixture documentation build");
    let ProtocolFrame::Response(response) = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive fixture documentation policy response")
    else {
        panic!("expected a service response");
    };
    assert_eq!(response.kind, ClientResponseKind::Rejected);
    assert!(
        response
            .error
            .as_ref()
            .is_some_and(|error| error.message.contains("unavailable in fixture mode")),
        "unexpected fixture documentation response: {:?}",
        response.error
    );
    client
        .send(
            ClientAction::Workspace(WorkspaceAction::PublishDocumentation),
            Some(client.latest_snapshot().revision),
        )
        .expect("send fixture documentation publication");
    let ProtocolFrame::Response(response) = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive fixture documentation publication policy response")
    else {
        panic!("expected a service response");
    };
    assert_eq!(response.kind, ClientResponseKind::Rejected);
    assert!(
        response
            .error
            .as_ref()
            .is_some_and(|error| { error.message.contains("unavailable in fixture mode") })
    );
    client
        .send(
            ClientAction::Workspace(WorkspaceAction::DeployApplication {
                target_triples: vec![],
                skip_documentation: true,
            }),
            Some(client.latest_snapshot().revision),
        )
        .expect("send application deployment");
    let ProtocolFrame::Response(response) = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive fixture deployment policy response")
    else {
        panic!("expected a service response");
    };
    assert_eq!(response.kind, ClientResponseKind::Rejected);
    assert!(
        response
            .error
            .is_some_and(|error| { error.message.contains("unavailable in fixture mode") })
    );
    client
        .send(
            ClientAction::Workspace(WorkspaceAction::PackageApplication {
                target_triples: vec!["x86_64-unknown-linux-gnu".into()],
            }),
            Some(client.latest_snapshot().revision),
        )
        .expect("send application packaging");
    let ProtocolFrame::Response(response) = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive fixture packaging policy response")
    else {
        panic!("expected a service response");
    };
    assert_eq!(response.kind, ClientResponseKind::Rejected);
    assert!(
        response
            .error
            .is_some_and(|error| { error.message.contains("unavailable in fixture mode") })
    );
    client
        .shutdown(Duration::from_secs(2))
        .expect("fixture service shuts down");
    fs::remove_dir_all(root).expect("remove fixture workspace");
}

#[test]
fn production_service_owns_workspace_run_configuration_resolution() {
    let root = std::env::temp_dir().join(format!(
        "vapor-client-service-process-run-route-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock follows epoch")
            .as_nanos()
    ));
    fs::create_dir_all(root.join("src")).expect("create source workspace");
    fs::write(
        root.join("Workspace.vapor.toml"),
        "schema = 1\n\n[workspace]\nname = \"run-route\"\norganization = \"ghf-studios\"\nversion = \"0.1.0\"\nrepository = \"https://example.invalid/run-route\"\n\n[toolchain]\nchannel = \"stable\"\nversion = \"1.97.0\"\ndate = \"2026-07-09\"\n\n[[project]]\nname = \"run-route\"\npath = \".\"\n",
    )
    .expect("write Vapor workspace manifest");
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"run-route\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write Cargo manifest");
    fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write Rust entrypoint");

    let executable = service_executable();
    let state_directory = root.with_extension("state");
    let mut client = Client::spawn(
        &executable,
        ClientHostConfig {
            application_executable: executable.clone(),
            workshop_app_id: None,
            workspace_root: Some(root.clone()),
            active_composition_id: Some("default".to_owned()),
            fixture_mode: false,
            legacy_storage: LegacyStorageSeed::default(),
            state_directory: Some(state_directory.clone()),
        },
    )
    .expect("production service starts");
    client
        .send(
            ClientAction::Workspace(WorkspaceAction::RunConfiguration {
                workspace_root: root.display().to_string(),
                configuration: "default".into(),
                profiling: false,
            }),
            Some(client.latest_snapshot().revision),
        )
        .expect("send workspace Run Configuration");
    let ProtocolFrame::Response(response) = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive accepted workspace operation")
    else {
        panic!("expected a service response");
    };
    assert_eq!(response.kind, ClientResponseKind::OperationAccepted);

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut service_resolved_run_configuration = false;
    while std::time::Instant::now() < deadline {
        if let ProtocolFrame::Event(ClientEvent::OperationChanged(event)) = client
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("receive Run Configuration result")
            && event.action == "workspace.run-configuration"
            && let vapor_client_protocol::OperationStatus::Failed { message } = event.status
        {
            assert!(message.contains("declares no Run Configuration `default`"));
            service_resolved_run_configuration = true;
            break;
        }
    }
    assert!(
        service_resolved_run_configuration,
        "the service resolves the workspace and reports its Run Configuration error"
    );
    client
        .shutdown(Duration::from_secs(2))
        .expect("production service shuts down");
    fs::remove_dir_all(state_directory).expect("remove temporary service state");
    fs::remove_dir_all(root).expect("remove source workspace");
}

#[test]
fn fixture_workshop_acquisition_round_trips_through_the_service_process() {
    let mut client = fixture_client();
    client
        .send(
            ClientAction::Workshop(WorkshopAction::Acquire {
                item_ids: vec!["1002".into()],
            }),
            Some(client.latest_snapshot().revision),
        )
        .expect("send Workshop acquisition");
    let response = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive accepted acquisition");
    let ProtocolFrame::Response(response) = response else {
        panic!("expected operation response");
    };
    assert_eq!(response.kind, ClientResponseKind::OperationAccepted);

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut saw_success = false;
    while std::time::Instant::now() < deadline {
        let frame = client
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("receive acquisition update");
        match frame {
            ProtocolFrame::Event(ClientEvent::OperationChanged(event))
                if event.action == "workshop.acquire"
                    && event.status == vapor_client_protocol::OperationStatus::Succeeded =>
            {
                saw_success = true;
            }
            ProtocolFrame::Event(ClientEvent::Snapshot(snapshot))
                if snapshot
                    .workshop
                    .acquisition
                    .as_ref()
                    .is_some_and(|batch| batch.revision > 0) =>
            {
                let batch = snapshot.workshop.acquisition.as_ref().unwrap();
                assert_eq!(batch.requested, 1);
                assert_eq!(batch.items[0].id, "fixture.content.1002");
                break;
            }
            _ => {}
        }
    }
    assert!(
        saw_success,
        "service reports completed Workshop acquisition"
    );
    assert_eq!(
        client
            .latest_snapshot()
            .workshop
            .acquisition
            .as_ref()
            .unwrap()
            .items[0]
            .id,
        "fixture.content.1002"
    );
    client
        .shutdown(Duration::from_secs(2))
        .expect("fixture service shuts down");
}

#[test]
fn fixture_workshop_review_and_publish_round_trip_through_the_service_process() {
    let executable = service_executable();
    let mut client = Client::spawn(
        &executable,
        ClientHostConfig {
            application_executable: executable.clone(),
            workshop_app_id: Some(2122620),
            workspace_root: Some(PathBuf::from("/fixture/workspace")),
            active_composition_id: Some("default".to_owned()),
            fixture_mode: true,
            legacy_storage: LegacyStorageSeed::default(),
            state_directory: None,
        },
    )
    .expect("fixture service starts with its authored workspace");
    client
        .send(
            ClientAction::Workshop(WorkshopAction::Stage {
                selector: "fixture.game@1.0.0".into(),
                target_triples: vec![
                    "x86_64-unknown-linux-gnu".into(),
                    "x86_64-pc-windows-msvc".into(),
                ],
            }),
            Some(client.latest_snapshot().revision),
        )
        .expect("send stage request");
    let response = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive accepted stage");
    let ProtocolFrame::Response(response) = response else {
        panic!("expected stage response");
    };
    assert_eq!(response.kind, ClientResponseKind::OperationAccepted);

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut saw_stage_success = false;
    while std::time::Instant::now() < deadline {
        match client
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("receive stage result")
        {
            ProtocolFrame::Event(ClientEvent::OperationChanged(event))
                if event.action == "workshop.stage"
                    && event.status == vapor_client_protocol::OperationStatus::Succeeded =>
            {
                saw_stage_success = true;
            }
            ProtocolFrame::Event(ClientEvent::Snapshot(snapshot))
                if snapshot.workshop.review.is_some() =>
            {
                assert_eq!(
                    snapshot.workshop.review.as_ref().unwrap().selector,
                    "fixture.game@1.0.0"
                );
                assert_eq!(
                    snapshot.workshop.review.as_ref().unwrap().preview.target,
                    "x86_64-unknown-linux-gnu,x86_64-pc-windows-msvc"
                );
                break;
            }
            _ => {}
        }
    }
    assert!(saw_stage_success, "service completes Workshop staging");
    let review = client
        .latest_snapshot()
        .workshop
        .review
        .as_ref()
        .expect("service snapshot contains the review")
        .clone();

    client
        .send(
            ClientAction::Workshop(WorkshopAction::Publish {
                review_id: review.review_id,
                account: "publisher".into(),
                confirmation: "PUBLISH fixture.game TO publisher".into(),
                change_note: "Fixture only".into(),
            }),
            Some(client.latest_snapshot().revision),
        )
        .expect("send reviewed publish request");
    let response = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive accepted fixture publication");
    let ProtocolFrame::Response(response) = response else {
        panic!("expected publish response");
    };
    assert_eq!(response.kind, ClientResponseKind::OperationAccepted);

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut saw_publish_success = false;
    while std::time::Instant::now() < deadline {
        match client
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("receive publish result")
        {
            ProtocolFrame::Event(ClientEvent::OperationChanged(event))
                if event.action == "workshop.publish"
                    && event.status == vapor_client_protocol::OperationStatus::Succeeded =>
            {
                saw_publish_success = true;
            }
            ProtocolFrame::Event(ClientEvent::Snapshot(snapshot))
                if snapshot.workshop.review.is_none()
                    && saw_publish_success
                    && client.latest_snapshot().workshop.review.is_none() =>
            {
                break;
            }
            _ => {}
        }
    }
    assert!(
        saw_publish_success,
        "fixture publication reports completion"
    );
    assert!(client.latest_snapshot().workshop.review.is_none());
    client
        .shutdown(Duration::from_secs(2))
        .expect("fixture service shuts down");
}

#[test]
fn fixture_workshop_batch_review_round_trips_through_the_service_process() {
    let executable = service_executable();
    let mut client = Client::spawn(
        &executable,
        ClientHostConfig {
            application_executable: executable.clone(),
            workshop_app_id: Some(2122620),
            workspace_root: Some(PathBuf::from("/fixture/workspace")),
            active_composition_id: Some("default".to_owned()),
            fixture_mode: true,
            legacy_storage: LegacyStorageSeed::default(),
            state_directory: None,
        },
    )
    .expect("fixture service starts with its authored workspace");
    client
        .send(
            ClientAction::Workshop(WorkshopAction::StageBatch {
                selectors: vec!["fixture.game@1.0.0".into()],
                target_triples: vec!["x86_64-unknown-linux-gnu".into()],
            }),
            Some(client.latest_snapshot().revision),
        )
        .expect("send batch stage request");
    let response = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive accepted batch stage");
    let ProtocolFrame::Response(response) = response else {
        panic!("expected batch stage response");
    };
    assert_eq!(response.kind, ClientResponseKind::OperationAccepted);

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if let Some(review) = client.latest_snapshot().workshop.batch_review.as_ref() {
            assert_eq!(review.items.len(), 1);
            assert_eq!(review.items[0].selector, "fixture.game@1.0.0");
            assert_eq!(review.items[0].preview.target, "x86_64-unknown-linux-gnu");
            break;
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let _ = client
            .recv_timeout(remaining)
            .expect("receive batch stage result");
    }
    let review = client
        .latest_snapshot()
        .workshop
        .batch_review
        .as_ref()
        .expect("service snapshot contains the batch review")
        .clone();

    client
        .send(
            ClientAction::Workshop(WorkshopAction::PublishBatch {
                review_id: review.review_id.clone(),
                account: "publisher".into(),
                confirmation: "PUBLISH 1 ITEM TO someone-else".into(),
                change_note: "Fixture batch".into(),
            }),
            Some(client.latest_snapshot().revision),
        )
        .expect("send wrong batch confirmation");
    let response = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive wrong-confirmation rejection");
    let ProtocolFrame::Response(response) = response else {
        panic!("expected wrong-confirmation response");
    };
    assert_eq!(response.kind, ClientResponseKind::Rejected);
    assert!(client.latest_snapshot().workshop.batch_review.is_some());

    client
        .send(
            ClientAction::Workshop(WorkshopAction::PublishBatch {
                review_id: review.review_id,
                account: "publisher".into(),
                confirmation: "PUBLISH 1 ITEM TO publisher".into(),
                change_note: "Fixture batch".into(),
            }),
            Some(client.latest_snapshot().revision),
        )
        .expect("send reviewed batch publish request");
    let response = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive accepted batch publication");
    let ProtocolFrame::Response(response) = response else {
        panic!("expected batch publish response");
    };
    assert_eq!(response.kind, ClientResponseKind::OperationAccepted);

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut saw_success = false;
    let mut saw_review_cleared = false;
    while std::time::Instant::now() < deadline {
        match client
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("receive batch publication result")
        {
            ProtocolFrame::Event(ClientEvent::OperationChanged(event))
                if event.action == "workshop.publish-batch"
                    && event.status == vapor_client_protocol::OperationStatus::Succeeded =>
            {
                saw_success = true;
            }
            ProtocolFrame::Event(ClientEvent::Snapshot(snapshot))
                if snapshot.workshop.batch_review.is_none() =>
            {
                saw_review_cleared = true;
            }
            _ => {}
        }
        if saw_success && saw_review_cleared {
            break;
        }
    }
    assert!(saw_success, "fixture batch publication reports completion");
    assert!(
        saw_review_cleared,
        "published review is removed from the snapshot"
    );
    assert!(client.latest_snapshot().workshop.batch_review.is_none());
    client
        .shutdown(Duration::from_secs(2))
        .expect("fixture service shuts down");
}

#[test]
fn fixture_process_cannot_publish_to_workshop() {
    let mut client = fixture_client();
    let revision = client.latest_snapshot().revision;
    client
        .send(
            ClientAction::Workshop(WorkshopAction::Publish {
                review_id: "fixture-review".into(),
                account: "publisher".into(),
                confirmation: "fixture-confirmation".into(),
                change_note: String::new(),
            }),
            Some(revision),
        )
        .expect("send publish attempt");
    let response = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive rejection");
    let ProtocolFrame::Response(response) = response else {
        panic!("expected a response frame");
    };
    assert_eq!(response.kind, ClientResponseKind::Rejected);
    assert_eq!(
        response.error.expect("structured rejection").code,
        vapor_client_protocol::ServiceErrorCode::InvalidRequest
    );
}

#[test]
fn every_checked_in_fixture_scenario_runs_through_a_child_process() {
    let executable = service_executable();
    let scenarios = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenarios");
    for name in [
        "fixture-async-operation.json",
        "fixture-composition-selection.json",
        "fixture-workspace-discovery.json",
        "workspace-open-recovery.json",
        "fixture-publisher-boundary.json",
    ] {
        run_simulator(&executable, Some(&scenarios.join(name)))
            .unwrap_or_else(|error| panic!("scenario {name} failed: {error}"));
    }
}

#[test]
fn production_service_imports_legacy_state_then_uses_its_store_after_restart() {
    let executable = service_executable();
    let state_directory = std::env::temp_dir().join(format!(
        "vapor-studio-restart-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos()
    ));
    let legacy_profiles = CompositionDocument::default();
    let legacy_storage = LegacyStorageSeed {
        profiles: Some(serde_json::to_string(&legacy_profiles).expect("encode legacy profiles")),
        active_profile_id: Some("default".into()),
        ..LegacyStorageSeed::default()
    };
    let mut client = Client::spawn(
        &executable,
        ClientHostConfig {
            application_executable: executable.clone(),
            workshop_app_id: None,
            workspace_root: None,
            active_composition_id: None,
            fixture_mode: false,
            legacy_storage,
            state_directory: Some(state_directory.clone()),
        },
    )
    .expect("production service imports legacy state");
    assert_eq!(
        client
            .latest_snapshot()
            .durable_state
            .compositions
            .compositions
            .len(),
        1
    );
    client
        .send(
            ClientAction::Composition(CompositionAction::Create {
                name: "Persisted composition".into(),
            }),
            Some(client.latest_snapshot().revision),
        )
        .expect("send profile creation");
    let response = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive profile creation");
    let ProtocolFrame::Response(response) = response else {
        panic!("expected profile response");
    };
    assert_eq!(response.kind, ClientResponseKind::Applied);
    assert_eq!(
        response
            .snapshot
            .durable_state
            .compositions
            .compositions
            .len(),
        2
    );
    client
        .send(
            ClientAction::Composition(CompositionAction::SetLaunchArguments {
                composition_id: "composition-1".into(),
                arguments: vec!["--safe-mode".into()],
            }),
            Some(response.revision),
        )
        .expect("send profile launch settings");
    let response = client
        .recv_timeout(Duration::from_secs(2))
        .expect("receive launch settings");
    let ProtocolFrame::Response(response) = response else {
        panic!("expected launch settings response");
    };
    assert_eq!(response.kind, ClientResponseKind::Applied);
    assert_eq!(response.snapshot.durable_state.composition_revision, 2);
    client
        .shutdown(Duration::from_secs(2))
        .expect("first service shuts down");

    let restarted = Client::spawn(
        &executable,
        ClientHostConfig {
            application_executable: executable.clone(),
            workshop_app_id: None,
            workspace_root: None,
            active_composition_id: None,
            fixture_mode: false,
            legacy_storage: LegacyStorageSeed {
                profiles: Some("invalid and stale fallback".into()),
                active_profile_id: Some("missing".into()),
                ..LegacyStorageSeed::default()
            },
            state_directory: Some(state_directory.clone()),
        },
    )
    .expect("service reloads authoritative state");
    assert_eq!(
        restarted
            .latest_snapshot()
            .durable_state
            .compositions
            .compositions[1]
            .name,
        "Persisted composition"
    );
    assert_eq!(
        restarted
            .latest_snapshot()
            .durable_state
            .active_composition_id,
        "composition-1"
    );
    assert_eq!(
        restarted
            .latest_snapshot()
            .durable_state
            .compositions
            .compositions[1]
            .launch
            .arguments,
        ["--safe-mode"]
    );
    assert!(state_directory.join("studio-state.json").is_file());
    restarted
        .shutdown(Duration::from_secs(2))
        .expect("restarted service shuts down");
    fs::remove_dir_all(state_directory).expect("remove test state");
}

#[test]
fn catalog_search_preserves_complete_facts_through_the_child_process() {
    let mut client = fixture_client();
    for (action, expected_matches) in [
        (CatalogAction::Refresh, 1),
        (
            CatalogAction::Search {
                query: "absent".into(),
            },
            0,
        ),
        (
            CatalogAction::Search {
                query: "ENGINE".into(),
            },
            1,
        ),
        (CatalogAction::Refresh, 1),
    ] {
        let request_id = client
            .send(
                ClientAction::Catalog(action),
                Some(client.latest_snapshot().revision),
            )
            .expect("send catalog action");
        let ProtocolFrame::Response(response) = client
            .recv_timeout(Duration::from_secs(2))
            .expect("catalog response")
        else {
            panic!("expected response");
        };
        assert_eq!(response.request_id, request_id);
        assert_eq!(response.kind, ClientResponseKind::Applied);
        assert_eq!(response.snapshot.catalog.len(), 1);
        assert_eq!(
            response.snapshot.catalog_search.matches.len(),
            expected_matches
        );
        assert!(response.snapshot.catalog_status.is_current());
    }
    assert_eq!(client.latest_snapshot().catalog_search.query, "engine");
    client
        .shutdown(Duration::from_secs(2))
        .expect("child shuts down");
}
