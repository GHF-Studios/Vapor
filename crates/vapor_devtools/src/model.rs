use std::collections::{BTreeMap, VecDeque};
use serde_json::Value;
use vapor_telemetry::{TelemetryEvent, TelemetryPayload};

pub const HISTORY_POINTS: usize = 900;
const LOG_LINES: usize = 1200;
const SNAPSHOTS: usize = 128;

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub configuration: String,
    pub workspace: String,
    pub address: String,
    pub profiling: bool,
}

#[derive(Debug)]
pub enum DevtoolsMessage {
    Waiting(String),
    Connected(Option<SessionInfo>),
    Event(TelemetryEvent),
    Disconnected(String),
}

#[derive(Debug, Clone)]
pub struct MetricSeries {
    pub source: String,
    pub name: String,
    pub latest: f64,
    pub history: VecDeque<(f64, f64)>,
}

impl MetricSeries {
    fn new(source: String, name: String, timestamp: f64, value: f64) -> Self {
        let mut history = VecDeque::new();
        history.push_back((timestamp, value));
        Self { source, name, latest: value, history }
    }

    fn push(&mut self, timestamp: f64, value: f64) {
        self.latest = value;
        self.history.push_back((timestamp, value));
        while self.history.len() > HISTORY_POINTS {
            self.history.pop_front();
        }
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotRecord {
    pub source: String,
    pub topic: String,
    pub timestamp: f64,
    pub value: Value,
}

pub struct DevtoolsState {
    pub status: String,
    pub session: Option<SessionInfo>,
    pub metrics: BTreeMap<String, MetricSeries>,
    pub snapshots: BTreeMap<String, SnapshotRecord>,
    pub logs: VecDeque<String>,
    pub lifecycle: BTreeMap<String, String>,
}

impl Default for DevtoolsState {
    fn default() -> Self {
        Self {
            status: "Starting…".to_owned(),
            session: None,
            metrics: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            logs: VecDeque::new(),
            lifecycle: BTreeMap::new(),
        }
    }
}

impl DevtoolsState {
    pub fn ingest(&mut self, message: DevtoolsMessage) {
        match message {
            DevtoolsMessage::Waiting(status) => self.status = status,
            DevtoolsMessage::Connected(session) => {
                self.status = "Live".to_owned();
                self.session = session;
            }
            DevtoolsMessage::Disconnected(status) => self.status = status,
            DevtoolsMessage::Event(event) => self.ingest_event(event),
        }
    }

    pub fn metric(&self, key: &str) -> Option<&MetricSeries> {
        self.metrics.get(key)
    }

    fn ingest_event(&mut self, event: TelemetryEvent) {
        let timestamp = event.timestamp_unix_ms as f64 / 1000.0;
        match event.payload {
            TelemetryPayload::Log { stream, message } => {
                self.logs.push_back(format!("[{}:{stream}] {message}", event.source));
                while self.logs.len() > LOG_LINES { self.logs.pop_front(); }
            }
            TelemetryPayload::Metrics { values } => {
                for (name, value) in values {
                    let key = format!("{}.{}", event.source, name);
                    match self.metrics.get_mut(&key) {
                        Some(metric) => metric.push(timestamp, value),
                        None => {
                            self.metrics.insert(key, MetricSeries::new(event.source.clone(), name, timestamp, value));
                        }
                    }
                }
            }
            TelemetryPayload::Snapshot { topic, value } => {
                let key = format!("{}.{}", event.source, topic);
                self.snapshots.insert(key, SnapshotRecord { source: event.source, topic, timestamp, value });
                while self.snapshots.len() > SNAPSHOTS {
                    let Some(oldest) = self.snapshots.iter().min_by(|a, b| a.1.timestamp.total_cmp(&b.1.timestamp)).map(|(key, _)| key.clone()) else { break; };
                    self.snapshots.remove(&oldest);
                }
            }
            TelemetryPayload::Lifecycle { state, detail } => {
                self.lifecycle.insert(event.source, detail.map(|detail| format!("{state} · {detail}")).unwrap_or(state));
            }
        }
    }
}

pub fn metric_label(name: &str) -> String {
    name.split('.').collect::<Vec<_>>().join(" / ").replace('-', " ").replace('_', " ")
}

pub fn metric_unit(name: &str) -> &'static str {
    if name.ends_with("-ms") || name.ends_with(".ms") { "ms" }
    else if name.ends_with("-percent") || name.ends_with(".percent") { "%" }
    else if name.ends_with("-gib") || name.ends_with(".gib") { "GiB" }
    else if name.ends_with("-bytes") || name.ends_with(".bytes") { "bytes" }
    else if name.ends_with("-fps") || name.ends_with(".fps") { "fps" }
    else { "" }
}

pub fn format_metric(metric: &MetricSeries) -> String {
    let unit = metric_unit(&metric.name);
    let value = metric.latest;
    let precision = if value.abs() >= 1000.0 { 0 } else if value.abs() >= 100.0 { 1 } else { 2 };
    if unit.is_empty() { format!("{value:.precision$}") }
    else if unit == "%" { format!("{value:.precision$}{unit}") }
    else { format!("{value:.precision$} {unit}") }
}
