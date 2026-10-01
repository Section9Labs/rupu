//! Per-run settings for recording findings, carried to the write path on
//! `ToolContext.findings` (agent builtin) or `FindingsContext` (MCP tool).

use crate::report::FindingProfile;
use std::path::PathBuf;

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
    /// The engagement profiles active for this run (`None` = the native
    /// `code` path, unchanged). Runtime-only: this struct is never
    /// serialized, so the set is resolved per run, not persisted with it.
    pub engagement: Option<std::sync::Arc<crate::profile::ActiveSet>>,
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

    /// Attach the active engagement set (`None` keeps the native `code` path).
    pub fn with_engagement(mut self, e: Option<std::sync::Arc<crate::profile::ActiveSet>>) -> Self {
        self.engagement = e;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::builtin_registry;
    use std::sync::Arc;

    fn set() -> crate::profile::ActiveSet {
        builtin_registry()
            .unwrap()
            .active_set(&["code".into(), "binary".into()])
            .unwrap()
    }

    #[test]
    fn engagement_defaults_to_none() {
        assert!(FindingWriteOptions::default().engagement.is_none());
    }

    #[test]
    fn with_engagement_carries_the_active_set() {
        let opts = FindingWriteOptions::default().with_engagement(Some(Arc::new(set())));
        assert!(opts.engagement.is_some());
        let cleared = opts.with_engagement(None);
        assert!(cleared.engagement.is_none());
    }

    #[test]
    fn options_with_equal_engagement_compare_equal() {
        let a = FindingWriteOptions::default().with_engagement(Some(Arc::new(set())));
        let b = a.clone();
        assert_eq!(a, b);
        // Equality is by value, not Arc pointer identity.
        let c = FindingWriteOptions::default().with_engagement(Some(Arc::new(set())));
        assert_eq!(a, c);
        assert_ne!(a, FindingWriteOptions::default());
    }
}
