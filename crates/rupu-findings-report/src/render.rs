//! The public render API: blocks → bytes in a chosen format, plus the
//! per-finding split archive.

use crate::blocks::{finding_blocks, index_blocks, project_blocks, Block};
use crate::model::{ExportFinding, ReportMeta};
use crate::number::{filename, number_map, title};
use crate::{html, markdown};
use std::collections::{HashMap, HashSet};
use std::io::Write;

#[cfg(feature = "pdf")]
use crate::{pdf, typst_doc};

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("PDF rendering failed: {0}")]
    Typst(String),
    /// `Format::Pdf` was requested from a build without the `pdf` feature.
    #[error("this build of rupu was compiled without PDF support")]
    PdfUnavailable,
    #[error("zip failed: {0}")]
    Zip(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Html,
    /// Compiled in-process from the bundled fonts only, none of which covers
    /// CJK or emoji: those characters render as missing-glyph boxes. Needs the
    /// `pdf` cargo feature (default); without it rendering returns
    /// [`ExportError::PdfUnavailable`].
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

    /// Whether this build can render the format: everything but PDF always,
    /// PDF only with the `pdf` cargo feature. Lets a caller refuse a request
    /// up front instead of after collecting its findings.
    pub fn is_available(self) -> bool {
        self != Format::Pdf || cfg!(feature = "pdf")
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

/// Fail fast, before any work (and even for an empty selection), when the
/// requested format is not compiled into this build.
fn ensure_supported(fmt: Format) -> Result<(), ExportError> {
    if !fmt.is_available() {
        return Err(ExportError::PdfUnavailable);
    }
    Ok(())
}

#[cfg(feature = "pdf")]
fn pdf_bytes(blocks: &[Block]) -> Result<Vec<u8>, ExportError> {
    pdf::render_pdf(typst_doc::render(blocks))
}

#[cfg(not(feature = "pdf"))]
fn pdf_bytes(_blocks: &[Block]) -> Result<Vec<u8>, ExportError> {
    Err(ExportError::PdfUnavailable)
}

fn emit(doc_title: &str, blocks: &[Block], fmt: Format) -> Result<Vec<u8>, ExportError> {
    Ok(match fmt {
        Format::Markdown => markdown::render(blocks).into_bytes(),
        Format::Html => html::render(doc_title, blocks).into_bytes(),
        Format::Pdf => pdf_bytes(blocks)?,
    })
}

/// The message inside a Typst error, without the "PDF rendering failed"
/// prefix its `Display` adds (the caller wraps it in its own sentence).
#[cfg(feature = "pdf")]
fn detail(e: ExportError) -> String {
    match e {
        ExportError::Typst(m) => m,
        other => other.to_string(),
    }
}

/// Say which finding a render error belongs to.
fn name_finding(number: &str, e: ExportError) -> ExportError {
    match e {
        ExportError::Typst(m) => {
            ExportError::Typst(format!("finding {number} could not be rendered: {m}"))
        }
        other => other,
    }
}

pub fn render_finding(
    f: &ExportFinding,
    numbers: &HashMap<String, String>,
    fmt: Format,
) -> Result<Vec<u8>, ExportError> {
    ensure_supported(fmt)?;
    emit(
        &format!("{} - {}", f.number, title(f)),
        &finding_blocks(f, numbers),
        fmt,
    )
}

/// A whole-project PDF. When the combined document fails to compile, each
/// finding is compiled on its own (failure path only) so the error names the
/// finding(s) responsible, with each one's own diagnostic, instead of leaving
/// the operator to bisect.
#[cfg(feature = "pdf")]
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
    let failing: Vec<(&str, String)> = findings
        .iter()
        .filter_map(|f| {
            compile(typst_doc::render(&finding_blocks(f, &numbers)))
                .err()
                .map(|e| (f.number.as_str(), detail(e)))
        })
        .collect();
    match failing.as_slice() {
        // Only the combination fails: the combined diagnostic is all there is.
        [] => Err(err),
        [(number, msg)] => Err(name_finding(number, ExportError::Typst(msg.clone()))),
        many => Err(ExportError::Typst(format!(
            "{} findings could not be rendered: {}",
            many.len(),
            many.iter()
                .map(|(n, m)| format!("{n} ({m})"))
                .collect::<Vec<_>>()
                .join("; ")
        ))),
    }
}

pub fn render_project(
    meta: &ReportMeta,
    findings: &[ExportFinding],
    fmt: Format,
) -> Result<Vec<u8>, ExportError> {
    ensure_supported(fmt)?;
    #[cfg(feature = "pdf")]
    {
        if fmt == Format::Pdf {
            return project_pdf(meta, findings, pdf::render_pdf);
        }
    }
    emit(&meta.title, &project_blocks(meta, findings), fmt)
}

/// A zip entry name that is one flat file name whatever the caller put in the
/// finding number. `number::filename` already cleans it; this is the last line
/// of defence at the point a path is actually written: separators, drive
/// colons and control characters become `_`, and leading dots go, so the entry
/// can never be a parent-directory or absolute path, or a hidden file.
fn safe_entry_name(name: &str) -> String {
    let flat: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    match flat.trim_start_matches('.') {
        "" => "finding".to_string(),
        n => n.to_string(),
    }
}

/// `name`, or `name (2)`, `name (3)`... (before the extension) when `used`
/// already holds it. Findings are numbered per project, so a multi-project
/// export can repeat a number and a title; the zip format rejects two entries
/// with one name. The comparison is case-insensitive because the archive is
/// meant to be extracted on case-insensitive file systems too.
fn unique_entry_name(name: String, used: &mut HashSet<String>) -> String {
    if used.insert(name.to_lowercase()) {
        return name;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name.as_str(), ""),
    };
    let mut n = 2;
    loop {
        let candidate = format!("{stem} ({n}){ext}");
        if used.insert(candidate.to_lowercase()) {
            return candidate;
        }
        n += 1;
    }
}

/// One file per finding (named by [`filename`]) plus `index.md`, the Markdown
/// project index. The index stays Markdown whatever the finding format is.
pub fn render_split_zip(
    meta: &ReportMeta,
    findings: &[ExportFinding],
    fmt: Format,
) -> Result<Vec<u8>, ExportError> {
    ensure_supported(fmt)?;
    build_zip(meta, findings, fmt, render_finding)
}

/// [`render_split_zip`] with the per-finding renderer injected, so the
/// "which finding failed" reporting can be tested without a compile failure.
fn build_zip(
    meta: &ReportMeta,
    findings: &[ExportFinding],
    fmt: Format,
    render_one: impl Fn(
        &ExportFinding,
        &HashMap<String, String>,
        Format,
    ) -> Result<Vec<u8>, ExportError>,
) -> Result<Vec<u8>, ExportError> {
    let numbers = number_map(findings);
    let zip_err = |e: zip::result::ZipError| ExportError::Zip(e.to_string());
    let io_err = |e: std::io::Error| ExportError::Zip(e.to_string());
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    let mut used = HashSet::new();

    let index = unique_entry_name("index.md".to_string(), &mut used);
    zip.start_file(index, opts).map_err(zip_err)?;
    zip.write_all(markdown::render(&index_blocks(meta, findings)).as_bytes())
        .map_err(io_err)?;
    for f in findings {
        let bytes = render_one(f, &numbers, fmt).map_err(|e| name_finding(&f.number, e))?;
        let name = unique_entry_name(safe_entry_name(&filename(f, fmt.ext())), &mut used);
        zip.start_file(name, opts).map_err(zip_err)?;
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

    #[test]
    fn entry_names_are_flat_and_never_hidden_or_absolute() {
        for (raw, want) in [
            ("SEC-001 - t.md", "SEC-001 - t.md"),
            ("../../evil.md", "_.._evil.md"),
            ("a/b\\c.md", "a_b_c.md"),
            ("/etc/passwd", "_etc_passwd"),
            ("C:\\x.md", "C__x.md"),
            ("..", "finding"),
            (".hidden", "hidden"),
            ("...", "finding"),
            ("", "finding"),
            ("a\nb\u{0}c.md", "a_b_c.md"),
        ] {
            let got = safe_entry_name(raw);
            assert_eq!(got, want, "{raw:?}");
            assert!(!got.contains(['/', '\\', ':']), "{got}");
            assert!(!got.starts_with('.'), "{got}");
        }
    }

    #[test]
    fn repeated_entry_names_get_a_numbered_suffix_before_the_extension() {
        let mut used = HashSet::new();
        let mut take = |n: &str| unique_entry_name(n.to_string(), &mut used);
        assert_eq!(take("SEC-001 - t.md"), "SEC-001 - t.md");
        assert_eq!(take("SEC-001 - t.md"), "SEC-001 - t (2).md");
        assert_eq!(take("SEC-001 - t.md"), "SEC-001 - t (3).md");
        // Case-insensitive (names differing only by case collide on macOS),
        // and the suffixed forms are taken into account too: "t (2)" and
        // "t (3)" are used, so the next free case-insensitive name is (4).
        assert_eq!(take("SEC-001 - T.md"), "SEC-001 - T (4).md");
        assert_eq!(take("noext"), "noext");
        assert_eq!(take("noext"), "noext (2)");
    }

    #[test]
    fn errors_name_the_finding_and_only_retag_typst_errors() {
        let e = name_finding("SEC-009", ExportError::Typst("boom".into()));
        assert_eq!(
            e.to_string(),
            "PDF rendering failed: finding SEC-009 could not be rendered: boom"
        );
        assert!(matches!(
            name_finding("SEC-009", ExportError::PdfUnavailable),
            ExportError::PdfUnavailable
        ));
    }

    #[test]
    fn a_split_zip_failure_names_the_finding_whose_render_failed() {
        let all = vec![finding(1, "fine"), finding(2, "bad"), finding(3, "fine")];
        let err = build_zip(&meta(), &all, Format::Pdf, |f, _, _| {
            if f.number == "SEC-002" {
                Err(ExportError::Typst("expected function".into()))
            } else {
                Ok(b"%PDF".to_vec())
            }
        })
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "PDF rendering failed: finding SEC-002 could not be rendered: expected function"
        );
    }

    #[cfg(feature = "pdf")]
    mod project_pdf {
        use super::*;
        use std::cell::Cell;

        /// A stand-in compiler: `POISON-A` / `POISON-B` in the markup make it
        /// fail with a diagnostic of that name (A wins when both are present,
        /// as a combined document would report its first error).
        fn fake(markup: String) -> Result<Vec<u8>, ExportError> {
            if markup.contains("POISON-A") {
                Err(ExportError::Typst("bad A".into()))
            } else if markup.contains("POISON-B") {
                Err(ExportError::Typst("bad B".into()))
            } else {
                Ok(b"%PDF-fake".to_vec())
            }
        }

        #[test]
        fn a_failing_project_pdf_names_each_finding_with_its_own_diagnostic() {
            let all = vec![
                finding(1, "fine"),
                finding(2, "POISON-A"),
                finding(3, "fine"),
                finding(4, "POISON-B"),
            ];
            let err = project_pdf(&meta(), &all, fake).unwrap_err().to_string();
            // Each finding carries the diagnostic from ITS OWN compile; the
            // combined document would have reported "bad A" for both.
            assert!(err.contains("SEC-002 (bad A)"), "{err}");
            assert!(err.contains("SEC-004 (bad B)"), "{err}");
            assert!(err.contains("2 findings could not be rendered"), "{err}");
            assert!(
                !err.contains("SEC-001") && !err.contains("SEC-003"),
                "{err}"
            );
        }

        #[test]
        fn a_single_bad_finding_is_named_in_the_singular() {
            // Only SEC-002 is broken; the fault is reported once.
            let all = vec![finding(1, "fine"), finding(2, "POISON-B")];
            let err = project_pdf(&meta(), &all, fake).unwrap_err().to_string();
            assert_eq!(
                err,
                "PDF rendering failed: finding SEC-002 could not be rendered: bad B"
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
}
