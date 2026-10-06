//! The public render API: blocks → bytes in a chosen format, plus the
//! per-finding split archive.

use crate::blocks::{finding_blocks, index_blocks, project_blocks, Block};
use crate::model::{Blobs, ExportFinding, ReportMeta};
use crate::number::{
    filename, fit_file_name, is_invisible_format, title, truncate_bytes, MAX_NAME_BYTES,
};
use crate::{html, markdown};
use chrono::{DateTime, Datelike, Timelike, Utc};
use std::collections::{HashMap, HashSet};
use std::io::Write;

#[cfg(feature = "pdf")]
use crate::pdf;
#[cfg(feature = "pdf")]
use crate::typst_doc::{self, TypstDoc};

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
    compile_blocks(blocks, &pdf::render_pdf)
}

/// Compile `blocks` with `compile`. Should that fail while the document embeds
/// images (failure path only), each image is compiled on its own, and every
/// one Typst cannot decode (magic bytes are only a sniff) becomes a note
/// saying so before the document is compiled again: a corrupt image costs its
/// own figure, not the export.
#[cfg(feature = "pdf")]
fn compile_blocks(
    blocks: &[Block],
    compile: &impl Fn(TypstDoc) -> Result<Vec<u8>, ExportError>,
) -> Result<Vec<u8>, ExportError> {
    let err = match compile(typst_doc::render_doc(blocks)) {
        Ok(bytes) => return Ok(bytes),
        Err(e) => e,
    };
    let mut undecodable: HashSet<&str> = HashSet::new();
    let mut decodable: HashSet<&str> = HashSet::new();
    for b in blocks {
        if let Block::Image { sha256, .. } = b {
            if undecodable.contains(sha256.as_str()) || decodable.contains(sha256.as_str()) {
                continue;
            }
            match compile(typst_doc::render_doc(std::slice::from_ref(b))) {
                Ok(_) => decodable.insert(sha256),
                Err(_) => undecodable.insert(sha256),
            };
        }
    }
    if undecodable.is_empty() {
        return Err(err);
    }
    let kept: Vec<Block> = blocks
        .iter()
        .map(|b| match b {
            Block::Image {
                caption, sha256, ..
            } if undecodable.contains(sha256.as_str()) => Block::Note(format!(
                "{caption} — not embedded: the image could not be decoded for the PDF"
            )),
            other => other.clone(),
        })
        .collect();
    compile(typst_doc::render_doc(&kept))
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

/// One finding as a stand-alone report. `blobs` is the local artifact store
/// an `image` evidence block's file is embedded from (HTML, PDF); pass
/// [`Blobs::NONE`] to show every file by reference only.
pub fn render_finding(
    f: &ExportFinding,
    numbers: &HashMap<String, String>,
    fmt: Format,
    blobs: Blobs<'_>,
) -> Result<Vec<u8>, ExportError> {
    ensure_supported(fmt)?;
    emit(
        &format!("{} - {}", f.number, title(f)),
        &finding_blocks(f, numbers, blobs),
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
    numbers: &HashMap<String, String>,
    blobs: Blobs<'_>,
    compile: impl Fn(TypstDoc) -> Result<Vec<u8>, ExportError>,
) -> Result<Vec<u8>, ExportError> {
    let numbers = crate::blocks::with_own_numbers(numbers, findings);
    let blocks = project_blocks(meta, findings, &numbers, blobs);
    let err = match compile_blocks(&blocks, &compile) {
        Ok(bytes) => return Ok(bytes),
        Err(e) => e,
    };
    let failing: Vec<(&str, String)> = findings
        .iter()
        .filter_map(|f| {
            compile_blocks(&finding_blocks(f, &numbers, blobs), &compile)
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

/// A whole-project report over `findings`. `numbers` maps finding ids to
/// display numbers for cross-references: pass the map of every finding the
/// selection was made from (see [`crate::blocks::project_blocks`]), so a
/// reference to a finding the selection left out still prints its number.
/// The findings' own numbers are always included. `blobs` is as for
/// [`render_finding`].
pub fn render_project(
    meta: &ReportMeta,
    findings: &[ExportFinding],
    numbers: &HashMap<String, String>,
    fmt: Format,
    blobs: Blobs<'_>,
) -> Result<Vec<u8>, ExportError> {
    ensure_supported(fmt)?;
    #[cfg(feature = "pdf")]
    {
        if fmt == Format::Pdf {
            return project_pdf(meta, findings, numbers, blobs, pdf::render_pdf);
        }
    }
    emit(
        &meta.title,
        &project_blocks(meta, findings, numbers, blobs),
        fmt,
    )
}

/// A zip entry name that is one flat file name whatever the caller put in the
/// finding number. `number::filename` already cleans it; this is the last line
/// of defence at the point a path is actually written: separators, drive
/// colons and control characters become `_`, bidi and zero-width characters
/// go, and leading dots go, so the entry can never be a parent-directory or
/// absolute path, or a hidden file. It is also cut to
/// [`MAX_NAME_BYTES`] bytes, keeping its extension, so it extracts on a file
/// system with a 255-byte name limit.
fn safe_entry_name(name: &str) -> String {
    let flat: String = name
        .chars()
        .filter(|c| !is_invisible_format(*c))
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let (stem, ext) = split_ext(flat.trim_start_matches('.'));
    match fit_file_name(stem, ext).trim_start_matches('.') {
        "" => "finding".to_string(),
        n => n.to_string(),
    }
}

/// Longest text after a name's last `.` that counts as its extension.
const MAX_EXT_BYTES: usize = 16;

/// `name` split into its stem and extension (without the dot). A dot that
/// starts the name, or one followed by more than [`MAX_EXT_BYTES`] bytes, is
/// part of the stem: only a real extension is kept whole when a name is cut.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i - 1 <= MAX_EXT_BYTES => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
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
    let (stem, ext) = split_ext(&name);
    let mut n = 2;
    loop {
        // The suffix must not push the name past the byte cap: the stem gives
        // way (at a char boundary) instead.
        let suffix = if ext.is_empty() {
            format!(" ({n})")
        } else {
            format!(" ({n}).{ext}")
        };
        let stem = truncate_bytes(stem, MAX_NAME_BYTES.saturating_sub(suffix.len())).trim_end();
        let candidate = format!("{stem}{suffix}");
        if used.insert(candidate.to_lowercase()) {
            return candidate;
        }
        n += 1;
    }
}

/// One file per finding (named by [`filename`]) plus `index.md`, the Markdown
/// project index. The index stays Markdown whatever the finding format is.
/// `numbers` and `blobs` are as for [`render_project`]. Every entry is dated
/// `meta.generated_at` (see [`zip_time`]).
pub fn render_split_zip(
    meta: &ReportMeta,
    findings: &[ExportFinding],
    numbers: &HashMap<String, String>,
    fmt: Format,
    blobs: Blobs<'_>,
) -> Result<Vec<u8>, ExportError> {
    ensure_supported(fmt)?;
    build_zip(meta, findings, numbers, fmt, |f, n, fmt| {
        render_finding(f, n, fmt, blobs)
    })
}

/// A zip entry timestamp for `t`. Zip stores a local-time date with no zone
/// from 1980 to 2107 (and seconds rounded down to even); `t`'s UTC fields are
/// used as they are, and a time outside the range is clamped to its nearest
/// end.
fn zip_time(t: DateTime<Utc>) -> zip::DateTime {
    let fields = match u16::try_from(t.year()) {
        Ok(y) if (1980..=2107).contains(&y) => {
            // chrono's month/day/hour/minute/second always fit a u8.
            let small = |v: u32| u8::try_from(v).unwrap_or(0);
            (
                y,
                small(t.month()),
                small(t.day()),
                small(t.hour()),
                small(t.minute()),
                small(t.second()),
            )
        }
        _ if t.year() < 1980 => (1980, 1, 1, 0, 0, 0),
        _ => (2107, 12, 31, 23, 59, 58),
    };
    let (y, mo, d, h, mi, s) = fields;
    zip::DateTime::from_date_and_time(y, mo, d, h, mi, s).unwrap_or_default()
}

/// [`render_split_zip`] with the per-finding renderer injected, so the
/// "which finding failed" reporting can be tested without a compile failure.
fn build_zip(
    meta: &ReportMeta,
    findings: &[ExportFinding],
    numbers: &HashMap<String, String>,
    fmt: Format,
    render_one: impl Fn(
        &ExportFinding,
        &HashMap<String, String>,
        Format,
    ) -> Result<Vec<u8>, ExportError>,
) -> Result<Vec<u8>, ExportError> {
    let numbers = crate::blocks::with_own_numbers(numbers, findings);
    let zip_err = |e: zip::result::ZipError| ExportError::Zip(e.to_string());
    let io_err = |e: std::io::Error| ExportError::Zip(e.to_string());
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts =
        zip::write::SimpleFileOptions::default().last_modified_time(zip_time(meta.generated_at));
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
                        codename: None,
                        agent: None,
                        provider: None,
                    },
                    declared_at: ts(),
                    profile: FindingProfile::Summary,
                    report: None,
                    tags: Vec::new(),
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
    fn entry_names_lose_zero_width_characters_and_fit_the_byte_cap() {
        assert_eq!(
            safe_entry_name("SEC-001 - a\u{200b}b\u{feff}c\u{2060}.md"),
            "SEC-001 - abc.md"
        );
        assert_eq!(safe_entry_name("\u{200d}.hidden"), "hidden");
        let long = format!("{}.pdf", "ß".repeat(300)); // 600 bytes of stem
        let got = safe_entry_name(&long);
        assert!(got.len() <= MAX_NAME_BYTES, "{}", got.len());
        assert!(got.ends_with(".pdf"), "{got}");
        assert!(got.trim_end_matches(".pdf").chars().all(|c| c == 'ß'));
        let no_ext = "x".repeat(400);
        assert_eq!(safe_entry_name(&no_ext).len(), MAX_NAME_BYTES);
        // An "extension" longer than any real one is cut like the rest.
        let dotted = format!("SEC.{}", "y".repeat(400));
        assert_eq!(safe_entry_name(&dotted).len(), MAX_NAME_BYTES);
    }

    #[test]
    fn a_suffixed_entry_name_stays_within_the_byte_cap() {
        let mut used = HashSet::new();
        let name = format!("{}.md", "ø".repeat(98)); // 196 + 3 = 199 bytes
        assert_eq!(unique_entry_name(name.clone(), &mut used), name);
        let second = unique_entry_name(name, &mut used);
        assert!(second.len() <= MAX_NAME_BYTES, "{}", second.len());
        assert!(second.ends_with(" (2).md"), "{second}");
    }

    #[test]
    fn zip_times_are_the_generated_time_clamped_to_the_zip_range() {
        let at = |s: &str| {
            let t = zip_time(s.parse().unwrap());
            (
                t.year(),
                t.month(),
                t.day(),
                t.hour(),
                t.minute(),
                t.second(),
            )
        };
        assert_eq!(at("2026-09-29T12:34:57Z"), (2026, 9, 29, 12, 34, 56));
        assert_eq!(at("1970-01-01T00:00:00Z"), (1980, 1, 1, 0, 0, 0));
        assert_eq!(at("2200-06-01T00:00:00Z"), (2107, 12, 31, 23, 59, 58));
        assert_eq!(at("1980-01-01T00:00:00Z"), (1980, 1, 1, 0, 0, 0));
        assert_eq!(at("2107-12-31T23:59:59Z"), (2107, 12, 31, 23, 59, 58));
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
        let err = build_zip(&meta(), &all, &HashMap::new(), Format::Pdf, |f, _, _| {
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
        fn fake(doc: TypstDoc) -> Result<Vec<u8>, ExportError> {
            let markup = doc.markup;
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
            let err = project_pdf(&meta(), &all, &HashMap::new(), Blobs::NONE, fake)
                .unwrap_err()
                .to_string();
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
            let err = project_pdf(&meta(), &all, &HashMap::new(), Blobs::NONE, fake)
                .unwrap_err()
                .to_string();
            assert_eq!(
                err,
                "PDF rendering failed: finding SEC-002 could not be rendered: bad B"
            );
        }

        #[test]
        fn a_failure_no_single_finding_reproduces_keeps_the_original_error() {
            // Fails only on the combined document (it has the index heading).
            let all = vec![finding(1, "fine"), finding(2, "fine")];
            let err = project_pdf(&meta(), &all, &HashMap::new(), Blobs::NONE, |d| {
                if d.markup.contains("Index") {
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
            let out = project_pdf(&meta(), &all, &HashMap::new(), Blobs::NONE, |m| {
                calls.set(calls.get() + 1);
                fake(m)
            })
            .unwrap();
            assert_eq!(out, b"%PDF-fake");
            assert_eq!(calls.get(), 1);
        }
    }
}
