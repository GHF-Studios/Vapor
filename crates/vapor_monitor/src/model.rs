use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use vapor_telemetry::{TelemetryEvent, TelemetryPayload};

pub const HISTORY_POINTS: usize = 480;
const LOG_LINES: usize = 500;
const SNAPSHOTS: usize = 64;

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub configuration: String,
    pub workspace: String,
    pub address: String,
}

#[derive(Debug)]
pub enum MonitorMessage {
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
    pub last_timestamp: f64,
    pub history: VecDeque<(f64, f64)>,
}

impl MetricSeries {
    fn new(source: String, name: String, timestamp: f64, value: f64) -> Self {
        let mut history = VecDeque::new();
        history.push_back((timestamp, value));
        Self {
            source,
            name,
            latest: value,
            last_timestamp: timestamp,
            history,
        }
    }

    fn push(&mut self, timestamp: f64, value: f64) {
        self.latest = value;
        self.last_timestamp = timestamp;
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

pub struct MonitorState {
    pub status: String,
    pub session: Option<SessionInfo>,
    pub metrics: BTreeMap<String, MetricSeries>,
    pub snapshots: BTreeMap<String, SnapshotRecord>,
    pub logs: VecDeque<String>,
    pub lifecycle: BTreeMap<String, String>,
}

impl Default for MonitorState {
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

impl MonitorState {
    pub fn ingest_message(&mut self, message: MonitorMessage) {
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

    pub fn metric(&self, key: &str) -> Option<&MetricSeries> {
        self.metrics.get(key)
    }

    pub fn source_count(&self) -> usize {
        self.metrics
            .values()
            .map(|metric| metric.source.as_str())
            .chain(
                self.snapshots
                    .values()
                    .map(|snapshot| snapshot.source.as_str()),
            )
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }

    fn ingest_event(&mut self, event: TelemetryEvent) {
        let timestamp = event.timestamp_unix_ms as f64 / 1000.0;
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
                    let key = format!("{}.{}", event.source, name);
                    match self.metrics.get_mut(&key) {
                        Some(metric) => metric.push(timestamp, value),
                        None => {
                            self.metrics.insert(
                                key,
                                MetricSeries::new(event.source.clone(), name, timestamp, value),
                            );
                        }
                    }
                }
            }
            TelemetryPayload::Snapshot { topic, value } => {
                let key = format!("{}.{}", event.source, topic);
                self.snapshots.insert(
                    key,
                    SnapshotRecord {
                        source: event.source,
                        topic,
                        timestamp,
                        value,
                    },
                );
                while self.snapshots.len() > SNAPSHOTS {
                    let Some(oldest) = self
                        .snapshots
                        .iter()
                        .min_by(|left, right| left.1.timestamp.total_cmp(&right.1.timestamp))
                        .map(|(key, _)| key.clone())
                    else {
                        break;
                    };
                    self.snapshots.remove(&oldest);
                }
            }
            TelemetryPayload::Lifecycle { state, detail } => {
                let value = match detail {
                    Some(detail) => format!("{state} · {detail}"),
                    None => state,
                };
                self.lifecycle.insert(event.source, value);
            }
        }
    }
}

pub fn metric_group(metric: &MetricSeries) -> &str {
    metric.name.split('.').next().unwrap_or("other")
}

pub fn metric_label(name: &str) -> String {
    let mut parts = name.split('.');
    let first = parts.next().unwrap_or(name);
    let rest = parts.collect::<Vec<_>>();
    let label = if rest.is_empty() {
        first.to_owned()
    } else {
        rest.join(" / ")
    };
    label.replace('-', " ").replace('_', " ")
}

pub fn metric_unit(name: &str) -> &'static str {
    if name.ends_with("-ms") || name.ends_with(".ms") {
        "ms"
    } else if name.ends_with("-percent") || name.ends_with(".percent") {
        "%"
    } else if name.ends_with("-gib") || name.ends_with(".gib") {
        "GiB"
    } else if name.ends_with("-bytes") || name.ends_with(".bytes") {
        "bytes"
    } else if name.ends_with("-fps") || name.ends_with(".fps") || name == "frame.fps" {
        "fps"
    } else if name.ends_with("-hz") || name.ends_with(".hz") {
        "Hz"
    } else {
        ""
    }
}

pub fn format_metric(metric: &MetricSeries) -> String {
    let unit = metric_unit(&metric.name);
    if unit == "bytes" {
        return format_bytes(metric.latest);
    }

    let value = metric.latest;
    let precision = if value.abs() >= 1000.0 {
        0
    } else if value.abs() >= 100.0 {
        1
    } else {
        2
    };
    if unit.is_empty() {
        format!("{value:.precision$}")
    } else if unit == "%" {
        format!("{value:.precision$}{unit}")
    } else {
        format!("{value:.precision$} {unit}")
    }
}

pub fn format_bytes(bytes: f64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn every_numeric_metric_keeps_bounded_history() {
        let mut state = MonitorState::default();
        for sample in 0..(HISTORY_POINTS + 7) {
            state.ingest_message(MonitorMessage::Event(TelemetryEvent {
                timestamp_unix_ms: sample as u64 * 10,
                source: "engine".to_owned(),
                payload: TelemetryPayload::Metrics {
                    values: BTreeMap::from([(
                        "arbitrary.deep.metric-ms".to_owned(),
                        sample as f64,
                    )]),
                },
            }));
        }

        let metric = state.metric("engine.arbitrary.deep.metric-ms").unwrap();
        assert_eq!(metric.history.len(), HISTORY_POINTS);
        assert_eq!(metric.latest, (HISTORY_POINTS + 6) as f64);
        assert_eq!(metric_unit(&metric.name), "ms");
    }

    #[test]
    fn lifecycle_is_kept_per_source() {
        let mut state = MonitorState::default();
        for source in ["engine", "server"] {
            state.ingest_message(MonitorMessage::Event(TelemetryEvent {
                timestamp_unix_ms: 0,
                source: source.to_owned(),
                payload: TelemetryPayload::Lifecycle {
                    state: "running".to_owned(),
                    detail: None,
                },
            }));
        }

        assert_eq!(state.lifecycle.len(), 2);
    }
}
