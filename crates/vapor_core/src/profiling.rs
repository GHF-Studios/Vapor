//! Saved Tracy trace library and statistical analysis.
//!
//! Tracy remains the collector and native deep viewer. Vapor owns an indexed
//! library of user-saved traces, source-layout context and repeatable reports.

use crate::{
    TracyError, VaporInstallation, active_development_session,
    resolve_tracy_csvexport, resolve_tracy_profiler,
};
use csv::ReaderBuilder;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const PROFILE_DIR: &str = "profiling";
const LIBRARY_FILE: &str = "trace-library.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileTraceRecord {
    pub id: String,
    pub path: PathBuf,
    pub added_unix_ms: u64,
    pub workspace: Option<PathBuf>,
    pub session_id: Option<String>,
    pub configuration: Option<String>,
}

impl ProfileTraceRecord {
    pub fn display_name(&self) -> String {
        self.path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&self.id)
            .to_owned()
    }

    pub fn exists(&self) -> bool {
        self.path.is_file()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSort {
    Total,
    Count,
    Mean,
    Median,
    P90,
    P95,
    P99,
    Max,
}

impl Default for ProfileSort {
    fn default() -> Self {
        Self::P95
    }
}

#[derive(Debug, Clone)]
pub struct ProfileReportOptions {
    pub top: usize,
    pub sort: ProfileSort,
    pub thread_ids: Vec<u64>,
    pub after_ms: Option<f64>,
    pub before_ms: Option<f64>,
    pub name_filter: Option<String>,
    pub self_time: bool,
}

impl Default for ProfileReportOptions {
    fn default() -> Self {
        Self {
            top: 50,
            sort: ProfileSort::P95,
            thread_ids: Vec::new(),
            after_ms: None,
            before_ms: None,
            name_filter: None,
            self_time: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileReport {
    pub trace: PathBuf,
    pub self_time: bool,
    pub occurrence_count: usize,
    pub zone_count: usize,
    pub zones: Vec<ProfileZoneSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileZoneSummary {
    pub name: String,
    pub source: String,
    pub line: u32,
    pub threads: Vec<u64>,
    pub count: usize,
    pub total_ns: u64,
    pub mean_ns: u64,
    pub median_ns: u64,
    pub p90_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
}

#[derive(Debug, Deserialize)]
struct TracyZoneOccurrence {
    name: String,
    src_file: String,
    src_line: u32,
    ns_since_start: i64,
    exec_time_ns: i64,
    thread: u64,
    #[serde(default)]
    value: String,
}

#[derive(Default)]
struct Aggregate {
    durations: Vec<u64>,
    threads: BTreeSet<u64>,
}

pub fn profile_library_path() -> Result<PathBuf, ProfileError> {
    let installation = VaporInstallation::discover()?;
    Ok(installation.user_data_root().join(PROFILE_DIR).join(LIBRARY_FILE))
}

pub fn list_profile_traces() -> Result<Vec<ProfileTraceRecord>, ProfileError> {
    let path = profile_library_path()?;
    let mut records = match fs::read_to_string(&path) {
        Ok(source) => serde_json::from_str::<Vec<ProfileTraceRecord>>(&source)
            .map_err(|source| ProfileError::LibraryMetadata {
                path: path.clone(),
                source,
            })?,
        Err(source) if source.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(source) => return Err(ProfileError::Io { path, source }),
    };

    records.sort_by_key(|record| std::cmp::Reverse(record.added_unix_ms));
    Ok(records)
}

pub fn import_profile_trace(source: &Path) -> Result<ProfileTraceRecord, ProfileError> {
    let source = fs::canonicalize(source).map_err(|error| ProfileError::Io {
        path: source.to_path_buf(),
        source: error,
    })?;

    if source.extension().and_then(|value| value.to_str()) != Some("tracy") {
        return Err(ProfileError::message(format!(
            "`{}` is not a .tracy capture",
            source.display()
        )));
    }

    let active = active_development_session().ok().flatten();
    let mut records = list_profile_traces()?;
    if let Some(existing) = records.iter().find(|record| record.path == source) {
        return Ok(existing.clone());
    }

    let added_unix_ms = vapor_telemetry::unix_time_ms();
    let stem = source
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("trace")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();

    let record = ProfileTraceRecord {
        id: format!("{added_unix_ms}-{stem}"),
        path: source,
        added_unix_ms,
        workspace: active.as_ref().map(|session| session.workspace.clone()),
        session_id: active.as_ref().map(|session| session.id.clone()),
        configuration: active.as_ref().map(|session| session.configuration.clone()),
    };

    records.push(record.clone());
    write_library(&records)?;
    Ok(record)
}

pub fn resolve_profile_trace(selector: Option<&str>) -> Result<ProfileTraceRecord, ProfileError> {
    if let Some(selector) = selector {
        let path = PathBuf::from(selector);
        if path.is_file() {
            return Ok(ProfileTraceRecord {
                id: path.file_stem().and_then(|name| name.to_str()).unwrap_or("trace").to_owned(),
                path,
                added_unix_ms: 0,
                workspace: None,
                session_id: None,
                configuration: None,
            });
        }

        let records = list_profile_traces()?;
        if let Some(exact) = records.iter().find(|record| record.id == selector) {
            return Ok(exact.clone());
        }
        let matches = records
            .into_iter()
            .filter(|record| record.id.starts_with(selector))
            .collect::<Vec<_>>();
        return match matches.as_slice() {
            [record] => Ok(record.clone()),
            [] => Err(ProfileError::message(format!("no saved trace matches `{selector}`"))),
            _ => Err(ProfileError::message(format!("saved trace selector `{selector}` is ambiguous"))),
        };
    }

    list_profile_traces()?
        .into_iter()
        .find(ProfileTraceRecord::exists)
        .ok_or_else(|| {
            ProfileError::message(
                "trace library is empty; drag a .tracy file into `vapor profile` or run `vapor profile import PATH`".to_owned(),
            )
        })
}

pub fn launch_tracy(trace: Option<&Path>) -> Result<(), ProfileError> {
    let installation = VaporInstallation::discover()?;
    let profiler = resolve_tracy_profiler(None, &installation.user_data_root())?;
    let mut command = Command::new(&profiler.executable);
    if let Some(trace) = trace {
        command.arg(trace);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| ProfileError::Process {
            operation: "launch Tracy",
            source,
        })?;
    Ok(())
}

pub fn analyze_profile_trace(
    record: &ProfileTraceRecord,
    options: &ProfileReportOptions,
) -> Result<ProfileReport, ProfileError> {
    if !record.path.is_file() {
        return Err(ProfileError::message(format!(
            "saved trace `{}` no longer exists",
            record.path.display()
        )));
    }

    let installation = VaporInstallation::discover()?;
    let csvexport = resolve_tracy_csvexport(&installation.user_data_root())?;
    let mut command = Command::new(csvexport);
    command.arg("-u").arg("-s").arg("\t");
    if options.self_time {
        command.arg("-e");
    }
    command.arg(&record.path);

    let output = command.output().map_err(|source| ProfileError::Process {
        operation: "run tracy-csvexport",
        source,
    })?;
    if !output.status.success() {
        return Err(ProfileError::message(format!(
            "tracy-csvexport failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let after_ns = options.after_ms.map(ms_to_ns);
    let before_ns = options.before_ms.map(ms_to_ns);
    let threads = options.thread_ids.iter().copied().collect::<BTreeSet<_>>();
    let filter = options.name_filter.as_deref().map(str::to_lowercase);

    let mut reader = ReaderBuilder::new()
        .delimiter(b'\t')
        .has_headers(true)
        .flexible(true)
        .from_reader(output.stdout.as_slice());

    let mut groups = BTreeMap::<(String, String, u32), Aggregate>::new();
    let mut occurrence_count = 0usize;

    for row in reader.deserialize::<TracyZoneOccurrence>() {
        let row = row.map_err(ProfileError::Csv)?;
        let _ = &row.value;
        if row.exec_time_ns <= 0 {
            continue;
        }
        if after_ns.is_some_and(|value| row.ns_since_start < value) {
            continue;
        }
        if before_ns.is_some_and(|value| row.ns_since_start > value) {
            continue;
        }
        if !threads.is_empty() && !threads.contains(&row.thread) {
            continue;
        }
        if filter.as_ref().is_some_and(|value| !row.name.to_lowercase().contains(value)) {
            continue;
        }

        occurrence_count += 1;
        let source = normalize_source(&row.src_file, record.workspace.as_deref());
        let aggregate = groups.entry((row.name, source, row.src_line)).or_default();
        aggregate.durations.push(row.exec_time_ns as u64);
        aggregate.threads.insert(row.thread);
    }

    let mut zones = groups
        .into_iter()
        .map(|((name, source, line), mut aggregate)| {
            aggregate.durations.sort_unstable();
            let count = aggregate.durations.len();
            let total = aggregate.durations.iter().copied().map(u128::from).sum::<u128>();
            ProfileZoneSummary {
                name,
                source,
                line,
                threads: aggregate.threads.into_iter().collect(),
                count,
                total_ns: total.min(u128::from(u64::MAX)) as u64,
                mean_ns: if count == 0 { 0 } else { (total / count as u128).min(u128::from(u64::MAX)) as u64 },
                median_ns: percentile(&aggregate.durations, 0.50),
                p90_ns: percentile(&aggregate.durations, 0.90),
                p95_ns: percentile(&aggregate.durations, 0.95),
                p99_ns: percentile(&aggregate.durations, 0.99),
                max_ns: aggregate.durations.last().copied().unwrap_or(0),
            }
        })
        .collect::<Vec<_>>();

    zones.sort_by(|left, right| {
        sort_value(right, options.sort)
            .cmp(&sort_value(left, options.sort))
            .then_with(|| right.total_ns.cmp(&left.total_ns))
            .then_with(|| left.name.cmp(&right.name))
    });

    let zone_count = zones.len();
    if options.top > 0 && zones.len() > options.top {
        zones.truncate(options.top);
    }

    Ok(ProfileReport {
        trace: record.path.clone(),
        self_time: options.self_time,
        occurrence_count,
        zone_count,
        zones,
    })
}

fn write_library(records: &[ProfileTraceRecord]) -> Result<(), ProfileError> {
    let path = profile_library_path()?;
    let parent = path.parent().expect("profile library has parent");
    fs::create_dir_all(parent).map_err(|source| ProfileError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let source = serde_json::to_vec_pretty(records).map_err(ProfileError::Serialize)?;
    fs::write(&path, source).map_err(|source| ProfileError::Io { path, source })
}

fn sort_value(zone: &ProfileZoneSummary, sort: ProfileSort) -> u64 {
    match sort {
        ProfileSort::Total => zone.total_ns,
        ProfileSort::Count => zone.count.min(u64::MAX as usize) as u64,
        ProfileSort::Mean => zone.mean_ns,
        ProfileSort::Median => zone.median_ns,
        ProfileSort::P90 => zone.p90_ns,
        ProfileSort::P95 => zone.p95_ns,
        ProfileSort::P99 => zone.p99_ns,
        ProfileSort::Max => zone.max_ns,
    }
}

fn percentile(sorted: &[u64], fraction: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let index = fraction.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let low = index.floor() as usize;
    let high = index.ceil() as usize;
    if low == high {
        return sorted[low];
    }
    let blend = index - low as f64;
    (sorted[low] as f64 + (sorted[high] as f64 - sorted[low] as f64) * blend)
        .round()
        .clamp(0.0, u64::MAX as f64) as u64
}

fn ms_to_ns(value: f64) -> i64 {
    (value.max(0.0) * 1_000_000.0).round().clamp(0.0, i64::MAX as f64) as i64
}

fn normalize_source(source: &str, workspace: Option<&Path>) -> String {
    let path = Path::new(source);
    if let Some(workspace) = workspace
        && let Ok(relative) = path.strip_prefix(workspace)
    {
        return relative.to_string_lossy().replace('\\', "/");
    }
    source.replace('\\', "/")
}

#[derive(Debug)]
pub enum ProfileError {
    Message(String),
    Installation(crate::InstallationError),
    Development(crate::DevelopmentRunError),
    Tracy(TracyError),
    Csv(csv::Error),
    Serialize(serde_json::Error),
    LibraryMetadata { path: PathBuf, source: serde_json::Error },
    Io { path: PathBuf, source: io::Error },
    Process { operation: &'static str, source: io::Error },
}

impl ProfileError {
    fn message(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<crate::InstallationError> for ProfileError {
    fn from(value: crate::InstallationError) -> Self { Self::Installation(value) }
}
impl From<crate::DevelopmentRunError> for ProfileError {
    fn from(value: crate::DevelopmentRunError) -> Self { Self::Development(value) }
}
impl From<TracyError> for ProfileError {
    fn from(value: TracyError) -> Self { Self::Tracy(value) }
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => formatter.write_str(message),
            Self::Installation(error) => error.fmt(formatter),
            Self::Development(error) => error.fmt(formatter),
            Self::Tracy(error) => error.fmt(formatter),
            Self::Csv(error) => write!(formatter, "failed to parse Tracy export: {error}"),
            Self::Serialize(error) => write!(formatter, "failed to serialize profile library: {error}"),
            Self::LibraryMetadata { path, source } => write!(formatter, "invalid profile library `{}`: {source}", path.display()),
            Self::Io { path, source } => write!(formatter, "failed to access `{}`: {source}", path.display()),
            Self::Process { operation, source } => write!(formatter, "failed to {operation}: {source}"),
        }
    }
}

impl std::error::Error for ProfileError {}
