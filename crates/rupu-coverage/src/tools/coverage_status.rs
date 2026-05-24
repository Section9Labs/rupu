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
    Ok(all
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
}
