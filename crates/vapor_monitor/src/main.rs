use clap::Parser;
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};
use vapor_core::{ActiveDevelopmentSession, active_development_session};
use vapor_telemetry::{TelemetryConsumer, TelemetryEvent, TelemetryPayload};

const LOG_LINES: usize = 16;
const SNAPSHOTS: usize = 4;

#[derive(Debug, Parser)]
#[command(name = "vapor-monitor", about = "Live Vapor Development Session telemetry")]
struct Cli {
    /// Connect directly to a telemetry broker instead of the active Vapor session.
    #[arg(long)]
    address: Option<String>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("vapor-monitor: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let (address, session) = match cli.address {
        Some(address) => (address, None),
        None => {
            let session = active_development_session()
                .map_err(|error| error.to_string())?
                .ok_or_else(|| {
                    "no active Vapor Development Session; start one with `vapor run`".to_owned()
                })?;
            (session.address.clone(), Some(session))
        }
    };

    let mut consumer =
        TelemetryConsumer::connect(&address).map_err(|error| format!("connect {address}: {error}"))?;
    let mut state = MonitorState::default();
    let mut last_render = Instant::now() - Duration::from_secs(1);
    state.render(session.as_ref())?;

    while let Some(event) = consumer.recv().map_err(|error| error.to_string())? {
        state.ingest(event);
        if last_render.elapsed() >= Duration::from_millis(100) {
            state.render(session.as_ref())?;
            last_render = Instant::now();
        }
    }

    state.render(session.as_ref())?;

    Ok(())
}

#[derive(Default)]
struct MonitorState {
    metrics: BTreeMap<String, f64>,
    snapshots: BTreeMap<String, Value>,
    logs: VecDeque<String>,
    lifecycle: Option<String>,
}

impl MonitorState {
    fn ingest(&mut self, event: TelemetryEvent) {
        match event.payload {
            TelemetryPayload::Log { stream, message } => {
                self.logs
                    .push_back(format!("[{}:{stream}] {message}", event.source));
                while self.logs.len() > LOG_LINES {
                    self.logs.pop_front();
                }
            }
            TelemetryPayload::Metrics { values } => {
                for (name, value) in values {
                    self.metrics
                        .insert(format!("{}.{}", event.source, name), value);
                }
            }
            TelemetryPayload::Snapshot { topic, value } => {
                self.snapshots
                    .insert(format!("{}.{}", event.source, topic), value);
                while self.snapshots.len() > SNAPSHOTS {
                    let Some(first) = self.snapshots.keys().next().cloned() else {
                        break;
                    };
                    self.snapshots.remove(&first);
                }
            }
            TelemetryPayload::Lifecycle { state, detail } => {
                self.lifecycle = Some(match detail {
                    Some(detail) => format!("{state} · {detail}"),
                    None => state,
                });
            }
        }
    }

    fn render(&self, session: Option<&ActiveDevelopmentSession>) -> Result<(), String> {
        print!("\x1b[2J\x1b[H");
        println!("VAPOR MONITOR");
        println!();

        if let Some(session) = session {
            println!("Session");
            println!("  id:            {}", session.id);
            println!("  configuration: {}", session.configuration);
            println!("  workspace:     {}", session.workspace.display());
            println!("  broker:        {}", session.address);
            if let Some(process_id) = session.process_id {
                println!("  process:       {process_id}");
            }
        }
        if let Some(lifecycle) = &self.lifecycle {
            println!("  state:         {lifecycle}");
        }

        println!();
        println!("Metrics");
        if self.metrics.is_empty() {
            println!("  waiting for metrics...");
        } else {
            for (name, value) in &self.metrics {
                println!("  {name:<52} {value:>14.3}");
            }
        }

        if !self.snapshots.is_empty() {
            println!();
            println!("Snapshots");
            for (topic, value) in &self.snapshots {
                println!("  {topic}");
                let rendered =
                    serde_json::to_string(value).unwrap_or_else(|_| "<invalid JSON>".to_owned());
                let rendered = if rendered.len() > 240 {
                    format!("{}…", &rendered[..240])
                } else {
                    rendered
                };
                println!("    {rendered}");
            }
        }

        println!();
        println!("Logs");
        if self.logs.is_empty() {
            println!("  waiting for process output...");
        } else {
            for line in &self.logs {
                println!("  {line}");
            }
        }

        io::stdout()
            .flush()
            .map_err(|error| format!("failed to flush monitor output: {error}"))
    }
}
