use crate::ledger::events::ConcernAssertion;
use crate::ledger::paths::CoveragePaths;
use crate::ledger::views::read_concern_assertions;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CoverageStatusInput {
    #[serde(default)]
    pub concern_id: Option<String>,
    #[serde(default)]
    pub file_path_prefix: Option<String>,
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
}

pub fn coverage_status(
    paths: &CoveragePaths,
    input: CoverageStatusInput,
) -> std::io::Result<Vec<ConcernAssertion>> {
    let all = read_concern_assertions(paths)?;

    // Within-run supersede: for each (concern_id, file_path, run_id), keep only
    // the last occurrence (most recent in append order).  Cross-run entries with
    // the same concern+file but a different run_id are intentionally preserved so
    // callers can observe multi-run disagreement.
    let deduped = {
        use std::collections::HashMap;
        // Map from (concern_id, file_path, run_id) → index into `out`.
        let mut index: HashMap<(String, String, String), usize> = HashMap::new();
        let mut out: Vec<ConcernAssertion> = Vec::with_capacity(all.len());
        for a in all {
            let key = (
                a.concern_id.clone(),
                a.file_path.clone(),
                a.declared_by.run_id.clone(),
            );
            if let Some(&pos) = index.get(&key) {
                // Replace the earlier entry in-place; preserve first-appearance order.
                out[pos] = a;
            } else {
                index.insert(key, out.len());
                out.push(a);
            }
        }
        out
    };

    Ok(deduped
        .into_iter()
        .filter(|a| {
            input.concern_id.as_deref().is_none_or(|c| a.concern_id == c)
                && input
                    .file_path_prefix
                    .as_deref()
                    .is_none_or(|p| a.file_path.starts_with(p))
                && input.since.is_none_or(|s| a.declared_at >= s)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{AssertionStatus, Attribution, Evidence, Surface};

    fn assertion(concern: &str, file: &str) -> ConcernAssertion {
        ConcernAssertion {
            concern_id: concern.to_string(),
            file_path: file.to_string(),
            status: AssertionStatus::Clean,
            evidence: Evidence {
                summary: "x".to_string(),
                line_ranges: vec![],
                finding_ids: vec![],
            },
            declared_by: Attribution {
                run_id: "r".to_string(),
                model: "m".to_string(),
                surface: Surface::Workflow,
            },
            declared_at: Utc::now(),
        }
    }

    fn write_jsonl(paths: &CoveragePaths, assertions: &[ConcernAssertion]) {
        paths.ensure_dir().unwrap();
        let body: String = assertions
            .iter()
            .map(|a| serde_json::to_string(a).unwrap() + "\n")
            .collect();
        std::fs::write(&paths.concerns, body).unwrap();
    }

    #[test]
    fn filters_by_concern_id_and_prefix() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        write_jsonl(
            &paths,
            &[
                assertion("ssrf", "src/handlers/users.rs"),
                assertion("ssrf", "src/db/queries.rs"),
                assertion("sqli", "src/handlers/admin.rs"),
            ],
        );
        let results = coverage_status(
            &paths,
            CoverageStatusInput {
                concern_id: Some("ssrf".to_string()),
                file_path_prefix: Some("src/handlers/".to_string()),
                since: None,
            },
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file_path, "src/handlers/users.rs");
    }

    #[test]
    fn within_run_remark_supersedes_earlier() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        // Same concern+file+run, marked twice: examined then clean.
        let mut a1 = assertion("ssrf", "src/a.rs");
        a1.status = AssertionStatus::Examined;
        let mut a2 = assertion("ssrf", "src/a.rs");
        a2.status = AssertionStatus::Clean;
        // both same run_id (the assertion() helper uses run_id "r")
        write_jsonl(&paths, &[a1, a2]);
        let results = coverage_status(&paths, CoverageStatusInput::default()).unwrap();
        assert_eq!(results.len(), 1, "re-mark should supersede");
        assert_eq!(results[0].status, AssertionStatus::Clean);
    }

    #[test]
    fn cross_run_assertions_are_preserved() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let mut a1 = assertion("ssrf", "src/a.rs");
        a1.declared_by.run_id = "run_A".to_string();
        a1.status = AssertionStatus::Clean;
        let mut a2 = assertion("ssrf", "src/a.rs");
        a2.declared_by.run_id = "run_B".to_string();
        a2.status = AssertionStatus::Finding;
        write_jsonl(&paths, &[a1, a2]);
        let results = coverage_status(&paths, CoverageStatusInput::default()).unwrap();
        assert_eq!(results.len(), 2, "different runs must both be kept");
    }
}
