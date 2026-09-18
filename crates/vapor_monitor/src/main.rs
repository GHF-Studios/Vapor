use clap::Parser;
use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;
use vapor_core::active_development_session;
use vapor_telemetry::{TelemetryConsumer, TelemetryEvent, TelemetryPayload};

const HISTORY_POINTS: usize = 480;
const LOG_LINES: usize = 80;
const SNAPSHOTS: usize = 8;

#[derive(Debug, Parser)]
#[command(name = "vapor-monitor", about = "Live Vapor Development Session dashboard")]
struct Cli {
    /// Connect directly to a telemetry broker instead of following the active Vapor session.
    #[arg(long)]
    address: Option<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let (sender, receiver) = mpsc::channel();
    spawn_connection_worker(cli.address, sender);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Vapor Monitor")
            .with_inner_size([980.0, 700.0])
            .with_min_inner_size([680.0, 460.0]),
        ..Default::default()
    };

    match eframe::run_native(
        "vapor-monitor",
        options,
        Box::new(move |_creation| Ok(Box::new(MonitorApp::new(receiver)))),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("vapor-monitor: {error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Clone)]
struct SessionInfo {
    id: String,
    configuration: String,
    workspace: String,
    address: String,
}

#[derive(Debug)]
enum MonitorMessage {
    Waiting(String),
    Connected(Option<SessionInfo>),
    Event(TelemetryEvent),
    Disconnected(String),
}

fn spawn_connection_worker(explicit_address: Option<String>, sender: Sender<MonitorMessage>) {
    thread::spawn(move || {
        let mut last_waiting = String::new();
        loop {
            let (address, session) = match explicit_address.as_ref() {
                Some(address) => (address.clone(), None),
                None => match active_development_session() {
                    Ok(Some(session)) => {
                        let info = SessionInfo {
                            id: session.id.clone(),
                            configuration: session.configuration.clone(),
                            workspace: session.workspace.display().to_string(),
                            address: session.address.clone(),
                        };
                        (session.address, Some(info))
                    }
                    Ok(None) => {
                        let message = "Waiting for `vapor run`…".to_owned();
                        if message != last_waiting {
                            let _ = sender.send(MonitorMessage::Waiting(message.clone()));
                            last_waiting = message;
                        }
                        thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                    Err(error) => {
                        let message = format!("Waiting for Vapor session state: {error}");
                        if message != last_waiting {
                            let _ = sender.send(MonitorMessage::Waiting(message.clone()));
                            last_waiting = message;
                        }
                        thread::sleep(Duration::from_secs(1));
                        continue;
                    }
                },
            };

            match TelemetryConsumer::connect(&address) {
                Ok(mut consumer) => {
                    last_waiting.clear();
                    if sender.send(MonitorMessage::Connected(session)).is_err() {
                        return;
                    }
                    loop {
                        match consumer.recv() {
                            Ok(Some(event)) => {
                                if sender.send(MonitorMessage::Event(event)).is_err() {
                                    return;
                                }
                            }
                            Ok(None) => break,
                            Err(error) => {
                                let _ = sender.send(MonitorMessage::Disconnected(format!(
                                    "Telemetry disconnected: {error}"
                                )));
                                break;
                            }
                        }
                    }
                }
                Err(error) => {
                    let message = format!("Waiting for telemetry at {address}: {error}");
                    if message != last_waiting {
                        let _ = sender.send(MonitorMessage::Waiting(message.clone()));
                        last_waiting = message;
                    }
                }
            }

            thread::sleep(Duration::from_millis(500));
        }
    });
}

struct MonitorApp {
    receiver: Receiver<MonitorMessage>,
    state: MonitorState,
}

impl MonitorApp {
    fn new(receiver: Receiver<MonitorMessage>) -> Self {
        Self {
            receiver,
            state: MonitorState::default(),
        }
    }

    fn drain_messages(&mut self) {
        while let Ok(message) = self.receiver.try_recv() {
            self.state.ingest_message(message);
        }
    }
}

impl eframe::App for MonitorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_messages();
        ui.ctx().request_repaint_after(Duration::from_millis(100));

        ui.horizontal(|ui| {
            ui.heading("Vapor Monitor");
            ui.separator();
            ui.label(&self.state.status);
        });
        if let Some(session) = &self.state.session {
            ui.label(format!(
                "{} · {} · {}",
                session.configuration, session.workspace, session.id
            ));
            ui.small(format!("telemetry: {}", session.address));
        }
        ui.separator();

        egui::Grid::new("headline_metrics")
            .num_columns(4)
            .spacing([18.0, 8.0])
            .show(ui, |ui| {
                self.state.metric_cell(ui, "FPS", "spacetime-engine.frame.fps", "");
                self.state.metric_cell(
                    ui,
                    "Frame",
                    "spacetime-engine.frame.time-ms",
                    " ms",
                );
                self.state.metric_cell(
                    ui,
                    "1% low",
                    "spacetime-engine.frame.one-percent-low-fps",
                    " fps",
                );
                self.state.metric_cell(
                    ui,
                    "Process CPU",
                    "spacetime-engine.system.process-cpu-percent",
                    "%",
                );
                ui.end_row();
                self.state.metric_cell(
                    ui,
                    "Process RAM",
                    "spacetime-engine.system.process-memory-gib",
                    " GiB",
                );
                self.state.metric_bytes_cell(
                    ui,
                    "ECS inline",
                    "spacetime-engine.ecs.inline-component-bytes",
                );
                self.state.metric_cell(
                    ui,
                    "Entities",
                    "spacetime-engine.world.entities",
                    "",
                );
                self.state.metric_cell(
                    ui,
                    "Components",
                    "spacetime-engine.world.component-instances",
                    "",
                );
                ui.end_row();
            });

        ui.add_space(8.0);
        ui.columns(2, |columns| {
            self.state.plot_metric(
                &mut columns[0],
                "Frame time",
                "spacetime-engine.frame.time-ms",
                "ms",
            );
            self.state.plot_metric(
                &mut columns[1],
                "Process CPU",
                "spacetime-engine.system.process-cpu-percent",
                "%",
            );
        });
        ui.columns(2, |columns| {
            self.state.plot_metric(
                &mut columns[0],
                "FPS",
                "spacetime-engine.frame.fps",
                "fps",
            );
            self.state.plot_metric(
                &mut columns[1],
                "Process RAM",
                "spacetime-engine.system.process-memory-gib",
                "GiB",
            );
        });

        ui.separator();
        egui::CollapsingHeader::new("ECS component memory")
            .default_open(true)
            .show(ui, |ui| self.state.draw_component_memory(ui));
        egui::CollapsingHeader::new("Logs")
            .default_open(false)
            .show(ui, |ui| self.state.draw_logs(ui));
    }
}

#[derive(Default)]
struct MonitorState {
    status: String,
    session: Option<SessionInfo>,
    metrics: BTreeMap<String, f64>,
    histories: BTreeMap<String, VecDeque<(f64, f64)>>,
    snapshots: BTreeMap<String, Value>,
    logs: VecDeque<String>,
    lifecycle: Option<String>,
}

impl MonitorState {
    fn ingest_message(&mut self, message: MonitorMessage) {
        match message {
            MonitorMessage::Waiting(status) => self.status = status,
            MonitorMessage::Connected(session) => {
                self.status = "Live".to_owned();
                self.session = session;
            }
            MonitorMessage::Disconnected(status) => self.status = status,
            MonitorMessage::Event(event) => self.ingest_event(event),
        }
    }

    fn ingest_event(&mut self, event: TelemetryEvent) {
        match event.payload {
            TelemetryPayload::Log { stream, message } => {
                self.logs
                    .push_back(format!("[{}:{stream}] {message}", event.source));
                while self.logs.len() > LOG_LINES {
                    self.logs.pop_front();
                }
            }
            TelemetryPayload::Metrics { values } => {
                let timestamp = event.timestamp_unix_ms as f64 / 1000.0;
                for (name, value) in values {
                    let key = format!("{}.{}", event.source, name);
                    self.metrics.insert(key.clone(), value);
                    if tracked_history(&name) {
                        let history = self.histories.entry(key).or_default();
                        history.push_back((timestamp, value));
                        while history.len() > HISTORY_POINTS {
                            history.pop_front();
                        }
                    }
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

    fn metric_cell(&self, ui: &mut egui::Ui, label: &str, key: &str, suffix: &str) {
        ui.vertical(|ui| {
            ui.weak(label);
            match self.metrics.get(key) {
                Some(value) => ui.strong(format!("{value:.2}{suffix}")),
                None => ui.strong("—"),
            };
        });
    }

    fn metric_bytes_cell(&self, ui: &mut egui::Ui, label: &str, key: &str) {
        ui.vertical(|ui| {
            ui.weak(label);
            match self.metrics.get(key) {
                Some(value) => ui.strong(format_bytes(*value)),
                None => ui.strong("—"),
            };
        });
    }

    fn plot_metric(&self, ui: &mut egui::Ui, title: &str, key: &str, unit: &str) {
        ui.label(title);
        let Some(history) = self.histories.get(key) else {
            ui.weak("waiting for samples…");
            ui.add_space(130.0);
            return;
        };
        let Some((start, _)) = history.front().copied() else {
            return;
        };
        let points = PlotPoints::from_iter(history.iter().map(|(time, value)| [time - start, *value]));
        Plot::new(key)
            .height(150.0)
            .allow_drag(true)
            .allow_zoom(true)
            .show(ui, |plot_ui| {
                plot_ui.line(Line::new(format!("{title} ({unit})"), points));
            });
    }

    fn draw_component_memory(&self, ui: &mut egui::Ui) {
        let Some(Value::Array(components)) = self
            .snapshots
            .get("spacetime-engine.ecs.component-memory")
        else {
            ui.weak("waiting for ECS memory snapshot…");
            return;
        };

        egui::Grid::new("component_memory")
            .striped(true)
            .show(ui, |ui| {
                ui.strong("Component");
                ui.strong("Instances");
                ui.strong("Inline payload");
                ui.end_row();
                for component in components.iter().take(16) {
                    let name = component.get("name").and_then(Value::as_str).unwrap_or("?");
                    let instances = component
                        .get("instances")
                        .and_then(Value::as_u64)
                        .unwrap_or_default();
                    let bytes = component
                        .get("inline_bytes")
                        .and_then(Value::as_u64)
                        .unwrap_or_default() as f64;
                    ui.label(name);
                    ui.label(instances.to_string());
                    ui.label(format_bytes(bytes));
                    ui.end_row();
                }
            });
    }

    fn draw_logs(&self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
            for line in &self.logs {
                ui.monospace(line);
            }
        });
    }
}

fn tracked_history(name: &str) -> bool {
    matches!(
        name,
        "frame.fps"
            | "frame.time-ms"
            | "frame.average-time-ms"
            | "frame.one-percent-low-fps"
            | "system.process-cpu-percent"
            | "system.process-memory-gib"
    )
}

fn format_bytes(bytes: f64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}
