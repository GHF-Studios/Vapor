use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;
use vapor_core::{
    DeveloperEnvironment, ProfileReport, ProfileReportOptions, ProfileSort,
    ProfileTraceRecord, analyze_profile_trace, import_profile_trace,
    list_profile_traces,
};

use crate::model::{MetricSeries, DevtoolsMessage, DevtoolsState, format_metric, metric_label};

pub struct DevtoolsApp {
    receiver: Receiver<DevtoolsMessage>,
    state: DevtoolsState,
    traces: Vec<ProfileTraceRecord>,
    trace_filter: String,
    selected_trace: Option<String>,
    metric_filter: String,
    selected_metric: Option<String>,
    report: Option<ProfileReport>,
    selected_zone: Option<usize>,
    analysis_receiver: Option<Receiver<Result<ProfileReport, String>>>,
    analysis_busy: bool,
    report_top: usize,
    report_sort: ProfileSort,
    report_self_time: bool,
    report_filter: String,
    status: String,
    configured_style: bool,
}

impl DevtoolsApp {
    pub fn new(receiver: Receiver<DevtoolsMessage>) -> Self {
        Self {
            receiver,
            state: DevtoolsState::default(),
            traces: list_profile_traces().unwrap_or_default(),
            trace_filter: String::new(),
            selected_trace: None,
            metric_filter: String::new(),
            selected_metric: None,
            report: None,
            selected_zone: None,
            analysis_receiver: None,
            analysis_busy: false,
            report_top: 50,
            report_sort: ProfileSort::P95,
            report_self_time: false,
            report_filter: String::new(),
            status: String::new(),
            configured_style: false,
        }
    }

    fn drain(&mut self) {
        while let Ok(message) = self.receiver.try_recv() {
            self.state.ingest(message);
        }
        let finished = self.analysis_receiver.as_ref().and_then(|receiver| receiver.try_recv().ok());
        if let Some(result) = finished {
            self.analysis_receiver = None;
            self.analysis_busy = false;
            match result {
                Ok(report) => {
                    self.report = Some(report);
                    self.selected_zone = None;
                    self.status.clear();
                }
                Err(error) => self.status = error,
            }
        }
    }

    fn configure_style(&mut self, ctx: &egui::Context) {
        if self.configured_style { return; }
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            let mut style = (*ctx.style_of(theme)).clone();
            style.spacing.item_spacing = egui::vec2(9.0, 7.0);
            style.spacing.button_padding = egui::vec2(11.0, 6.0);
            ctx.set_style_of(theme, style);
        }
        self.configured_style = true;
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        for file in dropped {
            let path = file.path();
            if path.extension().and_then(|value| value.to_str()) != Some("tracy") { continue; }
            match import_profile_trace(&path) {
                Ok(record) => {
                    self.status = format!("Added {}", record.path.display());
                    self.traces = list_profile_traces().unwrap_or_default();
                    self.selected_trace = Some(record.id);
                    self.report = None;
                }
                Err(error) => self.status = error.to_string(),
            }
        }
    }

    fn selected_trace_record(&self) -> Option<ProfileTraceRecord> {
        let id = self.selected_trace.as_deref()?;
        self.traces.iter().find(|trace| trace.id == id).cloned()
    }

    fn draw_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.heading("Vapor Devtools");
            ui.separator();
            ui.strong(&self.state.status);
            if let Some(session) = &self.state.session {
                ui.separator();
                ui.monospace(&session.configuration);
                ui.weak("·");
                ui.monospace(&session.id);
                if session.profiling { ui.strong("PROFILING"); } else { ui.weak("telemetry only"); }
            }
            ui.separator();
            if ui.button("Open Tracy").clicked() {
                match DeveloperEnvironment::discover().and_then(|environment| environment.launch_tracy()) {
                    Ok(()) => self.status = "Opened Tracy collector.".to_owned(),
                    Err(error) => self.status = error.to_string(),
                }
            }
            if ui.button("Refresh traces").clicked() {
                self.traces = list_profile_traces().unwrap_or_default();
            }
        });
        if let Some(session) = &self.state.session {
            ui.small(format!("{} · telemetry {}", session.workspace, session.address));
        } else {
            ui.small("No active run. Drag any saved .tracy file into this window to add it to the library.");
        }
        if !self.status.is_empty() { ui.label(&self.status); }
        ui.separator();
    }

    fn draw_left(&mut self, ui: &mut egui::Ui) {
        ui.heading("Artifacts");
        ui.small("Saved Tracy captures and runtime evidence");
        ui.text_edit_singleline(&mut self.trace_filter);
        let filter = self.trace_filter.to_lowercase();
        egui::ScrollArea::vertical().id_salt("trace-library").show(ui, |ui| {
            for trace in &self.traces {
                let name = trace.display_name();
                let haystack = format!("{} {} {}", name, trace.id, trace.configuration.as_deref().unwrap_or("")).to_lowercase();
                if !filter.is_empty() && !haystack.contains(&filter) { continue; }
                let selected = self.selected_trace.as_deref() == Some(trace.id.as_str());
                let mut label = name;
                if let Some(configuration) = &trace.configuration { label.push_str(&format!("  ·  {configuration}")); }
                if !trace.exists() { label.push_str("  ·  MISSING"); }
                if ui.selectable_label(selected, label).clicked() {
                    self.selected_trace = Some(trace.id.clone());
                    self.report = None;
                    self.selected_zone = None;
                }
                ui.small(trace.path.display().to_string());
                ui.add_space(5.0);
            }
        });
    }

    fn draw_live(&mut self, ui: &mut egui::Ui) {
        ui.heading("Live runtime");
        ui.horizontal_wrapped(|ui| {
            self.metric_card(ui, "Frame", "spacetime-engine.frame.time-ms");
            self.metric_card(ui, "FPS", "spacetime-engine.frame.fps");
            self.metric_card(ui, "CPU", "spacetime-engine.system.process-cpu-percent");
            self.metric_card(ui, "RAM", "spacetime-engine.system.process-memory-gib");
        });
        ui.add_space(10.0);
        ui.columns(2, |columns| {
            self.plot_metric(&mut columns[0], "Frame time", "spacetime-engine.frame.time-ms", 230.0);
            self.plot_metric(&mut columns[1], "FPS", "spacetime-engine.frame.fps", 230.0);
        });
        ui.separator();
        ui.horizontal(|ui| { ui.heading("Metrics"); ui.text_edit_singleline(&mut self.metric_filter); });
        let filter = self.metric_filter.to_lowercase();
        egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
            for (key, metric) in &self.state.metrics {
                let haystack = format!("{key} {} {}", metric.source, metric.name).to_lowercase();
                if !filter.is_empty() && !haystack.contains(&filter) { continue; }
                let selected = self.selected_metric.as_deref() == Some(key.as_str());
                ui.horizontal(|ui| {
                    if ui.selectable_label(selected, metric_label(&metric.name)).clicked() {
                        self.selected_metric = Some(key.clone());
                    }
                    ui.monospace(format_metric(metric));
                    ui.weak(&metric.source);
                });
            }
        });
        egui::CollapsingHeader::new(format!("Logs · {} retained", self.state.logs.len())).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(180.0).stick_to_bottom(true).show(ui, |ui| {
                for line in &self.state.logs { ui.monospace(line); }
            });
        });
        egui::CollapsingHeader::new(format!("Snapshots · {}", self.state.snapshots.len())).show(ui, |ui| {
            for snapshot in self.state.snapshots.values() {
                ui.horizontal(|ui| {
                    ui.strong(&snapshot.topic);
                    ui.weak(&snapshot.source);
                    ui.monospace(format!("{:.3}", snapshot.timestamp));
                });
            }
        });
    }

    fn draw_trace(&mut self, ui: &mut egui::Ui, trace: &ProfileTraceRecord) {
        ui.horizontal_wrapped(|ui| {
            ui.heading(trace.display_name());
            ui.weak(trace.path.display().to_string());
        });

        ui.horizontal_wrapped(|ui| {
            ui.label("Top");
            ui.add(egui::DragValue::new(&mut self.report_top).range(1..=500));
            egui::ComboBox::from_label("Sort").selected_text(sort_label(self.report_sort)).show_ui(ui, |ui| {
                for sort in [ProfileSort::P95, ProfileSort::P99, ProfileSort::Median, ProfileSort::Mean, ProfileSort::Max, ProfileSort::Total, ProfileSort::Count] {
                    ui.selectable_value(&mut self.report_sort, sort, sort_label(sort));
                }
            });
            ui.checkbox(&mut self.report_self_time, "Self time");
            ui.label("Filter");
            ui.text_edit_singleline(&mut self.report_filter);
            if ui.add_enabled(!self.analysis_busy, egui::Button::new("Analyze")).clicked() {
                self.start_analysis(trace.clone());
            }
        });

        if self.analysis_busy {
            ui.horizontal(|ui| { ui.spinner(); ui.label("Analyzing saved Tracy zones…"); });
        }

        let Some(report) = &self.report else {
            ui.add_space(30.0);
            ui.weak("Analyze this capture to aggregate count, total, mean, median, p90, p95, p99 and max. If the CSV exporter is missing, repair the developer environment with `vapor toolchain repair`.");
            return;
        };

        ui.small(format!("{} occurrences · {} aggregated zones · {} timing", report.occurrence_count, report.zone_count, if report.self_time { "self" } else { "inclusive" }));
        egui::ScrollArea::both().id_salt("trace-report").show(ui, |ui| {
            egui::Grid::new("trace-report-grid").num_columns(8).striped(true).show(ui, |ui| {
                for heading in ["Zone", "Calls", "Total", "Mean", "Median", "P95", "P99", "Max"] { ui.strong(heading); }
                ui.end_row();
                for (index, zone) in report.zones.iter().enumerate() {
                    let selected = self.selected_zone == Some(index);
                    if ui.selectable_label(selected, &zone.name).clicked() { self.selected_zone = Some(index); }
                    ui.monospace(zone.count.to_string());
                    ui.monospace(format_ns(zone.total_ns));
                    ui.monospace(format_ns(zone.mean_ns));
                    ui.monospace(format_ns(zone.median_ns));
                    ui.monospace(format_ns(zone.p95_ns));
                    ui.monospace(format_ns(zone.p99_ns));
                    ui.monospace(format_ns(zone.max_ns));
                    ui.end_row();
                }
            });
        });
    }

    fn start_analysis(&mut self, trace: ProfileTraceRecord) {
        let options = ProfileReportOptions {
            top: self.report_top,
            sort: self.report_sort,
            thread_ids: Vec::new(),
            after_ms: None,
            before_ms: None,
            name_filter: (!self.report_filter.trim().is_empty()).then(|| self.report_filter.trim().to_owned()),
            self_time: self.report_self_time,
        };
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = analyze_profile_trace(&trace, &options).map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
        self.analysis_receiver = Some(receiver);
        self.analysis_busy = true;
        self.status.clear();
    }

    fn draw_right(&self, ui: &mut egui::Ui) {
        if let Some(index) = self.selected_zone
            && let Some(report) = &self.report
            && let Some(zone) = report.zones.get(index)
        {
            ui.heading("Zone inspector");
            ui.strong(&zone.name);
            ui.monospace(format!("{}:{}", zone.source, zone.line));
            ui.separator();
            stat(ui, "Calls", zone.count.to_string());
            stat(ui, "Total", format_ns(zone.total_ns));
            stat(ui, "Mean", format_ns(zone.mean_ns));
            stat(ui, "Median", format_ns(zone.median_ns));
            stat(ui, "P90", format_ns(zone.p90_ns));
            stat(ui, "P95", format_ns(zone.p95_ns));
            stat(ui, "P99", format_ns(zone.p99_ns));
            stat(ui, "Max", format_ns(zone.max_ns));
            ui.separator();
            ui.label("Threads");
            ui.monospace(zone.threads.iter().map(u64::to_string).collect::<Vec<_>>().join(", "));
            return;
        }

        let selected = self.selected_metric.as_deref().and_then(|key| self.state.metrics.get(key)).or_else(|| self.state.metric("spacetime-engine.frame.time-ms"));
        if let Some(metric) = selected {
            ui.heading("Metric inspector");
            ui.strong(metric_label(&metric.name));
            ui.monospace(format!("{}.{}", metric.source, metric.name));
            ui.heading(format_metric(metric));
            ui.small(format!("{} retained samples", metric.history.len()));
            ui.add_space(10.0);
            draw_metric_plot(ui, metric, 330.0);
        } else {
            ui.heading("Inspector");
            ui.weak("Select a live metric or analyzed zone.");
        }
    }

    fn metric_card(&self, ui: &mut egui::Ui, label: &str, key: &str) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_width(130.0);
            ui.weak(label);
            if let Some(metric) = self.state.metric(key) { ui.heading(format_metric(metric)); } else { ui.heading("—"); }
        });
    }

    fn plot_metric(&self, ui: &mut egui::Ui, title: &str, key: &str, height: f32) {
        ui.strong(title);
        if let Some(metric) = self.state.metric(key) { draw_metric_plot(ui, metric, height); }
        else { ui.weak("Waiting for telemetry…"); ui.add_space(height - 20.0); }
    }
}

impl eframe::App for DevtoolsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.configure_style(ui.ctx());
        self.drain();
        self.handle_drops(ui.ctx());
        ui.ctx().request_repaint_after(Duration::from_millis(100));

        egui::Panel::top("profile-header").show(ui, |ui| self.draw_header(ui));
        egui::Panel::left("trace-library").resizable(true).default_size(300.0).min_size(230.0).show(ui, |ui| self.draw_left(ui));
        egui::Panel::right("profile-inspector").resizable(true).default_size(310.0).min_size(240.0).show(ui, |ui| self.draw_right(ui));
        egui::CentralPanel::default().show_inside(ui, |ui| {
            if let Some(trace) = self.selected_trace_record() { self.draw_trace(ui, &trace); }
            else { self.draw_live(ui); }
        });
    }
}

fn draw_metric_plot(ui: &mut egui::Ui, metric: &MetricSeries, height: f32) {
    let Some((start, _)) = metric.history.front().copied() else { ui.weak("Waiting for samples…"); return; };
    let points = PlotPoints::from_iter(metric.history.iter().map(|(time, value)| [time - start, *value]));
    Plot::new(format!("metric-plot-{}-{}", metric.source, metric.name)).height(height).allow_drag(true).allow_zoom(true).show(ui, |plot_ui| {
        plot_ui.line(Line::new(metric_label(&metric.name), points));
    });
}

fn stat(ui: &mut egui::Ui, label: &str, value: String) {
    ui.horizontal(|ui| { ui.weak(label); ui.monospace(value); });
}

fn sort_label(sort: ProfileSort) -> &'static str {
    match sort {
        ProfileSort::Total => "Total",
        ProfileSort::Count => "Count",
        ProfileSort::Mean => "Mean",
        ProfileSort::Median => "Median",
        ProfileSort::P90 => "P90",
        ProfileSort::P95 => "P95",
        ProfileSort::P99 => "P99",
        ProfileSort::Max => "Max",
    }
}

fn format_ns(ns: u64) -> String {
    if ns >= 1_000_000_000 { format!("{:.2}s", ns as f64 / 1_000_000_000.0) }
    else if ns >= 1_000_000 { format!("{:.2}ms", ns as f64 / 1_000_000.0) }
    else if ns >= 1_000 { format!("{:.1}µs", ns as f64 / 1_000.0) }
    else { format!("{ns}ns") }
}
