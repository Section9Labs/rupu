use chrono::{DateTime, Utc};
use rupu_coverage::FindingRecord;

/// One finding as the caller collected it.
#[derive(Debug, Clone)]
pub struct ExportInput {
    pub ws_id: String,
    pub project: String,
    pub workflow_name: Option<String>,
    pub record: FindingRecord,
}

/// A finding with its display number (e.g. `SEC-004`).
#[derive(Debug, Clone)]
pub struct ExportFinding {
    pub number: String,
    pub input: ExportInput,
}

#[derive(Debug, Clone)]
pub struct ReportMeta {
    pub title: String,
    pub generated_at: DateTime<Utc>,
    /// Human description of what was selected, e.g. "Project notebin · severity ≥ high".
    pub scope: String,
}
