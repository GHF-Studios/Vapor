use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileTraceSort {
    Total,
    Count,
    Mean,
    Median,
    P90,
    #[default]
    P95,
    P99,
    Max,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileAnalysisOptions {
    pub top: usize,
    pub sort: ProfileTraceSort,
    pub name_filter: Option<String>,
    pub self_time: bool,
}
