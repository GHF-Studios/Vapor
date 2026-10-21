use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use vapor_client_protocol::{ProfileReportSnapshot, TelemetryEvent, TelemetryPayload};
use vapor_console_core::{
    ConsoleField, ConsoleFilter, ConsoleHistory, ConsoleLevel, ConsoleRecord,
};

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
    Session {
        session: Option<SessionInfo>,
        status: String,
    },
    Event(TelemetryEvent),
    ProfileAnalysis(Result<ProfileReportSnapshot, String>),
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
        Self {
            source,
            name,
            latest: value,
            history,
        }
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
    pub logs: ConsoleHistory,
    pub log_filter: ConsoleFilter,
    pub lifecycle: BTreeMap<String, String>,
}

impl Default for DevtoolsState {
    fn default() -> Self {
        Self {
            status: "Starting…".to_owned(),
            session: None,
            metrics: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            logs: ConsoleHistory::new(LOG_LINES),
            log_filter: ConsoleFilter::default(),
            lifecycle: BTreeMap::new(),
        }
    }
}

impl DevtoolsState {
    pub fn ingest(&mut self, message: DevtoolsMessage) {
        match message {
            DevtoolsMessage::Session { session, status } => {
                self.status = status;
                self.session = session;
            }
            DevtoolsMessage::Event(event) => self.ingest_event(event),
            DevtoolsMessage::ProfileAnalysis(_) => {}
        }
    }

    pub fn metric(&self, key: &str) -> Option<&MetricSeries> {
        self.metrics.get(key)
    }

    fn ingest_event(&mut self, event: TelemetryEvent) {
        let timestamp = event.timestamp_unix_ms as f64 / 1000.0;
        match event.payload {
            TelemetryPayload::Log {
                stream,
                message,
                level,
                target,
                fields,
            } => {
                let mut record = ConsoleRecord::new(
                    event.timestamp_unix_ms,
                    level.unwrap_or(ConsoleLevel::System),
                    event.source,
                    message,
                )
                .field("stream", stream);
                if let Some(target) = target {
                    record.target = Some(target);
                }
                record
                    .fields
                    .extend(fields.into_iter().map(|field| ConsoleField {
                        key: field.key,
                        value: field.value,
                    }));
                self.logs.push(record);
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
                        .min_by(|a, b| a.1.timestamp.total_cmp(&b.1.timestamp))
                        .map(|(key, _)| key.clone())
                    else {
                        break;
                    };
                    self.snapshots.remove(&oldest);
                }
            }
            TelemetryPayload::Lifecycle { state, detail } => {
                self.lifecycle.insert(
                    event.source,
                    detail
                        .map(|detail| format!("{state} · {detail}"))
                        .unwrap_or(state),
                );
            }
        }
    }
}

pub fn metric_label(name: &str) -> String {
    name.split('.')
        .collect::<Vec<_>>()
        .join(" / ")
        .replace(['-', '_'], " ")
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
    } else if name.ends_with("-fps") || name.ends_with(".fps") {
        "fps"
    } else {
        ""
    }
}

pub fn format_metric(metric: &MetricSeries) -> String {
    let unit = metric_unit(&metric.name);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_log_keeps_severity_target_and_fields_structured() {
        let mut state = DevtoolsState::default();
        state.ingest_event(TelemetryEvent {
            timestamp_unix_ms: 42,
            source: "game.physics".to_owned(),
            payload: TelemetryPayload::Log {
                stream: "stderr".to_owned(),
                message: "contact resolved".to_owned(),
                level: Some(ConsoleLevel::Warn),
                target: Some("ship.3".to_owned()),
                fields: vec![ConsoleField {
                    key: "speed".to_owned(),
                    value: "147 km/s".to_owned(),
                }],
            },
        });
        let record = state.logs.records().next().expect("structured record");
        assert_eq!(record.timestamp_unix_ms, 42);
        assert_eq!(record.level, ConsoleLevel::Warn);
        assert_eq!(record.source.0, "game.physics");
        assert_eq!(record.target.as_deref(), Some("ship.3"));
        assert_eq!(record.fields[0].key, "stream");
        assert_eq!(record.fields[1].value, "147 km/s");
    }
}
