//! Run ordering, selector resolution, contribution building, and the
//! `run_diff` / `list_runs` entry points.

use crate::audit::generate::theme_key;
use crate::diff::types::{CellRef, FindingThemeRef, RunDiff, RunListEntry, VerdictFlip};
use crate::ledger::events::{
    AssertionStatus, ConcernAssertion, FileTouchEvent, FindingRecord, Surface,
};
use crate::ledger::paths::CoveragePaths;
use crate::ledger::views::{read_concern_assertions, read_file_events, read_findings};
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
pub(crate) fn contribution(
    runs: &BTreeSet<String>,
    files: &[FileTouchEvent],
    assertions: &[ConcernAssertion],
    findings: &[FindingRecord],
) -> Contribution {
    let mut touched: BTreeSet<String> = BTreeSet::new();
    for f in files
        .iter()
        .filter(|f| runs.contains(&f.attribution().run_id))
    {
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
    for f in findings
        .iter()
        .filter(|f| runs.contains(&f.declared_by.run_id))
    {
        finding_themes.insert((f.concern_id.clone(), theme_key(&f.summary)));
    }

    Contribution {
        cells,
        touched,
        finding_themes,
    }
}

/// Diff two run selectors against a target's ledgers. `base` is the
/// earlier reference; `compare` is the run under inspection.
pub fn run_diff(
    paths: &CoveragePaths,
    base: &RunSelector,
    compare: &RunSelector,
) -> Result<RunDiff, DiffError> {
    let files = read_file_events(paths)?;
    let assertions = read_concern_assertions(paths)?;
    let findings = read_findings(paths)?;

    let ordered = ordered_runs(&files, &assertions, &findings);
    let base_runs = resolve_selector(base, &ordered)?;
    let compare_runs = resolve_selector(compare, &ordered)?;

    let base_set: BTreeSet<String> = base_runs.iter().cloned().collect();
    let compare_set: BTreeSet<String> = compare_runs.iter().cloned().collect();
    let b = contribution(&base_set, &files, &assertions, &findings);
    let c = contribution(&compare_set, &files, &assertions, &findings);

    // Cell-coverage delta.
    let mut newly_asserted: Vec<CellRef> = c
        .cells
        .iter()
        .filter(|(k, _)| !b.cells.contains_key(*k))
        .map(|((concern_id, file_path), status)| CellRef {
            concern_id: concern_id.clone(),
            file_path: file_path.clone(),
            status: *status,
        })
        .collect();
    let mut no_longer_asserted: Vec<CellRef> = b
        .cells
        .iter()
        .filter(|(k, _)| !c.cells.contains_key(*k))
        .map(|((concern_id, file_path), status)| CellRef {
            concern_id: concern_id.clone(),
            file_path: file_path.clone(),
            status: *status,
        })
        .collect();

    // Verdict flips: cells in both with a changed status.
    let mut verdict_flips: Vec<VerdictFlip> = b
        .cells
        .iter()
        .filter_map(|(k, base_status)| {
            c.cells.get(k).and_then(|compare_status| {
                if base_status != compare_status {
                    Some(VerdictFlip {
                        concern_id: k.0.clone(),
                        file_path: k.1.clone(),
                        base_status: *base_status,
                        compare_status: *compare_status,
                        high_signal: *base_status == AssertionStatus::Clean
                            && *compare_status == AssertionStatus::Finding,
                    })
                } else {
                    None
                }
            })
        })
        .collect();

    // Finding themes appeared / disappeared.
    let mut findings_appeared: Vec<FindingThemeRef> = c
        .finding_themes
        .difference(&b.finding_themes)
        .map(|(concern_id, theme)| FindingThemeRef {
            concern_id: concern_id.clone(),
            theme: theme.clone(),
        })
        .collect();
    let mut findings_disappeared: Vec<FindingThemeRef> = b
        .finding_themes
        .difference(&c.finding_themes)
        .map(|(concern_id, theme)| FindingThemeRef {
            concern_id: concern_id.clone(),
            theme: theme.clone(),
        })
        .collect();

    // File-touch delta.
    let mut newly_touched: Vec<String> = c.touched.difference(&b.touched).cloned().collect();
    let mut no_longer_touched: Vec<String> = b.touched.difference(&c.touched).cloned().collect();

    // Deterministic ordering for stable output.
    let cell_key = |r: &CellRef| (r.concern_id.clone(), r.file_path.clone());
    newly_asserted.sort_by_key(cell_key);
    no_longer_asserted.sort_by_key(cell_key);
    verdict_flips.sort_by_key(|f| (f.concern_id.clone(), f.file_path.clone()));
    let theme_key_sort = |r: &FindingThemeRef| (r.concern_id.clone(), r.theme.clone());
    findings_appeared.sort_by_key(theme_key_sort);
    findings_disappeared.sort_by_key(theme_key_sort);
    newly_touched.sort();
    no_longer_touched.sort();

    Ok(RunDiff {
        base_runs,
        compare_runs,
        newly_asserted,
        no_longer_asserted,
        verdict_flips,
        findings_appeared,
        findings_disappeared,
        newly_touched,
        no_longer_touched,
    })
}

/// List every run on a target with its identity and contribution counts,
/// most-recent-first.
pub fn list_runs(paths: &CoveragePaths) -> Result<Vec<RunListEntry>, DiffError> {
    let files = read_file_events(paths)?;
    let assertions = read_concern_assertions(paths)?;
    let findings = read_findings(paths)?;
    let ordered = ordered_runs(&files, &assertions, &findings);

    let mut out = Vec::with_capacity(ordered.len());
    for run_id in ordered {
        let single: BTreeSet<String> = [run_id.clone()].into_iter().collect();
        let c = contribution(&single, &files, &assertions, &findings);

        // Identity (model, surface) and earliest timestamp from any of the
        // run's ledger rows. Every row for a run carries the same model +
        // surface, so the first match is representative.
        let mut model = String::new();
        let mut surface = Surface::Session;
        let mut started_at: Option<DateTime<Utc>> = None;
        let mut consider = |attr_run: &str, m: &str, s: Surface, at: DateTime<Utc>| {
            if attr_run == run_id {
                if model.is_empty() {
                    model = m.to_string();
                    surface = s;
                }
                started_at = Some(match started_at {
                    Some(cur) if cur <= at => cur,
                    _ => at,
                });
            }
        };
        for f in &files {
            let a = f.attribution();
            consider(&a.run_id, &a.model, a.surface, f.at());
        }
        for a in &assertions {
            consider(
                &a.declared_by.run_id,
                &a.declared_by.model,
                a.declared_by.surface,
                a.declared_at,
            );
        }
        for f in &findings {
            consider(
                &f.declared_by.run_id,
                &f.declared_by.model,
                f.declared_by.surface,
                f.declared_at,
            );
        }

        out.push(RunListEntry {
            run_id,
            started_at: started_at.unwrap_or_else(Utc::now),
            model,
            surface,
            cells_asserted: c.cells.len(),
            findings: c.finding_themes.len(),
            files_touched: c.touched.len(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::types::Severity;
    use crate::ledger::events::{Attribution, Evidence, FindingEvidence, FindingScope, Surface};
    use crate::ledger::paths::CoveragePaths;

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

    fn assertion(
        run: &str,
        concern: &str,
        file: &str,
        status: AssertionStatus,
        secs: i64,
    ) -> ConcernAssertion {
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

    fn write_ledgers(
        paths: &CoveragePaths,
        files: &[FileTouchEvent],
        assertions: &[ConcernAssertion],
        findings: &[FindingRecord],
    ) {
        paths.ensure_dir().unwrap();
        let f: String = files
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        std::fs::write(&paths.files, f).unwrap();
        let a: String = assertions
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        std::fs::write(&paths.concerns, a).unwrap();
        let fi: String = findings
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        std::fs::write(&paths.findings, fi).unwrap();
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
        assert!(!c
            .cells
            .contains_key(&("c2".to_string(), "src/b.rs".to_string())));
        // Touched paths come only from run_a.
        assert!(c.touched.contains("src/a.rs"));
        assert!(c.touched.contains("src/only_a.rs"));
        // Finding themes are (concern_id, theme_key); run_b's is excluded.
        assert!(c.finding_themes.contains(&(
            Some("c1".to_string()),
            theme_key("sql injection in login handler path")
        )));
        assert_eq!(c.finding_themes.len(), 1);
    }

    #[test]
    fn run_diff_reports_all_four_dimensions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");

        let files = vec![
            read_event("run_old", 100), // a.rs
            FileTouchEvent::Read {
                path: "src/b.rs".to_string(),
                line_range: [1, 5],
                tool: "read_file".to_string(),
                attribution: attribution("run_old"),
                at: DateTime::<Utc>::from_timestamp(101, 0).unwrap(),
            },
            read_event("run_new", 200), // a.rs
            FileTouchEvent::Read {
                path: "src/c.rs".to_string(),
                line_range: [1, 5],
                tool: "read_file".to_string(),
                attribution: attribution("run_new"),
                at: DateTime::<Utc>::from_timestamp(201, 0).unwrap(),
            },
        ];
        let assertions = vec![
            assertion("run_old", "c1", "src/a.rs", AssertionStatus::Clean, 100),
            assertion("run_old", "c2", "src/b.rs", AssertionStatus::Clean, 101),
            assertion("run_new", "c1", "src/a.rs", AssertionStatus::Finding, 200),
            assertion("run_new", "c3", "src/c.rs", AssertionStatus::Clean, 201),
        ];
        let findings = vec![
            finding("run_old", Some("c1"), "alpha alpha alpha alpha alpha alpha"),
            finding("run_new", Some("c1"), "beta beta beta beta beta beta"),
        ];
        write_ledgers(&paths, &files, &assertions, &findings);

        let diff = run_diff(&paths, &RunSelector::Previous, &RunSelector::Latest).unwrap();

        assert_eq!(diff.base_runs, vec!["run_old"]);
        assert_eq!(diff.compare_runs, vec!["run_new"]);
        assert!(diff
            .newly_asserted
            .iter()
            .any(|c| c.concern_id == "c3" && c.file_path == "src/c.rs"));
        assert!(diff
            .no_longer_asserted
            .iter()
            .any(|c| c.concern_id == "c2" && c.file_path == "src/b.rs"));
        let flip = diff
            .verdict_flips
            .iter()
            .find(|f| f.concern_id == "c1")
            .unwrap();
        assert_eq!(flip.base_status, AssertionStatus::Clean);
        assert_eq!(flip.compare_status, AssertionStatus::Finding);
        assert!(flip.high_signal);
        assert!(diff
            .findings_appeared
            .iter()
            .any(|f| f.theme.starts_with("beta")));
        assert!(diff
            .findings_disappeared
            .iter()
            .any(|f| f.theme.starts_with("alpha")));
        assert!(diff.newly_touched.contains(&"src/c.rs".to_string()));
        assert!(diff.no_longer_touched.contains(&"src/b.rs".to_string()));
        assert!(!diff.is_empty());
    }

    #[test]
    fn run_diff_identical_runs_is_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        let files = vec![read_event("run_old", 100), read_event("run_new", 200)];
        let assertions = vec![
            assertion("run_old", "c1", "src/a.rs", AssertionStatus::Clean, 100),
            assertion("run_new", "c1", "src/a.rs", AssertionStatus::Clean, 200),
        ];
        write_ledgers(&paths, &files, &assertions, &[]);
        let diff = run_diff(&paths, &RunSelector::Previous, &RunSelector::Latest).unwrap();
        assert!(diff.is_empty());
    }

    #[test]
    fn list_runs_reports_counts_most_recent_first() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        let files = vec![read_event("run_old", 100), read_event("run_new", 200)];
        let assertions = vec![
            assertion("run_old", "c1", "src/a.rs", AssertionStatus::Clean, 100),
            assertion("run_new", "c1", "src/a.rs", AssertionStatus::Finding, 200),
            assertion("run_new", "c2", "src/b.rs", AssertionStatus::Clean, 201),
        ];
        let findings = vec![finding(
            "run_new",
            Some("c1"),
            "something something here now ok yes",
        )];
        write_ledgers(&paths, &files, &assertions, &findings);

        let runs = list_runs(&paths).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].run_id, "run_new");
        assert_eq!(runs[1].run_id, "run_old");
        assert_eq!(runs[0].cells_asserted, 2);
        assert_eq!(runs[0].findings, 1);
        assert_eq!(runs[0].files_touched, 1);
        assert_eq!(
            runs[0].started_at,
            DateTime::<Utc>::from_timestamp(200, 0).unwrap()
        );
    }
}
