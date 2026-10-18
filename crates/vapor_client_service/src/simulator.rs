use crate::scenario::load_scenario;
use std::{
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use vapor_client::{Client, ClientHostConfig as ClientBootstrap};
use vapor_client_protocol::{
    CatalogAction, ClientAction, ClientEvent, CompositionAction, OperationEvent, OperationStatus,
    ProtocolFrame, WorkshopAction,
};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
const OPERATION_TIMEOUT: Duration = Duration::from_secs(30);

pub fn run_simulator(
    service_executable: &Path,
    scenario_path: Option<&Path>,
) -> Result<(), String> {
    if let Some(path) = scenario_path {
        return run_scenario(service_executable, path);
    }
    run_interactive(service_executable)
}

fn spawn_fixture_client(executable: &Path) -> Result<Client, String> {
    Client::spawn(
        executable,
        ClientBootstrap {
            application_executable: executable.to_path_buf(),
            workshop_app_id: None,
            workspace_root: None,
            active_composition_id: Some("default".to_owned()),
            fixture_mode: true,
            legacy_storage: vapor_client_protocol::LegacyStorageSeed::default(),
            state_directory: None,
        },
    )
    .map_err(|error| error.to_string())
}

fn run_scenario(executable: &Path, scenario_path: &Path) -> Result<(), String> {
    let scenario = load_scenario(scenario_path)?;
    let fixture_workspace = FixtureWorkspace::create()?;
    let mut client = spawn_fixture_client(executable)?;
    println!("Scenario: {}", scenario.name);
    println!("Service: fixture mode; no production provider is available.");
    for (index, mut step) in scenario.steps.into_iter().enumerate() {
        if let ClientAction::Workspace(vapor_client_protocol::WorkspaceAction::Open { root }) =
            &mut step.action
            && root == "fixture://workspace"
        {
            *root = fixture_workspace.path.display().to_string();
        } else if let ClientAction::Workspace(vapor_client_protocol::WorkspaceAction::Open { root }) =
            &mut step.action
            && let Some(relative) = root.strip_prefix("fixture://workspace/")
        {
            *root = fixture_workspace.path.join(relative).display().to_string();
        }
        let previous_revision = client.latest_snapshot().revision;
        let request_id = client
            .send(step.action.clone(), Some(previous_revision))
            .map_err(|error| error.to_string())?;
        let frame = client
            .recv_timeout(RESPONSE_TIMEOUT)
            .map_err(|error| error.to_string())?;
        let ProtocolFrame::Response(response) = frame else {
            return Err(format!("step {} received a non-response frame", index + 1));
        };
        if response.request_id != request_id {
            return Err(format!(
                "step {} received a mismatched request ID",
                index + 1
            ));
        }
        let terminal_operation =
            if response.kind == vapor_client_protocol::ClientResponseKind::OperationAccepted {
                let operation_id = response
                    .snapshot
                    .active_operation
                    .as_ref()
                    .map(|operation| operation.id.as_str())
                    .ok_or_else(|| {
                        format!(
                            "step {} accepted an operation without an operation ID",
                            index + 1
                        )
                    })?;
                Some(await_terminal_operation(&mut client, operation_id)?)
            } else {
                None
            };
        if step
            .expect
            .response
            .is_some_and(|expected| expected != response.kind)
        {
            return Err(format!(
                "step {} expected {:?}, got {:?}",
                index + 1,
                step.expect.response,
                response.kind
            ));
        }
        if step.expect.error.is_some_and(|expected| {
            response.error.as_ref().map(|error| error.code) != Some(expected)
        }) {
            return Err(format!(
                "step {} expected error {:?}, got {:?}",
                index + 1,
                step.expect.error,
                response.error
            ));
        }
        if let Some(expected) = &step.expect.operation_action
            && terminal_operation.as_ref().map(|event| &event.action) != Some(expected)
        {
            return Err(format!(
                "step {} expected operation action {expected:?}, got {:?}",
                index + 1,
                terminal_operation.as_ref().map(|event| &event.action)
            ));
        }
        if let Some(expected) = &step.expect.operation_status
            && terminal_operation.as_ref().map(|event| &event.status) != Some(expected)
        {
            return Err(format!(
                "step {} expected operation status {expected:?}, got {:?}",
                index + 1,
                terminal_operation.as_ref().map(|event| &event.status)
            ));
        }
        if let Some(expected) = &step.expect.operation_message_contains
            && !terminal_operation
                .as_ref()
                .and_then(|event| event.message.as_ref())
                .is_some_and(|message| message.contains(expected))
        {
            return Err(format!(
                "step {} operation message did not contain {expected:?}",
                index + 1
            ));
        }
        let snapshot = terminal_operation
            .as_ref()
            .map(|_| client.latest_snapshot().clone())
            .unwrap_or(response.snapshot);
        if step
            .expect
            .active_composition_id
            .as_ref()
            .is_some_and(|expected| snapshot.active_composition_id.as_ref() != Some(expected))
        {
            return Err(format!(
                "step {} active profile did not match expectation",
                index + 1
            ));
        }
        if step
            .expect
            .selected_content_id
            .as_ref()
            .is_some_and(|expected| snapshot.selected_content_id.as_ref() != Some(expected))
        {
            return Err(format!(
                "step {} selected content did not match expectation",
                index + 1
            ));
        }
        if step
            .expect
            .catalog_len
            .is_some_and(|expected| snapshot.catalog.len() != expected)
        {
            return Err(format!(
                "step {} expected {} catalog entries, got {}",
                index + 1,
                step.expect.catalog_len.unwrap_or_default(),
                snapshot.catalog.len()
            ));
        }
        if step
            .expect
            .catalog_matches_len
            .is_some_and(|expected| snapshot.catalog_search.matches.len() != expected)
        {
            return Err(format!(
                "step {} catalog query result count did not match",
                index + 1
            ));
        }
        if step
            .expect
            .catalog_loaded
            .is_some_and(|expected| snapshot.catalog_status.loaded != expected)
        {
            return Err(format!(
                "step {} catalog observation state did not match",
                index + 1
            ));
        }
        if step
            .expect
            .catalog_has_error
            .is_some_and(|expected| snapshot.catalog_status.error.is_some() != expected)
        {
            return Err(format!(
                "step {} catalog error state did not match",
                index + 1
            ));
        }
        if step
            .expect
            .workspace_source_projects_len
            .is_some_and(|expected| snapshot.workspace_content.source_projects.len() != expected)
        {
            return Err(format!(
                "step {} expected {} workspace source projects, got {}",
                index + 1,
                step.expect
                    .workspace_source_projects_len
                    .unwrap_or_default(),
                snapshot.workspace_content.source_projects.len()
            ));
        }
        if step
            .expect
            .workspace_candidates_len
            .is_some_and(|expected| {
                snapshot.workspace_content.unregistered_projects.len() != expected
            })
        {
            return Err(format!(
                "step {} expected {} unregistered projects, got {}",
                index + 1,
                step.expect.workspace_candidates_len.unwrap_or_default(),
                snapshot.workspace_content.unregistered_projects.len()
            ));
        }
        println!(
            "{}. {} · revision {} → {} · {:?}{}{}",
            index + 1,
            step.action.id(),
            previous_revision.0,
            response.revision.0,
            response.kind,
            response
                .error
                .as_ref()
                .map(|error| format!(" · {}", error.message))
                .unwrap_or_default(),
            terminal_operation
                .as_ref()
                .map(|event| format!(" · operation {:?}", event.status))
                .unwrap_or_default(),
        );
    }
    client
        .shutdown(Duration::from_secs(2))
        .map_err(|error| error.to_string())?;
    println!("Scenario passed.");
    Ok(())
}

fn await_terminal_operation(
    client: &mut Client,
    operation_id: &str,
) -> Result<OperationEvent, String> {
    let deadline = Instant::now() + OPERATION_TIMEOUT;
    let mut terminal = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "operation {operation_id} did not finish within {} seconds",
                OPERATION_TIMEOUT.as_secs()
            ));
        }
        let frame = client
            .recv_timeout(remaining)
            .map_err(|error| format!("waiting for operation {operation_id}: {error}"))?;
        match frame {
            ProtocolFrame::Event(ClientEvent::OperationChanged(event))
                if event.id == operation_id && event.status != OperationStatus::Running =>
            {
                terminal = Some(event);
            }
            ProtocolFrame::Event(ClientEvent::Snapshot(_)) if terminal.is_some() => {
                return Ok(terminal.expect("terminal event checked above"));
            }
            ProtocolFrame::Event(ClientEvent::ProtocolError(error)) => {
                return Err(format!("operation {operation_id}: {}", error.message));
            }
            _ => {}
        }
    }
}

struct FixtureWorkspace {
    path: PathBuf,
}

impl FixtureWorkspace {
    fn create() -> Result<Self, String> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| format!("system clock is before the epoch: {error}"))?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "vapor-studio-scenario-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path)
            .map_err(|error| format!("could not create isolated fixture workspace: {error}"))?;
        Ok(Self { path })
    }
}

impl Drop for FixtureWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn run_interactive(executable: &Path) -> Result<(), String> {
    let mut client = spawn_fixture_client(executable)?;
    println!("Vapor Vapor Client service simulator · fixture mode");
    print_snapshot(client.latest_snapshot());
    println!("Type help for commands; this process cannot publish to Workshop or Steam.");
    let stdin = io::stdin();
    let mut input = String::new();
    loop {
        print!("studio> ");
        io::stdout().flush().map_err(|error| error.to_string())?;
        input.clear();
        if stdin
            .lock()
            .read_line(&mut input)
            .map_err(|error| error.to_string())?
            == 0
        {
            break;
        }
        let line = input.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let action = match parts.next().unwrap_or_default() {
            "help" => {
                println!(
                    "open WORKSPACE | discover | refresh | search QUERY | inspect ID [VERSION] | composition ID | publish | snapshot | quit"
                );
                continue;
            }
            "refresh" => ClientAction::Catalog(CatalogAction::Refresh),
            "discover" => ClientAction::Catalog(CatalogAction::DiscoverContentProjects),
            "open" => {
                let Some(root) = parts.next() else {
                    println!("usage: open WORKSPACE_PATH");
                    continue;
                };
                ClientAction::Workspace(vapor_client_protocol::WorkspaceAction::Open {
                    root: root.to_owned(),
                })
            }
            "search" => ClientAction::Catalog(CatalogAction::Search {
                query: parts.collect::<Vec<_>>().join(" "),
            }),
            "inspect" => {
                let Some(content_id) = parts.next() else {
                    println!("usage: inspect ID [VERSION]");
                    continue;
                };
                ClientAction::Catalog(CatalogAction::Inspect {
                    content_id: content_id.to_owned(),
                    version: parts.next().map(ToOwned::to_owned),
                })
            }
            "composition" | "profile" => {
                let Some(composition_id) = parts.next() else {
                    println!("usage: composition ID");
                    continue;
                };
                ClientAction::Composition(CompositionAction::Select {
                    composition_id: composition_id.to_owned(),
                })
            }
            "publish" => ClientAction::Workshop(WorkshopAction::Publish {
                review_id: "simulated-review".into(),
                account: String::new(),
                confirmation: "simulated-confirmation".into(),
                change_note: String::new(),
            }),
            "snapshot" => {
                print_snapshot(client.latest_snapshot());
                continue;
            }
            "quit" | "exit" => break,
            command => {
                println!("unknown command: {command}");
                continue;
            }
        };
        let revision = client.latest_snapshot().revision;
        client
            .send(action, Some(revision))
            .map_err(|error| error.to_string())?;
        match client
            .recv_timeout(RESPONSE_TIMEOUT)
            .map_err(|error| error.to_string())?
        {
            ProtocolFrame::Response(response) => {
                println!(
                    "{:?} · revision {}{}",
                    response.kind,
                    response.revision.0,
                    response
                        .error
                        .map(|error| format!(" · {}", error.message))
                        .unwrap_or_default()
                );
                print_snapshot(&response.snapshot);
            }
            frame => println!("unexpected service frame: {frame:?}"),
        }
    }
    client
        .shutdown(Duration::from_secs(2))
        .map_err(|error| error.to_string())
}

fn print_snapshot(snapshot: &vapor_client_protocol::ClientSnapshot) {
    println!(
        "revision {} · {} catalog item(s) · host mode {}",
        snapshot.revision.0,
        snapshot.catalog.len(),
        snapshot.host.mode
    );
    if !snapshot.workspace_content.source_projects.is_empty()
        || snapshot.workspace_content.source_projects_loaded
        || snapshot.workspace_content.unregistered_projects_loaded
    {
        println!(
            "workspace · {} source project(s) · {} adoption candidate(s){}{}",
            snapshot.workspace_content.source_projects.len(),
            snapshot.workspace_content.unregistered_projects.len(),
            snapshot
                .workspace_content
                .source_projects_error
                .as_ref()
                .map(|error| format!(" · source discovery error: {error}"))
                .unwrap_or_default(),
            snapshot
                .workspace_content
                .unregistered_projects_error
                .as_ref()
                .map(|error| format!(" · project scan error: {error}"))
                .unwrap_or_default(),
        );
    }
    println!(
        "catalog · loaded={} · omitted={} · error={} · query={:?} · {} match(es)",
        snapshot.catalog_status.loaded,
        snapshot.catalog_status.omitted,
        snapshot.catalog_status.error.as_deref().unwrap_or("none"),
        snapshot.catalog_search.query,
        snapshot.catalog_search.matches.len(),
    );
    for entry in &snapshot.catalog {
        println!(
            "  {} {} [{}] · {} location(s)",
            entry.id,
            entry.version.as_deref().unwrap_or(""),
            entry.kind,
            entry.locations.len()
        );
    }
    if let Some(composition) = &snapshot.active_composition_id {
        println!("active composition: {composition}");
    }
}
