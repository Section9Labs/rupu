//! Per-run settings for recording findings, carried to the write path on
//! `ToolContext.findings` (agent builtin) or `FindingsContext` (MCP tool).

use crate::report::FindingProfile;
use std::path::PathBuf;

pub const DEFAULT_ARTIFACT_MAX_BYTES: u64 = 500 * 1024 * 1024;
pub const DEFAULT_REPORT_MAX_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingWriteOptions {
    pub profile: FindingProfile,
    /// Root of the artifact store (`<RUPU_HOME>/findings/artifacts`). `None`
    /// makes a report that lists artifacts fail loudly rather than drop them.
    pub artifact_root: Option<PathBuf>,
    pub artifact_max_bytes: u64,
    pub report_max_bytes: usize,
    /// Organisation-specific ticket patterns (from `[findings].ticket_patterns`),
    /// appended to the prompt guidance. Nothing org-specific ships in rupu.
    pub ticket_patterns: Vec<String>,
}

impl Default for FindingWriteOptions {
    fn default() -> Self {
        Self {
            profile: FindingProfile::default(),
            artifact_root: None,
            artifact_max_bytes: DEFAULT_ARTIFACT_MAX_BYTES,
            report_max_bytes: DEFAULT_REPORT_MAX_BYTES,
            ticket_patterns: Vec::new(),
        }
    }
}

impl FindingWriteOptions {
    pub fn with_profile(mut self, profile: FindingProfile) -> Self {
        self.profile = profile;
        self
    }
}
