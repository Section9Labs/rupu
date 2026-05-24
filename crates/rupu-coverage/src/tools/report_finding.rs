use crate::catalog::types::Severity;
use crate::ledger::events::{Attribution, FindingEvidence, FindingRecord, FindingScope};
use crate::ledger::paths::CoveragePaths;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportFindingInput {
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub line_range: Option<[u32; 2]>,
    pub scope: FindingScope,
    pub summary: String,
    pub severity: Severity,
    #[serde(default)]
    pub concern_id: Option<String>,
    pub evidence: FindingEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportFindingOutput {
    pub id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ReportFindingError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}

pub fn report_finding(
    paths: &CoveragePaths,
    attribution: Attribution,
    input: ReportFindingInput,
) -> Result<ReportFindingOutput, ReportFindingError> {
    let id = format!("fnd_{}", Ulid::new());
    let record = FindingRecord {
        id: id.clone(),
        file_path: input.file_path,
        line_range: input.line_range,
        scope: input.scope,
        summary: input.summary,
        severity: input.severity,
        concern_id: input.concern_id,
        evidence: input.evidence,
        declared_by: attribution,
        declared_at: Utc::now(),
    };
    paths.ensure_dir()?;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.findings)?;
    let line = serde_json::to_string(&record)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    f.flush()?;
    Ok(ReportFindingOutput { id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::Surface;

    fn attribution() -> Attribution {
        Attribution {
            run_id: "r".to_string(),
            model: "m".to_string(),
            surface: Surface::Workflow,
        }
    }

    #[test]
    fn appends_finding_and_returns_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let out = report_finding(
            &paths,
            attribution(),
            ReportFindingInput {
                file_path: Some("src/config.rs".to_string()),
                line_range: Some([20, 28]),
                scope: FindingScope::Line,
                summary: "Hardcoded API key.".to_string(),
                severity: Severity::High,
                concern_id: Some("secrets-in-source".to_string()),
                evidence: FindingEvidence {
                    code_excerpt: Some("const X = \"...\";".to_string()),
                    rationale: "Key in source.".to_string(),
                    references: vec![],
                },
            },
        )
        .unwrap();
        assert!(out.id.starts_with("fnd_"));
        let body = std::fs::read_to_string(&paths.findings).unwrap();
        assert_eq!(body.lines().count(), 1);
    }

    #[test]
    fn accepts_null_concern_for_serendipitous_finding() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let out = report_finding(
            &paths,
            attribution(),
            ReportFindingInput {
                file_path: None,
                line_range: None,
                scope: FindingScope::Repo,
                summary: "Spotted while looking for something else.".to_string(),
                severity: Severity::Low,
                concern_id: None,
                evidence: FindingEvidence {
                    code_excerpt: None,
                    rationale: "ad-hoc".to_string(),
                    references: vec![],
                },
            },
        )
        .unwrap();
        assert!(out.id.starts_with("fnd_"));
    }

    /// Verify that all three JSON strings that the `report_finding` tool schema
    /// advertises ("line", "file", "repo") deserialize cleanly into `FindingScope`.
    /// If the schema enum and the Rust enum ever diverge, serde will reject the
    /// value with an obscure error rather than a compile-time failure — this test
    /// catches that mismatch at the unit level before it affects LLM calls.
    #[test]
    fn all_schema_scope_values_deserialize_to_finding_scope() {
        for (json_str, expected) in [
            ("\"line\"", FindingScope::Line),
            ("\"file\"", FindingScope::File),
            ("\"repo\"", FindingScope::Repo),
        ] {
            let decoded: FindingScope = serde_json::from_str(json_str)
                .unwrap_or_else(|e| panic!("failed to deserialize scope {json_str:?}: {e}"));
            assert_eq!(
                decoded, expected,
                "scope {json_str:?} should round-trip cleanly"
            );
        }
    }
}
