use crate::ledger::events::AssertionStatus;
use serde::{Deserialize, Serialize};

/// A `(concern_id, file_path)` cell with the status a run gave it. Used
/// for the cell-coverage delta (newly / no-longer asserted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellRef {
    pub concern_id: String,
    pub file_path: String,
    pub status: AssertionStatus,
}

/// A `(concern_id, file_path)` cell whose verdict differs between the two
/// runs. `high_signal` is set for the `clean -> finding` transition (a
/// later run found something an earlier run called clean).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictFlip {
    pub concern_id: String,
    pub file_path: String,
    pub base_status: AssertionStatus,
    pub compare_status: AssertionStatus,
    pub high_signal: bool,
}

/// A finding matched across runs by `(concern_id, theme)` — the same
/// best-effort theme primitive the audit's serendipitous clustering uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingThemeRef {
    pub concern_id: Option<String>,
    pub theme: String,
}

/// The result of `run_diff(base, compare)`. All vectors are sorted
/// deterministically so output is stable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunDiff {
    pub base_runs: Vec<String>,
    pub compare_runs: Vec<String>,
    pub newly_asserted: Vec<CellRef>,
    pub no_longer_asserted: Vec<CellRef>,
    pub verdict_flips: Vec<VerdictFlip>,
    pub findings_appeared: Vec<FindingThemeRef>,
    pub findings_disappeared: Vec<FindingThemeRef>,
    pub newly_touched: Vec<String>,
    pub no_longer_touched: Vec<String>,
}

impl RunDiff {
    /// True when the two contributions are identical across all
    /// dimensions (no changes to report).
    pub fn is_empty(&self) -> bool {
        self.newly_asserted.is_empty()
            && self.no_longer_asserted.is_empty()
            && self.verdict_flips.is_empty()
            && self.findings_appeared.is_empty()
            && self.findings_disappeared.is_empty()
            && self.newly_touched.is_empty()
            && self.no_longer_touched.is_empty()
    }
}

/// One row of `rupu coverage runs` — a run with its identity and
/// contribution counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunListEntry {
    pub run_id: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub model: String,
    pub surface: crate::ledger::events::Surface,
    pub cells_asserted: usize,
    pub findings: usize,
    pub files_touched: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_diff_is_empty_when_all_dimensions_empty() {
        let diff = RunDiff {
            base_runs: vec!["a".into()],
            compare_runs: vec!["b".into()],
            newly_asserted: vec![],
            no_longer_asserted: vec![],
            verdict_flips: vec![],
            findings_appeared: vec![],
            findings_disappeared: vec![],
            newly_touched: vec![],
            no_longer_touched: vec![],
        };
        assert!(diff.is_empty());
    }

    #[test]
    fn run_diff_round_trips_json() {
        let diff = RunDiff {
            base_runs: vec!["run_a".into()],
            compare_runs: vec!["run_b".into()],
            newly_asserted: vec![CellRef {
                concern_id: "stride:spoofing".into(),
                file_path: "src/a.rs".into(),
                status: AssertionStatus::Clean,
            }],
            no_longer_asserted: vec![],
            verdict_flips: vec![VerdictFlip {
                concern_id: "stride:tampering".into(),
                file_path: "src/b.rs".into(),
                base_status: AssertionStatus::Clean,
                compare_status: AssertionStatus::Finding,
                high_signal: true,
            }],
            findings_appeared: vec![FindingThemeRef {
                concern_id: None,
                theme: "missing csrf token on".into(),
            }],
            findings_disappeared: vec![],
            newly_touched: vec!["src/c.rs".into()],
            no_longer_touched: vec![],
        };
        let json = serde_json::to_string(&diff).unwrap();
        let back: RunDiff = serde_json::from_str(&json).unwrap();
        assert_eq!(diff, back);
        assert!(!diff.is_empty());
    }
}
