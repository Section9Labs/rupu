//! The public render API: blocks → bytes in a chosen format, plus the
//! per-finding split archive.

use crate::blocks::{finding_blocks, index_blocks, project_blocks, Block};
use crate::model::{ExportFinding, ReportMeta};
use crate::number::{filename, number_map, title};
use crate::{html, markdown, pdf, typst_doc};
use std::collections::HashMap;
use std::io::Write;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("PDF rendering failed: {0}")]
    Typst(String),
    #[error("zip failed: {0}")]
    Zip(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Html,
    Pdf,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Html => "html",
            Format::Pdf => "pdf",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Format::Markdown => "text/markdown; charset=utf-8",
            Format::Html => "text/html; charset=utf-8",
            Format::Pdf => "application/pdf",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "md" | "markdown" => Some(Format::Markdown),
            "html" => Some(Format::Html),
            "pdf" => Some(Format::Pdf),
            _ => None,
        }
    }
}

fn emit(doc_title: &str, blocks: &[Block], fmt: Format) -> Result<Vec<u8>, ExportError> {
    Ok(match fmt {
        Format::Markdown => markdown::render(blocks).into_bytes(),
        Format::Html => html::render(doc_title, blocks).into_bytes(),
        Format::Pdf => pdf::render_pdf(typst_doc::render(blocks))?,
    })
}

pub fn render_finding(
    f: &ExportFinding,
    numbers: &HashMap<String, String>,
    fmt: Format,
) -> Result<Vec<u8>, ExportError> {
    emit(
        &format!("{} - {}", f.number, title(f)),
        &finding_blocks(f, numbers),
        fmt,
    )
}

/// A whole-project PDF. When the combined document fails to compile, each
/// finding is compiled on its own (failure path only) so the error names the
/// finding(s) responsible instead of leaving the operator to bisect.
fn project_pdf(
    meta: &ReportMeta,
    findings: &[ExportFinding],
    compile: impl Fn(String) -> Result<Vec<u8>, ExportError>,
) -> Result<Vec<u8>, ExportError> {
    let blocks = project_blocks(meta, findings);
    let err = match compile(typst_doc::render(&blocks)) {
        Ok(bytes) => return Ok(bytes),
        Err(e) => e,
    };
    let numbers = number_map(findings);
    let failing: Vec<&str> = findings
        .iter()
        .filter(|f| compile(typst_doc::render(&finding_blocks(f, &numbers))).is_err())
        .map(|f| f.number.as_str())
        .collect();
    if failing.is_empty() {
        return Err(err);
    }
    let detail = match &err {
        ExportError::Typst(m) => m.clone(),
        other => other.to_string(),
    };
    Err(ExportError::Typst(format!(
        "{} {} could not be rendered: {detail}",
        if failing.len() == 1 {
            "finding"
        } else {
            "findings"
        },
        failing.join(", ")
    )))
}

pub fn render_project(
    meta: &ReportMeta,
    findings: &[ExportFinding],
    fmt: Format,
) -> Result<Vec<u8>, ExportError> {
    match fmt {
        Format::Pdf => project_pdf(meta, findings, pdf::render_pdf),
        _ => emit(&meta.title, &project_blocks(meta, findings), fmt),
    }
}

/// One file per finding (named by [`filename`]) plus `index.md`, the Markdown
/// project index. The index stays Markdown whatever the finding format is.
pub fn render_split_zip(
    meta: &ReportMeta,
    findings: &[ExportFinding],
    fmt: Format,
) -> Result<Vec<u8>, ExportError> {
    let numbers = number_map(findings);
    let zip_err = |e: zip::result::ZipError| ExportError::Zip(e.to_string());
    let io_err = |e: std::io::Error| ExportError::Zip(e.to_string());
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();

    zip.start_file("index.md", opts).map_err(zip_err)?;
    zip.write_all(markdown::render(&index_blocks(meta, findings)).as_bytes())
        .map_err(io_err)?;
    for f in findings {
        let bytes = render_finding(f, &numbers, fmt)?;
        zip.start_file(filename(f, fmt.ext()), opts)
            .map_err(zip_err)?;
        zip.write_all(&bytes).map_err(io_err)?;
    }
    Ok(zip.finish().map_err(zip_err)?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExportInput;
    use chrono::{DateTime, Utc};
    use rupu_coverage::{
        Attribution, FindingEvidence, FindingProfile, FindingRecord, FindingScope, Severity,
        Surface,
    };
    use std::cell::Cell;

    fn ts() -> DateTime<Utc> {
        "2026-09-29T12:00:00Z".parse().unwrap()
    }

    fn finding(n: usize, summary: &str) -> ExportFinding {
        ExportFinding {
            number: format!("SEC-{n:03}"),
            input: ExportInput {
                ws_id: "p".into(),
                project: "p".into(),
                workflow_name: None,
                record: FindingRecord {
                    id: format!("fnd_{n}"),
                    file_path: None,
                    line_range: None,
                    target_ref: None,
                    scope: FindingScope::Repo,
                    summary: summary.into(),
                    severity: Severity::High,
                    concern_id: None,
                    evidence: FindingEvidence {
                        code_excerpt: None,
                        rationale: "r".into(),
                        references: vec![],
                    },
                    declared_by: Attribution {
                        run_id: "run_1".into(),
                        model: "m".into(),
                        surface: Surface::Workflow,
                    },
                    declared_at: ts(),
                    profile: FindingProfile::Summary,
                    report: None,
                },
            },
        }
    }

    fn meta() -> ReportMeta {
        ReportMeta {
            title: "T".into(),
            generated_at: ts(),
            scope: "s".into(),
        }
    }

    /// A stand-in compiler that rejects any document containing `POISON`.
    fn fake(markup: String) -> Result<Vec<u8>, ExportError> {
        if markup.contains("POISON") {
            Err(ExportError::Typst("unexpected token".into()))
        } else {
            Ok(b"%PDF-fake".to_vec())
        }
    }

    #[test]
    fn a_failing_project_pdf_names_the_finding_that_broke_it() {
        let all = vec![
            finding(1, "fine"),
            finding(2, "POISON"),
            finding(3, "fine"),
            finding(4, "POISON"),
        ];
        let err = project_pdf(&meta(), &all, fake).unwrap_err().to_string();
        assert!(err.contains("SEC-002") && err.contains("SEC-004"), "{err}");
        assert!(
            !err.contains("SEC-001") && !err.contains("SEC-003"),
            "{err}"
        );
        assert!(err.contains("unexpected token"), "{err}");
    }

    #[test]
    fn a_single_bad_finding_is_named_in_the_singular() {
        let all = vec![finding(1, "fine"), finding(2, "POISON")];
        let err = project_pdf(&meta(), &all, fake).unwrap_err().to_string();
        assert!(
            err.contains("finding SEC-002 could not be rendered"),
            "{err}"
        );
    }

    #[test]
    fn a_failure_no_single_finding_reproduces_keeps_the_original_error() {
        // Fails only on the combined document (it has the index heading).
        let all = vec![finding(1, "fine"), finding(2, "fine")];
        let err = project_pdf(&meta(), &all, |m| {
            if m.contains("Index") {
                Err(ExportError::Typst("combined only".into()))
            } else {
                Ok(vec![])
            }
        })
        .unwrap_err();
        assert!(
            matches!(&err, ExportError::Typst(m) if m == "combined only"),
            "{err}"
        );
    }

    #[test]
    fn a_successful_project_compiles_once_and_skips_the_per_finding_pass() {
        let calls = Cell::new(0);
        let all = vec![finding(1, "fine"), finding(2, "fine")];
        let out = project_pdf(&meta(), &all, |m| {
            calls.set(calls.get() + 1);
            fake(m)
        })
        .unwrap();
        assert_eq!(out, b"%PDF-fake");
        assert_eq!(calls.get(), 1);
    }
}
