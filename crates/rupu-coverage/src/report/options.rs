//! Per-run settings for recording findings, carried to the write path on
//! `ToolContext.findings` (agent builtin) or `FindingsContext` (MCP tool).

use crate::profile::ActiveSet;
use crate::report::FindingProfile;
use std::path::PathBuf;
use std::sync::Arc;

pub const DEFAULT_ARTIFACT_MAX_BYTES: u64 = 500 * 1024 * 1024;
pub const DEFAULT_REPORT_MAX_BYTES: usize = 256 * 1024;
/// Most files one report's artifacts may expand to.
pub const DEFAULT_ARTIFACT_MAX_FILES: usize = 500;
/// Most bytes one report's artifacts may add up to (2 GiB).
pub const DEFAULT_ARTIFACT_TOTAL_MAX_BYTES: u64 = 2_147_483_648;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingWriteOptions {
    pub profile: FindingProfile,
    /// Root of the artifact store (`<RUPU_HOME>/findings/artifacts`). `None`
    /// makes a report that lists artifacts fail loudly rather than drop them.
    pub artifact_root: Option<PathBuf>,
    /// Per-file copy cap: a larger artifact is hashed and recorded
    /// `external` instead of copied.
    pub artifact_max_bytes: u64,
    /// Most files one report's artifacts may expand to (directories count
    /// each file inside them). Checked before anything is copied.
    pub artifact_max_files: usize,
    /// Most bytes one report's artifacts may add up to in the store: only
    /// files at or under `artifact_max_bytes` (the ones that are copied)
    /// count; a larger file is recorded by reference and never counts.
    /// Checked before anything is copied.
    pub artifact_total_max_bytes: u64,
    pub report_max_bytes: usize,
    /// Organisation-specific ticket patterns (from `[findings].ticket_patterns`),
    /// appended to the prompt guidance. Nothing org-specific ships in rupu.
    pub ticket_patterns: Vec<String>,
    /// The run's active engagement profiles. `None` is the native code path:
    /// findings record exactly as before, with no profile routing, no
    /// completeness gate, and no asset stamping — byte-identical.
    pub engagement: Option<Arc<ActiveSet>>,
}

impl Default for FindingWriteOptions {
    fn default() -> Self {
        Self {
            profile: FindingProfile::default(),
            artifact_root: None,
            artifact_max_bytes: DEFAULT_ARTIFACT_MAX_BYTES,
            artifact_max_files: DEFAULT_ARTIFACT_MAX_FILES,
            artifact_total_max_bytes: DEFAULT_ARTIFACT_TOTAL_MAX_BYTES,
            report_max_bytes: DEFAULT_REPORT_MAX_BYTES,
            ticket_patterns: Vec::new(),
            engagement: None,
        }
    }
}

impl FindingWriteOptions {
    pub fn with_profile(mut self, profile: FindingProfile) -> Self {
        self.profile = profile;
        self
    }

    /// The artifact-ingest bounds these options carry.
    pub fn ingest_limits(&self) -> crate::report::IngestLimits {
        crate::report::IngestLimits {
            max_file_bytes: self.artifact_max_bytes,
            max_files: self.artifact_max_files,
            max_total_bytes: self.artifact_total_max_bytes,
        }
    }
}
