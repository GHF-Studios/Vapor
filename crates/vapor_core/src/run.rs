//! Vapor Run Configurations and local Development Sessions.

use crate::{
    InstallationError, ManagedToolchain, RunConfiguration, RunConfigurationError, ToolchainError,
    VaporInstallation, VaporProject, VaporWorkspace, WorkspaceError, development_target_dir,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::thread;
use vapor_telemetry::{
    TELEMETRY_ADDR_ENV, TELEMETRY_SESSION_ENV, TelemetryBroker, TelemetryPayload,
    TelemetryPublisher,
};

const SESSION_ROOT: &str = "development/sessions";
const ACTIVE_SESSION_FILE: &str = "active.json";
const SESSION_FILE: &str = "session.json";
const EVENTS_FILE: &str = "events.ndjson";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveDevelopmentSession {
    pub schema: u32,
    pub id: String,
    pub configuration: String,
    pub workspace: PathBuf,
    pub started_unix_ms: u64,
    pub address: String,
    pub process_id: Option<u32>,
    pub state: String,
}

struct DevelopmentSession {
    info: ActiveDevelopmentSession,
    session_path: PathBuf,
    active_path: PathBuf,
    broker: TelemetryBroker,
}

impl DevelopmentSession {
    fn start(
        toolchain: &ManagedToolchain,
        workspace: &VaporWorkspace,
        configuration: &str,
    ) -> Result<Self, DevelopmentRunError> {
        let root = toolchain.user_data_root.join(SESSION_ROOT);
        fs::create_dir_all(&root).map_err(|source| DevelopmentRunError::Io {
            path: root.clone(),
            source,
        })?;

        let started_unix_ms = vapor_telemetry::unix_time_ms();
        let id = format!("{started_unix_ms}-{}", std::process::id());
        let session_root = root.join(&id);
        fs::create_dir_all(&session_root).map_err(|source| DevelopmentRunError::Io {
            path: session_root.clone(),
            source,
        })?;

        let broker = TelemetryBroker::start(session_root.join(EVENTS_FILE))
            .map_err(DevelopmentRunError::Telemetry)?;
        let info = ActiveDevelopmentSession {
            schema: 1,
            id,
            configuration: configuration.to_owned(),
            workspace: workspace.root.clone(),
            started_unix_ms,
            address: broker.address().to_string(),
            process_id: None,
            state: "starting".to_owned(),
        };
        let session_path = session_root.join(SESSION_FILE);
        let active_path = root.join(ACTIVE_SESSION_FILE);

        write_json(&session_path, &info)?;
        write_json(&active_path, &info)?;
        broker.publisher().payload(
            "vapor",
            TelemetryPayload::Lifecycle {
                state: "starting".to_owned(),
                detail: Some(configuration.to_owned()),
            },
        );

        Ok(Self {
            info,
            session_path,
            active_path,
            broker,
        })
    }

    fn mark_running(&mut self, process_id: u32) -> Result<(), DevelopmentRunError> {
        self.info.process_id = Some(process_id);
        self.info.state = "running".to_owned();
        write_json(&self.session_path, &self.info)?;
        write_json(&self.active_path, &self.info)?;
        self.broker.publisher().payload(
            "vapor",
            TelemetryPayload::Lifecycle {
                state: "running".to_owned(),
                detail: Some(process_id.to_string()),
            },
        );
        Ok(())
    }

    fn finish(mut self, success: bool) -> Result<(), DevelopmentRunError> {
        self.info.state = if success { "finished" } else { "failed" }.to_owned();
        write_json(&self.session_path, &self.info)?;
        self.broker.publisher().payload(
            "vapor",
            TelemetryPayload::Lifecycle {
                state: self.info.state.clone(),
                detail: None,
            },
        );

        if let Ok(source) = fs::read_to_string(&self.active_path)
            && let Ok(active) = serde_json::from_str::<ActiveDevelopmentSession>(&source)
            && active.id == self.info.id
        {
            let _ = fs::remove_file(&self.active_path);
        }

        Ok(())
    }
}

pub fn active_development_session(
) -> Result<Option<ActiveDevelopmentSession>, DevelopmentRunError> {
    let installation =
        VaporInstallation::discover().map_err(DevelopmentRunError::Installation)?;
    let path = installation
        .user_data_root()
        .join(SESSION_ROOT)
        .join(ACTIVE_SESSION_FILE);

    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(DevelopmentRunError::Io { path, source });
        }
    };

    serde_json::from_str(&source)
        .map(Some)
        .map_err(|source| DevelopmentRunError::SessionMetadata { path, source })
}

pub fn run_workspace_configuration(
    workspace: &VaporWorkspace,
    name: &str,
) -> Result<(), DevelopmentRunError> {
    let configuration = workspace
        .run_configuration(name)
        .map_err(DevelopmentRunError::Configuration)?;
    let project = select_project(workspace, &configuration)?;
    let toolchain =
        ManagedToolchain::for_workspace(workspace).map_err(DevelopmentRunError::Toolchain)?;

    let mut command = toolchain
        .cargo_command()
        .map_err(DevelopmentRunError::Toolchain)?;
    command
        .arg("run")
        .args(["--manifest-path", "Cargo.toml"])
        .env("CARGO_TARGET_DIR", development_target_dir(&toolchain, project))
        .current_dir(&project.root)
        .envs(&configuration.environment);

    if let Some(package) = &configuration.package {
        command.args(["--package", package]);
    }
    if let Some(binary) = &configuration.binary {
        command.args(["--bin", binary]);
    }
    if let Some(profile) = &configuration.profile {
        command.args(["--profile", profile]);
    }
    if !configuration.features.is_empty() {
        command
            .arg("--features")
            .arg(configuration.features.join(","));
    }
    if !configuration.arguments.is_empty() {
        command.arg("--").args(&configuration.arguments);
    }

    if !configuration.telemetry {
        let status = command
            .status()
            .map_err(|source| DevelopmentRunError::CargoStart {
                configuration: name.to_owned(),
                source,
            })?;
        return cargo_result(name, status);
    }

    let mut session = DevelopmentSession::start(&toolchain, workspace, name)?;
    println!(
        "Development Session {} · telemetry {} · open `vapor-monitor` to observe",
        session.info.id, session.info.address
    );
    command
        .env(TELEMETRY_ADDR_ENV, &session.info.address)
        .env(TELEMETRY_SESSION_ENV, &session.info.id)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|source| DevelopmentRunError::CargoStart {
            configuration: name.to_owned(),
            source,
        })?;
    session.mark_running(child.id())?;

    let publisher = session.broker.publisher();
    let stdout = child
        .stdout
        .take()
        .map(|stdout| forward_lines(stdout, "stdout", publisher.clone(), false));
    let stderr = child
        .stderr
        .take()
        .map(|stderr| forward_lines(stderr, "stderr", publisher, true));

    let status = child
        .wait()
        .map_err(|source| DevelopmentRunError::CargoStart {
            configuration: name.to_owned(),
            source,
        })?;

    if let Some(thread) = stdout {
        let _ = thread.join();
    }
    if let Some(thread) = stderr {
        let _ = thread.join();
    }

    session.finish(status.success())?;
    cargo_result(name, status)
}

fn select_project<'a>(
    workspace: &'a VaporWorkspace,
    configuration: &RunConfiguration,
) -> Result<&'a VaporProject, DevelopmentRunError> {
    if let Some(project) = configuration.project.as_deref() {
        return workspace
            .project(project)
            .ok_or_else(|| DevelopmentRunError::UnknownProject {
                configuration: configuration.name.clone(),
                project: project.to_owned(),
            });
    }

    match workspace.projects.as_slice() {
        [project] => Ok(project),
        projects => Err(DevelopmentRunError::AmbiguousProject {
            configuration: configuration.name.clone(),
            projects: projects.iter().map(|project| project.name.clone()).collect(),
        }),
    }
}

fn forward_lines<R: Read + Send + 'static>(
    reader: R,
    stream: &'static str,
    publisher: TelemetryPublisher,
    stderr: bool,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            if stderr {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
            publisher.payload(
                "process",
                TelemetryPayload::Log {
                    stream: stream.to_owned(),
                    message: line,
                },
            );
        }
    })
}

fn cargo_result(name: &str, status: ExitStatus) -> Result<(), DevelopmentRunError> {
    if status.success() {
        Ok(())
    } else {
        Err(DevelopmentRunError::CargoFailed {
            configuration: name.to_owned(),
            status,
        })
    }
}

fn write_json(
    path: &Path,
    value: &impl Serialize,
) -> Result<(), DevelopmentRunError> {
    let source =
        serde_json::to_vec_pretty(value).map_err(DevelopmentRunError::SerializeSession)?;
    fs::write(path, source).map_err(|source| DevelopmentRunError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Debug)]
pub enum DevelopmentRunError {
    Workspace(WorkspaceError),
    Configuration(RunConfigurationError),
    Toolchain(ToolchainError),
    Installation(InstallationError),
    UnknownProject {
        configuration: String,
        project: String,
    },
    AmbiguousProject {
        configuration: String,
        projects: Vec<String>,
    },
    CargoStart {
        configuration: String,
        source: io::Error,
    },
    CargoFailed {
        configuration: String,
        status: ExitStatus,
    },
    Telemetry(io::Error),
    Io {
        path: PathBuf,
        source: io::Error,
    },
    SerializeSession(serde_json::Error),
    SessionMetadata {
        path: PathBuf,
        source: serde_json::Error,
    },
}

impl fmt::Display for DevelopmentRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Workspace(error) => error.fmt(formatter),
            Self::Configuration(error) => error.fmt(formatter),
            Self::Toolchain(error) => error.fmt(formatter),
            Self::Installation(error) => error.fmt(formatter),
            Self::UnknownProject {
                configuration,
                project,
            } => write!(
                formatter,
                "Run Configuration `{configuration}` selects unknown Vapor Project `{project}`"
            ),
            Self::AmbiguousProject {
                configuration,
                projects,
            } => write!(
                formatter,
                "Run Configuration `{configuration}` must select a Vapor Project; available Projects: {}",
                projects.join(", ")
            ),
            Self::CargoStart {
                configuration,
                source,
            } => write!(
                formatter,
                "failed to start Run Configuration `{configuration}`: {source}"
            ),
            Self::CargoFailed {
                configuration,
                status,
            } => write!(
                formatter,
                "Run Configuration `{configuration}` exited with {status}"
            ),
            Self::Telemetry(error) => write!(formatter, "failed to start telemetry broker: {error}"),
            Self::Io { path, source } => {
                write!(formatter, "failed to access `{}`: {source}", path.display())
            }
            Self::SerializeSession(error) => {
                write!(formatter, "failed to serialize Development Session metadata: {error}")
            }
            Self::SessionMetadata { path, source } => write!(
                formatter,
                "invalid Development Session metadata `{}`: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for DevelopmentRunError {}
