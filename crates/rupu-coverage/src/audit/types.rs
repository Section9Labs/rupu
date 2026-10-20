use crate::catalog::types::Severity;
use crate::ledger::events::AssertionStatus;
use serde::{Deserialize, Serialize};

/// Coverage outcome for a single concern across the target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConcernCoverage {
    pub concern_id: String,
    pub name: String,
    pub severity: Severity,
    /// Files in scope = touched files whose path matches the concern's applicable_globs.
    pub in_scope_files: Vec<String>,
    /// Files with a non-NotApplicable assertion for this concern.
    pub asserted_files: Vec<String>,
    /// in_scope − asserted: files that should have been assessed but weren't.
    pub gap_files: Vec<String>,
    pub clean: u32,
    pub findings: u32,
    pub examined: u32,
    pub not_applicable: u32,
}

impl ConcernCoverage {
    /// True when every in-scope file has been assessed (no gaps).
    pub fn is_complete(&self) -> bool {
        self.gap_files.is_empty()
    }
}

/// Per-file coverage summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileCoverage {
    pub path: String,
    pub strongest_touch: String,
    pub asserted_concerns: Vec<String>,
    /// Catalog concern ids whose applicable_globs match this file but have no assertion.
    pub missing_concerns: Vec<String>,
}

/// A (concern, file) pair assessed by more than one model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossModelEntry {
    pub concern_id: String,
    pub file_path: String,
    pub model_statuses: Vec<(String, AssertionStatus)>,
    pub disagreement: bool,
}

/// Serendipitous findings (concern_id = None) grouped by a coarse theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerendipitousCluster {
    pub theme: String,
    pub finding_ids: Vec<String>,
    pub count: u32,
}

/// Full audit report for a target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditReport {
    pub target_id: String,
    pub concerns: Vec<ConcernCoverage>,
    pub files: Vec<FileCoverage>,
    pub cross_model: Vec<CrossModelEntry>,
    pub serendipitous: Vec<SerendipitousCluster>,
    pub total_concerns: usize,
    pub complete_concerns: usize,
    pub total_gap_files: usize,
}

impl AuditReport {
    /// The coverage gaps alone: one row per (concern, in-scope file lacking
    /// an assertion), in concern order — what `rupu coverage gap` prints and
    /// `GET /api/coverage/:target/gap` serves.
    pub fn gaps(&self) -> CoverageGaps {
        let rows = self
            .concerns
            .iter()
            .flat_map(|c| {
                c.gap_files.iter().map(|f| GapRow {
                    concern_id: c.concern_id.clone(),
                    file: f.clone(),
                })
            })
            .collect();
        CoverageGaps {
            kind: "coverage_gaps",
            version: 1,
            rows,
        }
    }
}

/// One coverage gap: an in-scope file the concern has no assertion for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GapRow {
    pub concern_id: String,
    pub file: String,
}

/// A target's coverage gaps ([`AuditReport::gaps`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoverageGaps {
    pub kind: &'static str,
    pub version: u8,
    pub rows: Vec<GapRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concern_coverage_is_complete_when_no_gaps() {
        let cc = ConcernCoverage {
            concern_id: "ssrf".to_string(),
            name: "SSRF".to_string(),
            severity: Severity::High,
            in_scope_files: vec!["a.rs".to_string()],
            asserted_files: vec!["a.rs".to_string()],
            gap_files: vec![],
            clean: 1,
            findings: 0,
            examined: 0,
            not_applicable: 0,
        };
        assert!(cc.is_complete());
    }

    #[test]
    fn gaps_are_one_row_per_concern_and_gap_file_in_order() {
        let concern = |id: &str, gaps: &[&str]| ConcernCoverage {
            concern_id: id.to_string(),
            name: id.to_uppercase(),
            severity: Severity::High,
            in_scope_files: vec![],
            asserted_files: vec![],
            gap_files: gaps.iter().map(|g| g.to_string()).collect(),
            clean: 0,
            findings: 0,
            examined: 0,
            not_applicable: 0,
        };
        let report = AuditReport {
            target_id: "t".to_string(),
            concerns: vec![
                concern("ssrf", &["b.rs", "a.rs"]),
                concern("xss", &[]),
                concern("sqli", &["c.rs"]),
            ],
            files: vec![],
            cross_model: vec![],
            serendipitous: vec![],
            total_concerns: 3,
            complete_concerns: 1,
            total_gap_files: 3,
        };
        assert_eq!(
            serde_json::to_value(report.gaps()).unwrap(),
            serde_json::json!({
                "kind": "coverage_gaps",
                "version": 1,
                "rows": [
                    {"concern_id": "ssrf", "file": "b.rs"},
                    {"concern_id": "ssrf", "file": "a.rs"},
                    {"concern_id": "sqli", "file": "c.rs"},
                ],
            })
        );
    }

    #[test]
    fn concern_coverage_incomplete_with_gaps() {
        let cc = ConcernCoverage {
            concern_id: "ssrf".to_string(),
            name: "SSRF".to_string(),
            severity: Severity::High,
            in_scope_files: vec!["a.rs".to_string(), "b.rs".to_string()],
            asserted_files: vec!["a.rs".to_string()],
            gap_files: vec!["b.rs".to_string()],
            clean: 1,
            findings: 0,
            examined: 0,
            not_applicable: 0,
        };
        assert!(!cc.is_complete());
    }
}
