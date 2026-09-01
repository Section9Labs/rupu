use crate::catalog::types::Severity;
use crate::ledger::events::{
    Attribution, FindingEvidence, FindingRecord, FindingScope, ScopeLocator,
};
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
    /// Host / endpoint / resource this finding is about, for the non-code
    /// scopes.
    #[serde(default)]
    pub target_ref: Option<String>,
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
    #[error("scope `{scope}` requires {needs}, but {got}")]
    Locator {
        scope: &'static str,
        needs: &'static str,
        got: &'static str,
    },
}

pub fn report_finding(
    paths: &CoveragePaths,
    attribution: Attribution,
    input: ReportFindingInput,
) -> Result<ReportFindingOutput, ReportFindingError> {
    validate_locator(&input)?;
    let id = format!("fnd_{}", Ulid::new());
    let record = FindingRecord {
        id: id.clone(),
        file_path: input.file_path,
        line_range: input.line_range,
        target_ref: input.target_ref,
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

/// A scope must actually point at something.
///
/// Without this, `scope` is decoration: a caller can declare `host` and give
/// no host, and the ledger accepts a finding nobody can act on. A finding
/// that says "something is wrong somewhere" is worse than no finding, because
/// it still costs a reader their attention.
///
/// Deliberately NOT enforced: that `file_path` is absent on a target scope,
/// or `target_ref` on a code scope. A finding can legitimately carry both --
/// a misconfiguration observed on a live host AND present in the Terraform
/// that produced it. Requiring the locator is a floor, not an exclusion.
fn validate_locator(input: &ReportFindingInput) -> Result<(), ReportFindingError> {
    let scope = input.scope.as_str();
    let missing = |needs, got| Err(ReportFindingError::Locator { scope, needs, got });
    match input.scope.locator() {
        ScopeLocator::None => Ok(()),
        ScopeLocator::File => match input.file_path {
            Some(_) => Ok(()),
            None => missing("a file_path", "none was given"),
        },
        ScopeLocator::FileAndLine => {
            if input.file_path.is_none() {
                return missing("a file_path and a line_range", "no file_path was given");
            }
            if input.line_range.is_none() {
                return missing("a file_path and a line_range", "no line_range was given");
            }
            Ok(())
        }
        ScopeLocator::Target => {
            let named = input
                .target_ref
                .as_deref()
                .is_some_and(|t| !t.trim().is_empty());
            if named {
                Ok(())
            } else {
                missing(
                    "a target_ref naming the host, endpoint or resource",
                    "none was given",
                )
            }
        }
    }
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

    fn input(scope: FindingScope) -> ReportFindingInput {
        ReportFindingInput {
            file_path: None,
            line_range: None,
            target_ref: None,
            scope,
            summary: "s".to_string(),
            severity: Severity::Medium,
            concern_id: None,
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".to_string(),
                references: vec![],
            },
        }
    }

    #[test]
    fn target_scopes_require_a_target_ref() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        for scope in [
            FindingScope::Host,
            FindingScope::Endpoint,
            FindingScope::Resource,
        ] {
            let err = report_finding(&paths, attribution(), input(scope))
                .expect_err("a target scope with no target_ref must be refused");
            assert!(
                matches!(err, ReportFindingError::Locator { .. }),
                "expected a locator error for {scope:?}, got {err:?}"
            );
        }
        // Nothing was written: a refused finding must not reach the ledger.
        assert!(!paths.findings.exists());
    }

    #[test]
    fn whitespace_only_target_ref_does_not_count_as_naming_a_target() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let mut i = input(FindingScope::Host);
        i.target_ref = Some("   ".to_string());
        assert!(matches!(
            report_finding(&paths, attribution(), i),
            Err(ReportFindingError::Locator { .. })
        ));
    }

    #[test]
    fn host_scope_with_a_target_ref_is_recorded() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let mut i = input(FindingScope::Host);
        i.target_ref = Some("identity.us-westjordan-1.example".to_string());
        report_finding(&paths, attribution(), i).expect("should record");
        let text = std::fs::read_to_string(&paths.findings).unwrap();
        let rec: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(rec["scope"], "host");
        assert_eq!(rec["target_ref"], "identity.us-westjordan-1.example");
        assert!(rec["file_path"].is_null());
    }

    #[test]
    fn code_scopes_still_require_their_file_locators() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        // file: no file_path
        assert!(matches!(
            report_finding(&paths, attribution(), input(FindingScope::File)),
            Err(ReportFindingError::Locator { .. })
        ));
        // line: file_path but no line_range
        let mut i = input(FindingScope::Line);
        i.file_path = Some("src/a.rs".to_string());
        assert!(matches!(
            report_finding(&paths, attribution(), i),
            Err(ReportFindingError::Locator { .. })
        ));
        // repo: locates nothing, needs nothing
        report_finding(&paths, attribution(), input(FindingScope::Repo))
            .expect("repo scope needs no locator");
    }

    #[test]
    fn a_legacy_record_without_target_ref_still_deserializes() {
        // The field is additive. Every finding written before it existed must
        // keep loading, or adding a scope variant silently orphans the
        // existing ledger.
        let legacy = r#"{"id":"fnd_1","file_path":"src/a.rs","line_range":[1,2],"scope":"line","summary":"s","severity":"high","concern_id":null,"evidence":{"code_excerpt":null,"rationale":"r","references":[]},"declared_by":{"run_id":"r","model":"m","surface":"workflow"},"declared_at":"2026-01-01T00:00:00Z"}"#;
        let rec: FindingRecord = serde_json::from_str(legacy).expect("legacy record must load");
        assert_eq!(rec.scope, FindingScope::Line);
        assert!(rec.target_ref.is_none());
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
                target_ref: None,
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
                target_ref: None,
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
