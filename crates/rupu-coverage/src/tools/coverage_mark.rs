use crate::catalog::types::{FlatCatalog, TouchStrength};
use crate::ledger::events::{
    AssertionStatus, Attribution, ConcernAssertion, Evidence,
};
use crate::ledger::paths::CoveragePaths;
use crate::ledger::views::{file_views, read_file_events};
use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageMarkInput {
    pub concern_id: String,
    pub file_path: String,
    pub status: AssertionStatus,
    pub evidence: Evidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageMarkOutput {
    pub ok: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CoverageMarkError {
    #[error("unknown concern_id `{0}` — must be declared in the effective catalog")]
    UnknownConcernId(String),
    #[error("file `{file}` was never read at min_strength `{required:?}` — call read_file first or use status `not_applicable`")]
    FileNotExamined {
        file: String,
        required: TouchStrength,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

pub async fn coverage_mark(
    paths: &CoveragePaths,
    catalog: &FlatCatalog,
    attribution: Attribution,
    input: CoverageMarkInput,
) -> Result<CoverageMarkOutput, CoverageMarkError> {
    // Validation 1: concern_id must exist in the catalog.
    let concern = catalog
        .concerns
        .iter()
        .find(|c| c.id == input.concern_id)
        .ok_or_else(|| CoverageMarkError::UnknownConcernId(input.concern_id.clone()))?;

    // Validation 2: file must have been read (or any qualifying touch),
    // unless status is `not_applicable`.
    if input.status != AssertionStatus::NotApplicable {
        let events = read_file_events(paths)?;
        let views = file_views(&events);
        let view = views.iter().find(|v| v.path == input.file_path);
        let touched = view.map(|v| v.strongest).unwrap_or(TouchStrength::Glob);
        if touched < concern.min_strength {
            return Err(CoverageMarkError::FileNotExamined {
                file: input.file_path.clone(),
                required: concern.min_strength,
            });
        }
    }

    // Validation 3 (warn-only): status=Finding with empty finding_ids.
    let mut warnings = Vec::new();
    if input.status == AssertionStatus::Finding && input.evidence.finding_ids.is_empty() {
        warnings.push(
            "status=finding with no finding_ids — call report_finding first or attach the id"
                .to_string(),
        );
    }

    let assertion = ConcernAssertion {
        concern_id: input.concern_id,
        file_path: input.file_path,
        status: input.status,
        evidence: input.evidence,
        declared_by: attribution,
        declared_at: Utc::now(),
    };
    paths.ensure_dir()?;
    let line = serde_json::to_string(&assertion)?;
    let body = format!("{line}\n");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.concerns)?;
    use std::io::Write;
    f.write_all(body.as_bytes())?;
    f.flush()?;

    Ok(CoverageMarkOutput { ok: true, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::flatten::flatten;
    use crate::catalog::types::{ConcernsBlock, ConcernsEntry, IncludeDirective};
    use crate::ledger::events::{FileTouchEvent, Surface};
    use crate::ledger::writer::CoverageWriterHandle;

    fn attribution() -> Attribution {
        Attribution {
            run_id: "run_t".to_string(),
            model: "m".to_string(),
            surface: Surface::Workflow,
        }
    }

    async fn touch_file_as_read(paths: &CoveragePaths, rel: &str) {
        let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();
        handle
            .writer
            .record_file_touch(FileTouchEvent::Read {
                path: rel.to_string(),
                line_range: [1, 100],
                tool: "read_file".to_string(),
                attribution: attribution(),
                at: Utc::now(),
            })
            .await;
        handle.shutdown().await;
    }

    fn stride_block() -> ConcernsBlock {
        ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: crate::catalog::types::CatalogMode::Auto,
                filter: None,
            })],
        }
    }

    #[tokio::test]
    async fn happy_path_clean_assertion_persists() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let catalog = flatten(&stride_block()).unwrap();
        touch_file_as_read(&paths, "src/auth/login.rs").await;

        let out = coverage_mark(
            &paths,
            &catalog,
            attribution(),
            CoverageMarkInput {
                concern_id: "stride:spoofing".to_string(),
                file_path: "src/auth/login.rs".to_string(),
                status: AssertionStatus::Clean,
                evidence: Evidence {
                    summary: "OK".to_string(),
                    line_ranges: vec![[1, 80]],
                    finding_ids: vec![],
                },
            },
        )
        .await
        .unwrap();
        assert!(out.ok);
        assert!(out.warnings.is_empty());

        let body = std::fs::read_to_string(&paths.concerns).unwrap();
        let assertion: ConcernAssertion = serde_json::from_str(body.trim()).unwrap();
        assert_eq!(assertion.concern_id, "stride:spoofing");
    }

    #[tokio::test]
    async fn rejects_unknown_concern_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let catalog = flatten(&stride_block()).unwrap();
        touch_file_as_read(&paths, "x.rs").await;

        let err = coverage_mark(
            &paths,
            &catalog,
            attribution(),
            CoverageMarkInput {
                concern_id: "not-real".to_string(),
                file_path: "x.rs".to_string(),
                status: AssertionStatus::Clean,
                evidence: Evidence {
                    summary: "x".to_string(),
                    line_ranges: vec![],
                    finding_ids: vec![],
                },
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, CoverageMarkError::UnknownConcernId(_)));
    }

    #[tokio::test]
    async fn rejects_clean_when_file_not_read() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let catalog = flatten(&stride_block()).unwrap();
        // Do NOT touch the file.

        let err = coverage_mark(
            &paths,
            &catalog,
            attribution(),
            CoverageMarkInput {
                concern_id: "stride:spoofing".to_string(),
                file_path: "unread.rs".to_string(),
                status: AssertionStatus::Clean,
                evidence: Evidence {
                    summary: "x".to_string(),
                    line_ranges: vec![],
                    finding_ids: vec![],
                },
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, CoverageMarkError::FileNotExamined { .. }));
    }

    #[tokio::test]
    async fn allows_not_applicable_without_read() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let catalog = flatten(&stride_block()).unwrap();
        let out = coverage_mark(
            &paths,
            &catalog,
            attribution(),
            CoverageMarkInput {
                concern_id: "stride:spoofing".to_string(),
                file_path: "trivially-na.rs".to_string(),
                status: AssertionStatus::NotApplicable,
                evidence: Evidence {
                    summary: "wrong language".to_string(),
                    line_ranges: vec![],
                    finding_ids: vec![],
                },
            },
        )
        .await
        .unwrap();
        assert!(out.ok);
    }

    #[tokio::test]
    async fn finding_without_finding_ids_warns() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let catalog = flatten(&stride_block()).unwrap();
        touch_file_as_read(&paths, "x.rs").await;
        let out = coverage_mark(
            &paths,
            &catalog,
            attribution(),
            CoverageMarkInput {
                concern_id: "stride:spoofing".to_string(),
                file_path: "x.rs".to_string(),
                status: AssertionStatus::Finding,
                evidence: Evidence {
                    summary: "issue here".to_string(),
                    line_ranges: vec![],
                    finding_ids: vec![],
                },
            },
        )
        .await
        .unwrap();
        assert!(out.ok);
        assert_eq!(out.warnings.len(), 1);
    }
}
