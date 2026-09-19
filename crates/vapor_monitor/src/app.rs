use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use crate::model::{
    MetricSeries, MonitorMessage, MonitorState, format_metric, metric_group, metric_label,
    metric_unit,
};

const OVERVIEW_METRICS: &[(&str, &str)] = &[
    ("FPS", "spacetime-engine.frame.fps"),
    ("Frame time", "spacetime-engine.frame.time-ms"),
    (
        "Narrow max",
        "spacetime-engine.physics.collision.narrow-phase.step-max-ms",
    ),
    (
        "Narrow / frame",
        "spacetime-engine.physics.collision.narrow-phase.last-frame-ms",
    ),
    (
        "Solver max",
        "spacetime-engine.physics.solver.total.step-max-ms",
    ),
    (
        "Physics steps / frame",
        "spacetime-engine.physics.steps-per-frame.last",
    ),
    (
        "Active pairs",
        "spacetime-engine.physics.contact-pairs.active",
    ),
    ("Dynamic bodies", "spacetime-engine.physics.bodies.dynamic"),
    ("Process CPU", "spacetime-engine.system.process-cpu-percent"),
    ("Process RAM", "spacetime-engine.system.process-memory-gib"),
];

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum MonitorTab {
    #[default]
    Overview,
    Metrics,
    Snapshots,
    Logs,
}

pub struct MonitorApp {
    receiver: Receiver<MonitorMessage>,
    state: MonitorState,
    tab: MonitorTab,
    metric_filter: String,
    selected_metric: Option<String>,
    snapshot_filter: String,
    selected_snapshot: Option<String>,
    log_filter: String,
    configured_style: bool,
}

impl MonitorApp {
    pub fn new(receiver: Receiver<MonitorMessage>) -> Self {
        Self {
            receiver,
            state: MonitorState::default(),
            tab: MonitorTab::default(),
            metric_filter: String::new(),
            selected_metric: None,
            snapshot_filter: String::new(),
            selected_snapshot: None,
            log_filter: String::new(),
            configured_style: false,
        }
    }

    fn drain_messages(&mut self) {
        while let Ok(message) = self.receiver.try_recv() {
            self.state.ingest_message(message);
        }
    }

    fn configure_style(&mut self, ctx: &egui::Context) {
        if self.configured_style {
            return;
        }
        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        ctx.set_style(style);
        self.configured_style = true;
    }

    fn draw_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.heading("Vapor Monitor");
            ui.separator();
            ui.strong(&self.state.status);
            ui.separator();
            ui.weak(format!(
                "{} metrics · {} snapshots · {} sources",
                self.state.metrics.len(),
                self.state.snapshots.len(),
                self.state.source_count()
            ));
        });

        if let Some(session) = &self.state.session {
            ui.horizontal_wrapped(|ui| {
                ui.label(&session.configuration);
                ui.weak("·");
                ui.label(&session.workspace);
                ui.weak("·");
                ui.monospace(&session.id);
            });
            ui.small(format!("telemetry: {}", session.address));
        }

        if !self.state.lifecycle.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for (source, lifecycle) in &self.state.lifecycle {
                    ui.weak(format!("{source}: {lifecycle}"));
                }
            });
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, MonitorTab::Overview, "Overview");
            ui.selectable_value(&mut self.tab, MonitorTab::Metrics, "Metrics");
            ui.selectable_value(&mut self.tab, MonitorTab::Snapshots, "Snapshots");
            ui.selectable_value(&mut self.tab, MonitorTab::Logs, "Logs");
        });
        ui.separator();
    }

    fn draw_overview(&mut self, ui: &mut egui::Ui) {
        ui.label("Live runtime vitals");
        ui.horizontal_wrapped(|ui| {
            for (label, key) in OVERVIEW_METRICS {
                self.metric_card(ui, label, key);
            }
        });

        ui.add_space(10.0);
        let plots = [
            ("Frame time", "spacetime-engine.frame.time-ms"),
            (
                "Narrow phase / step max",
                "spacetime-engine.physics.collision.narrow-phase.step-max-ms",
            ),
            (
                "Solver / step max",
                "spacetime-engine.physics.solver.total.step-max-ms",
            ),
            (
                "Physics steps / frame",
                "spacetime-engine.physics.steps-per-frame.last",
            ),
            (
                "Active contact pairs",
                "spacetime-engine.physics.contact-pairs.active",
            ),
            ("FPS", "spacetime-engine.frame.fps"),
        ];
        for chunk in plots.chunks(2) {
            ui.columns(chunk.len(), |columns| {
                for (column, (title, key)) in columns.iter_mut().zip(chunk.iter()) {
                    self.plot_metric(column, title, key, 170.0);
                }
            });
        }

        if self
            .state
            .snapshots
            .contains_key("spacetime-engine.physics.contact-pairs")
        {
            ui.separator();
            egui::CollapsingHeader::new("Physics contact pairs")
                .default_open(true)
                .show(ui, |ui| {
                    if let Some(snapshot) = self
                        .state
                        .snapshots
                        .get("spacetime-engine.physics.contact-pairs")
                    {
                        draw_json_value(ui, "overview-contact-pairs", &snapshot.value, 0);
                    }
                });
        }
    }

    fn metric_card(&self, ui: &mut egui::Ui, label: &str, key: &str) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_width(132.0);
            ui.weak(label);
            match self.state.metric(key) {
                Some(metric) => {
                    ui.heading(format_metric(metric));
                    ui.small(metric.name.as_str());
                }
                None => {
                    ui.heading("—");
                    ui.small("waiting for samples");
                }
            }
        });
    }

    fn draw_metrics(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.text_edit_singleline(&mut self.metric_filter);
            if ui.button("Clear").clicked() {
                self.metric_filter.clear();
            }
        });

        let filter = self.metric_filter.to_lowercase();
        let mut grouped = BTreeMap::<(String, String), Vec<(String, String, String)>>::new();
        for (key, metric) in &self.state.metrics {
            let haystack = format!("{} {} {}", key, metric.source, metric.name).to_lowercase();
            if !filter.is_empty() && !haystack.contains(&filter) {
                continue;
            }
            grouped
                .entry((metric.source.clone(), metric_group(metric).to_owned()))
                .or_default()
                .push((
                    key.clone(),
                    metric_label(&metric.name),
                    format_metric(metric),
                ));
        }

        ui.columns(2, |columns| {
            egui::ScrollArea::vertical()
                .id_salt("metric-browser")
                .show(&mut columns[0], |ui| {
                    for ((source, group), metrics) in grouped {
                        egui::CollapsingHeader::new(format!("{source} / {group}"))
                            .default_open(group == "physics" || group == "frame")
                            .show(ui, |ui| {
                                egui::Grid::new(format!("metric-grid-{source}-{group}"))
                                    .num_columns(2)
                                    .striped(true)
                                    .show(ui, |ui| {
                                        for (key, label, value) in metrics {
                                            let selected = self.selected_metric.as_deref()
                                                == Some(key.as_str());
                                            if ui.selectable_label(selected, label).clicked() {
                                                self.selected_metric = Some(key);
                                            }
                                            ui.monospace(value);
                                            ui.end_row();
                                        }
                                    });
                            });
                    }
                });

            columns[1].vertical(|ui| {
                let selected = self
                    .selected_metric
                    .as_deref()
                    .and_then(|key| self.state.metrics.get(key))
                    .or_else(|| self.state.metrics.values().next())
                    .cloned();
                let Some(metric) = selected else {
                    ui.weak("Waiting for telemetry metrics…");
                    return;
                };

                ui.heading(metric_label(&metric.name));
                ui.monospace(format!("{}.{}", metric.source, metric.name));
                ui.heading(format_metric(&metric));
                ui.small(format!("{} retained samples", metric.history.len()));
                ui.add_space(8.0);
                draw_metric_plot(ui, &metric, 280.0);
            });
        });
    }

    fn draw_snapshots(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.text_edit_singleline(&mut self.snapshot_filter);
            if ui.button("Clear").clicked() {
                self.snapshot_filter.clear();
            }
        });

        let filter = self.snapshot_filter.to_lowercase();
        let visible = self
            .state
            .snapshots
            .iter()
            .filter(|(key, snapshot)| {
                filter.is_empty()
                    || key.to_lowercase().contains(&filter)
                    || snapshot.topic.to_lowercase().contains(&filter)
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();

        ui.columns(2, |columns| {
            egui::ScrollArea::vertical()
                .id_salt("snapshot-browser")
                .show(&mut columns[0], |ui| {
                    for key in &visible {
                        let Some(snapshot) = self.state.snapshots.get(key) else {
                            continue;
                        };
                        let label = format!("{} / {}", snapshot.source, snapshot.topic);
                        let selected = self.selected_snapshot.as_deref() == Some(key.as_str());
                        if ui.selectable_label(selected, label).clicked() {
                            self.selected_snapshot = Some(key.clone());
                        }
                    }
                });

            columns[1].vertical(|ui| {
                let selected = self
                    .selected_snapshot
                    .as_deref()
                    .and_then(|key| self.state.snapshots.get(key))
                    .or_else(|| {
                        visible
                            .first()
                            .and_then(|key| self.state.snapshots.get(key))
                    })
                    .cloned();
                let Some(snapshot) = selected else {
                    ui.weak("Waiting for telemetry snapshots…");
                    return;
                };

                ui.heading(&snapshot.topic);
                ui.monospace(&snapshot.source);
                ui.small(format!("sample timestamp {:.3}", snapshot.timestamp));
                ui.separator();
                egui::ScrollArea::both()
                    .id_salt("snapshot-value")
                    .show(ui, |ui| {
                        draw_json_value(ui, &snapshot.topic, &snapshot.value, 0);
                    });
            });
        });
    }

    fn draw_logs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.text_edit_singleline(&mut self.log_filter);
            if ui.button("Clear").clicked() {
                self.log_filter.clear();
            }
        });
        let filter = self.log_filter.to_lowercase();
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for line in &self.state.logs {
                    if filter.is_empty() || line.to_lowercase().contains(&filter) {
                        ui.monospace(line);
                    }
                }
            });
    }

    fn plot_metric(&self, ui: &mut egui::Ui, title: &str, key: &str, height: f32) {
        ui.label(title);
        let Some(metric) = self.state.metric(key) else {
            ui.weak("waiting for samples…");
            ui.add_space(height - 22.0);
            return;
        };
        draw_metric_plot(ui, metric, height);
    }
}

impl eframe::App for MonitorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.configure_style(ui.ctx());
        self.drain_messages();
        ui.ctx().request_repaint_after(Duration::from_millis(100));

        self.draw_header(ui);
        match self.tab {
            MonitorTab::Overview => self.draw_overview(ui),
            MonitorTab::Metrics => self.draw_metrics(ui),
            MonitorTab::Snapshots => self.draw_snapshots(ui),
            MonitorTab::Logs => self.draw_logs(ui),
        }
    }
}

fn draw_metric_plot(ui: &mut egui::Ui, metric: &MetricSeries, height: f32) {
    let Some((start, _)) = metric.history.front().copied() else {
        ui.weak("waiting for samples…");
        return;
    };
    let points = PlotPoints::from_iter(
        metric
            .history
            .iter()
            .map(|(time, value)| [time - start, *value]),
    );
    let unit = metric_unit(&metric.name);
    let legend = if unit.is_empty() {
        metric_label(&metric.name)
    } else {
        format!("{} ({unit})", metric_label(&metric.name))
    };
    Plot::new(format!("metric-plot-{}-{}", metric.source, metric.name))
        .height(height)
        .allow_drag(true)
        .allow_zoom(true)
        .show(ui, |plot_ui| {
            plot_ui.line(Line::new(legend, points));
        });
}

fn draw_json_value(ui: &mut egui::Ui, id: &str, value: &Value, depth: usize) {
    if depth > 8 {
        ui.weak("…");
        return;
    }

    match value {
        Value::Null => {
            ui.weak("null");
        }
        Value::Bool(value) => {
            ui.monospace(value.to_string());
        }
        Value::Number(value) => {
            ui.monospace(value.to_string());
        }
        Value::String(value) => {
            ui.monospace(value);
        }
        Value::Array(values) => {
            if let Some(columns) = tabular_columns(values) {
                draw_json_table(ui, id, values, &columns);
            } else {
                for (index, value) in values.iter().enumerate() {
                    egui::CollapsingHeader::new(format!("[{index}]"))
                        .default_open(index < 4)
                        .show(ui, |ui| {
                            draw_json_value(ui, &format!("{id}-{index}"), value, depth + 1);
                        });
                }
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                match value {
                    Value::Array(_) | Value::Object(_) => {
                        egui::CollapsingHeader::new(key)
                            .default_open(depth < 2)
                            .show(ui, |ui| {
                                draw_json_value(ui, &format!("{id}-{key}"), value, depth + 1);
                            });
                    }
                    _ => {
                        ui.horizontal(|ui| {
                            ui.weak(key);
                            draw_json_value(ui, &format!("{id}-{key}"), value, depth + 1);
                        });
                    }
                }
            }
        }
    }
}

fn tabular_columns(values: &[Value]) -> Option<Vec<String>> {
    if values.is_empty() || values.len() > 512 {
        return None;
    }
    let objects = values
        .iter()
        .map(Value::as_object)
        .collect::<Option<Vec<_>>>()?;
    let first = objects.first()?;
    if first.is_empty() || first.len() > 18 {
        return None;
    }
    let columns = first.keys().cloned().collect::<Vec<_>>();
    let scalar = |value: &Value| {
        matches!(
            value,
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
        )
    };
    if objects.iter().all(|object| {
        columns
            .iter()
            .all(|column| object.get(column).is_some_and(scalar))
    }) {
        Some(columns)
    } else {
        None
    }
}

fn draw_json_table(ui: &mut egui::Ui, id: &str, values: &[Value], columns: &[String]) {
    egui::ScrollArea::horizontal().show(ui, |ui| {
        egui::Grid::new(format!("json-table-{id}"))
            .striped(true)
            .show(ui, |ui| {
                for column in columns {
                    ui.strong(column);
                }
                ui.end_row();

                for value in values {
                    let Some(object) = value.as_object() else {
                        continue;
                    };
                    for column in columns {
                        draw_json_scalar(ui, object, column);
                    }
                    ui.end_row();
                }
            });
    });
}

fn draw_json_scalar(ui: &mut egui::Ui, object: &Map<String, Value>, column: &str) {
    let value = object.get(column).unwrap_or(&Value::Null);
    match value {
        Value::Null => {
            ui.weak("—");
        }
        Value::Bool(value) => {
            ui.monospace(value.to_string());
        }
        Value::Number(value) => {
            ui.monospace(value.to_string());
        }
        Value::String(value) => {
            ui.monospace(value);
        }
        Value::Array(_) | Value::Object(_) => {
            ui.weak("…");
        }
    }
}
