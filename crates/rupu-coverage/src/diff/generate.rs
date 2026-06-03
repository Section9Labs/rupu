//! Run ordering, selector resolution, contribution building, and the
//! `run_diff` / `list_runs` entry points.

use crate::audit::generate::theme_key;
use crate::ledger::events::{AssertionStatus, ConcernAssertion, FileTouchEvent, FindingRecord};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

/// Selects which run(s) a diff side refers to. v1 selectors each resolve
/// to exactly one run; the return type is a `Vec` so future `model:` /
/// `through:` selectors (sets of runs) feed the same engine unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunSelector {
    RunId(String),
    Latest,
    Previous,
}

impl FromStr for RunSelector {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "latest" => RunSelector::Latest,
            "previous" => RunSelector::Previous,
            other => RunSelector::RunId(other.to_string()),
        })
    }
}

/// Errors from the diff / runs surface.
#[derive(Debug, thiserror::Error)]
pub enum DiffError {
    #[error("io error reading ledgers: {0}")]
    Io(#[from] std::io::Error),
    #[error("no run with id '{0}' on this target")]
    UnknownRun(String),
    #[error("no run matches '{0}'")]
    NoRunMatches(String),
}

/// Run ids ordered most-recent-first. "Recency" is the maximum timestamp
/// observed for a run across all three ledgers; ties break by run id
/// ascending for stability.
#[allow(dead_code)]
pub(crate) fn ordered_runs(
    files: &[FileTouchEvent],
    assertions: &[ConcernAssertion],
    findings: &[FindingRecord],
) -> Vec<String> {
    let mut max_at: BTreeMap<String, DateTime<Utc>> = BTreeMap::new();
    let mut bump = |run_id: &str, at: DateTime<Utc>| {
        max_at
            .entry(run_id.to_string())
            .and_modify(|cur| {
                if at > *cur {
                    *cur = at;
                }
            })
            .or_insert(at);
    };
    for f in files {
        bump(&f.attribution().run_id, f.at());
    }
    for a in assertions {
        bump(&a.declared_by.run_id, a.declared_at);
    }
    for f in findings {
        bump(&f.declared_by.run_id, f.declared_at);
    }
    let mut runs: Vec<(String, DateTime<Utc>)> = max_at.into_iter().collect();
    // Most-recent-first; ties broken by run id ascending.
    runs.sort_by(|(a_id, a_at), (b_id, b_at)| b_at.cmp(a_at).then(a_id.cmp(b_id)));
    runs.into_iter().map(|(id, _)| id).collect()
}

/// Resolve a selector against the recency-ordered run list. v1 returns a
/// single-element Vec.
#[allow(dead_code)]
pub(crate) fn resolve_selector(
    selector: &RunSelector,
    ordered: &[String],
) -> Result<Vec<String>, DiffError> {
    match selector {
        RunSelector::RunId(id) => {
            if ordered.iter().any(|r| r == id) {
                Ok(vec![id.clone()])
            } else {
                Err(DiffError::UnknownRun(id.clone()))
            }
        }
        RunSelector::Latest => ordered
            .first()
            .map(|r| vec![r.clone()])
            .ok_or_else(|| DiffError::NoRunMatches("latest".to_string())),
        RunSelector::Previous => ordered
            .get(1)
            .map(|r| vec![r.clone()])
            .ok_or_else(|| DiffError::NoRunMatches("previous".to_string())),
    }
}

/// One run set's contribution to a target, reduced for diffing.
#[allow(dead_code)]
pub(crate) struct Contribution {
    /// `(concern_id, file_path) -> last status` for assertions by these runs.
    pub cells: BTreeMap<(String, String), AssertionStatus>,
    /// File paths touched by these runs.
    pub touched: BTreeSet<String>,
    /// `(concern_id, theme_key(summary))` for findings by these runs.
    pub finding_themes: BTreeSet<(Option<String>, String)>,
}

/// Build a contribution from the ledgers, restricted to `runs`. Cell
/// supersession matches the audit: assertions are applied in timestamp
/// order so the last write within the run set wins.
#[allow(dead_code)]
pub(crate) fn contribution(
    runs: &BTreeSet<String>,
    files: &[FileTouchEvent],
    assertions: &[ConcernAssertion],
    findings: &[FindingRecord],
) -> Contribution {
    let mut touched: BTreeSet<String> = BTreeSet::new();
    for f in files.iter().filter(|f| runs.contains(&f.attribution().run_id)) {
        if let Some(path) = f.path() {
            touched.insert(path.to_string());
        }
    }

    let mut sorted: Vec<&ConcernAssertion> = assertions
        .iter()
        .filter(|a| runs.contains(&a.declared_by.run_id))
        .collect();
    sorted.sort_by_key(|a| a.declared_at);
    let mut cells: BTreeMap<(String, String), AssertionStatus> = BTreeMap::new();
    for a in sorted {
        cells.insert((a.concern_id.clone(), a.file_path.clone()), a.status);
    }

    let mut finding_themes: BTreeSet<(Option<String>, String)> = BTreeSet::new();
    for f in findings.iter().filter(|f| runs.contains(&f.declared_by.run_id)) {
        finding_themes.insert((f.concern_id.clone(), theme_key(&f.summary)));
    }

    Contribution {
        cells,
        touched,
        finding_themes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::types::Severity;
    use crate::ledger::events::{Attribution, Evidence, FindingEvidence, FindingScope, Surface};

    fn attribution(run_id: &str) -> Attribution {
        Attribution {
            run_id: run_id.to_string(),
            model: "m1".to_string(),
            surface: Surface::Session,
        }
    }

    fn read_event(run_id: &str, secs: i64) -> FileTouchEvent {
        FileTouchEvent::Read {
            path: "src/a.rs".to_string(),
            line_range: [1, 10],
            tool: "read_file".to_string(),
            attribution: attribution(run_id),
            at: DateTime::<Utc>::from_timestamp(secs, 0).unwrap(),
        }
    }

    fn assertion(run: &str, concern: &str, file: &str, status: AssertionStatus, secs: i64) -> ConcernAssertion {
        ConcernAssertion {
            concern_id: concern.to_string(),
            file_path: file.to_string(),
            status,
            evidence: Evidence {
                summary: "s".to_string(),
                line_ranges: vec![],
                finding_ids: vec![],
            },
            declared_by: attribution(run),
            declared_at: DateTime::<Utc>::from_timestamp(secs, 0).unwrap(),
        }
    }

    fn finding(run: &str, concern: Option<&str>, summary: &str) -> FindingRecord {
        FindingRecord {
            id: format!("find_{summary}"),
            file_path: None,
            line_range: None,
            scope: FindingScope::File,
            summary: summary.to_string(),
            severity: Severity::Medium,
            concern_id: concern.map(|c| c.to_string()),
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".to_string(),
                references: vec![],
            },
            declared_by: attribution(run),
            declared_at: Utc::now(),
        }
    }

    #[test]
    fn ordered_runs_is_most_recent_first() {
        let files = vec![read_event("run_old", 100), read_event("run_new", 200)];
        let ordered = ordered_runs(&files, &[], &[]);
        assert_eq!(ordered, vec!["run_new", "run_old"]);
    }

    #[test]
    fn ordered_runs_breaks_ties_by_run_id() {
        let files = vec![read_event("run_b", 100), read_event("run_a", 100)];
        let ordered = ordered_runs(&files, &[], &[]);
        assert_eq!(ordered, vec!["run_a", "run_b"]);
    }

    #[test]
    fn resolve_latest_and_previous() {
        let ordered = vec!["run_new".to_string(), "run_old".to_string()];
        assert_eq!(
            resolve_selector(&RunSelector::Latest, &ordered).unwrap(),
            vec!["run_new"]
        );
        assert_eq!(
            resolve_selector(&RunSelector::Previous, &ordered).unwrap(),
            vec!["run_old"]
        );
    }

    #[test]
    fn resolve_explicit_run_id() {
        let ordered = vec!["run_new".to_string(), "run_old".to_string()];
        assert_eq!(
            resolve_selector(&RunSelector::RunId("run_old".into()), &ordered).unwrap(),
            vec!["run_old"]
        );
    }

    #[test]
    fn resolve_unknown_run_id_errors() {
        let ordered = vec!["run_new".to_string()];
        let err = resolve_selector(&RunSelector::RunId("nope".into()), &ordered).unwrap_err();
        assert!(matches!(err, DiffError::UnknownRun(id) if id == "nope"));
    }

    #[test]
    fn resolve_previous_with_single_run_errors() {
        let ordered = vec!["only".to_string()];
        let err = resolve_selector(&RunSelector::Previous, &ordered).unwrap_err();
        assert!(matches!(err, DiffError::NoRunMatches(s) if s == "previous"));
    }

    #[test]
    fn resolve_latest_with_no_runs_errors() {
        let err = resolve_selector(&RunSelector::Latest, &[]).unwrap_err();
        assert!(matches!(err, DiffError::NoRunMatches(s) if s == "latest"));
    }

    #[test]
    fn contribution_collects_cells_touched_and_themes_for_run_set() {
        let runs: BTreeSet<String> = ["run_a".to_string()].into_iter().collect();
        let mut files = vec![read_event("run_a", 100), read_event("run_b", 100)];
        // run_b's file event must NOT appear in run_a's contribution.
        files.push(FileTouchEvent::Read {
            path: "src/only_a.rs".to_string(),
            line_range: [1, 5],
            tool: "read_file".to_string(),
            attribution: attribution("run_a"),
            at: DateTime::<Utc>::from_timestamp(101, 0).unwrap(),
        });
        let assertions = vec![
            assertion("run_a", "c1", "src/a.rs", AssertionStatus::Clean, 100),
            // later assertion in the same run supersedes the earlier one
            assertion("run_a", "c1", "src/a.rs", AssertionStatus::Finding, 200),
            assertion("run_b", "c2", "src/b.rs", AssertionStatus::Clean, 100),
        ];
        let findings = vec![
            finding("run_a", Some("c1"), "sql injection in login handler path"),
            finding("run_b", None, "unrelated finding from other run here"),
        ];
        let c = contribution(&runs, &files, &assertions, &findings);

        // Cell supersession: last status within the run wins.
        assert_eq!(
            c.cells.get(&("c1".to_string(), "src/a.rs".to_string())),
            Some(&AssertionStatus::Finding)
        );
        // run_b's cell is excluded.
        assert!(!c.cells.contains_key(&("c2".to_string(), "src/b.rs".to_string())));
        // Touched paths come only from run_a.
        assert!(c.touched.contains("src/a.rs"));
        assert!(c.touched.contains("src/only_a.rs"));
        // Finding themes are (concern_id, theme_key); run_b's is excluded.
        assert!(c
            .finding_themes
            .contains(&(Some("c1".to_string()), theme_key("sql injection in login handler path"))));
        assert_eq!(c.finding_themes.len(), 1);
    }
}
