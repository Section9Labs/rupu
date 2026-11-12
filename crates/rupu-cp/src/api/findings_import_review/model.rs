use super::discovery::FileIdentity;
use rupu_coverage::tools::PreparedImportReport;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const VERSION: u32 = 1;
pub const REVIEW_SECONDS: i64 = 15 * 60;
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_DEPENDENCY_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_DEPENDENCY_FILES: usize = 1000;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewRequest {
    #[serde(default)]
    pub files: Vec<PathBuf>,
    pub directory: Option<PathBuf>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitRequest {
    pub review_id: String,
    pub review_digest: String,
    pub operation_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Target {
    pub ws_id: String,
    pub target_id: String,
    pub finding_id: String,
    pub project: String,
    pub workspace_path: PathBuf,
    pub workspace_identity: FileIdentity,
    pub registry_path: PathBuf,
    pub registry_sha256: String,
    pub registry_identity: FileIdentity,
    pub ledger_path: PathBuf,
    pub ledger_identity: FileIdentity,
    pub ledger_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlannedFinding {
    pub summary: String,
    pub severity: rupu_coverage::Severity,
    pub evidence: rupu_coverage::FindingEvidence,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Row {
    pub ordinal: usize,
    pub source_path: PathBuf,
    pub canonical_source_path: PathBuf,
    pub source_sha256: String,
    pub source_bytes: u64,
    pub target: Option<Target>,
    pub status: String,
    pub reason: Option<String>,
    pub title: Option<String>,
    pub normalized_report_sha256: Option<String>,
    pub planned_finding: Option<PlannedFinding>,
    pub dependencies: Vec<rupu_coverage::report::ArtifactRef>,
    pub dependency_count: usize,
    pub dependency_bytes: u64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredRow {
    pub row: Row,
    pub prepared: Option<PreparedImportReport>,
    pub prepared_ledger: Option<rupu_coverage::tools::PreparedLedgerIdentity>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub review_id: String,
    pub owner: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub policy_digest: String,
    pub rows: Vec<StoredRow>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preview {
    pub version: u32,
    pub review_id: String,
    pub review_digest: String,
    pub host: String,
    pub scope: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub discovery_complete: bool,
    pub rows: Vec<Row>,
    pub ready: usize,
    pub skipped: usize,
    pub failed: usize,
    pub project_count: usize,
    pub ledger_count: usize,
    pub warnings: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResultRow {
    pub ordinal: usize,
    pub source_path: PathBuf,
    pub target: Option<Target>,
    pub status: String,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LedgerResult {
    pub ledger_path: PathBuf,
    pub ws_id: String,
    pub target_id: String,
    pub status: String,
    pub backup: Option<PathBuf>,
    /// Installed content-addressed artifacts may outlive a failed ledger write.
    pub artifact_side_effects_possible: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Operation {
    pub version: u32,
    pub reviewed: Preview,
    pub operation_id: String,
    pub review_id: String,
    pub review_digest: String,
    pub owner: String,
    pub status: String,
    pub receipt_durable: bool,
    pub rows: Vec<ResultRow>,
    pub ledgers: Vec<LedgerResult>,
    pub warnings: Vec<String>,
}
