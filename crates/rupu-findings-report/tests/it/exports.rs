//! Export API behaviour that does not depend on the `pdf` feature: formats,
//! Markdown/HTML output, the split zip and its entry names. (PDF itself is in
//! `tests/it/pdf.rs`.)

use crate::common::*;
use rupu_coverage::report::{CrossRef, OrSentinel, Relation};
use rupu_coverage::Severity;
use rupu_findings_report::model::{ExportFinding, ReportMeta};
use rupu_findings_report::number::{filename, number_map, MAX_NAME_BYTES};
use rupu_findings_report::Blobs;
use rupu_findings_report::{render_finding, render_project, render_split_zip, Format};
use std::collections::HashMap;
use std::io::Read;

fn meta() -> ReportMeta {
    ReportMeta {
        title: "Notebin findings".into(),
        generated_at: ts("2026-09-29T12:00:00Z"),
        scope: "Project notebin".into(),
    }
}

fn two() -> Vec<ExportFinding> {
    numbered(vec![
        input(
            "notebin",
            Some("audit"),
            full_record("fnd_full", Severity::Critical, full_report()),
        ),
        input("notebin", None, summary_record("fnd_sum", Severity::High)),
    ])
}

fn one_numbers(f: &ExportFinding) -> HashMap<String, String> {
    number_map(std::slice::from_ref(f))
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).expect("utf-8")
}

fn unzip(bytes: &[u8]) -> Vec<(String, String)> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("a zip");
    (0..z.len())
        .map(|i| {
            let mut f = z.by_index(i).unwrap();
            let mut body = String::new();
            f.read_to_string(&mut body).unwrap();
            (f.name().to_string(), body)
        })
        .collect()
}

#[test]
fn format_names_and_content_types() {
    assert_eq!(Format::Pdf.content_type(), "application/pdf");
    assert_eq!(
        Format::Markdown.content_type(),
        "text/markdown; charset=utf-8"
    );
    assert_eq!(Format::Html.content_type(), "text/html; charset=utf-8");
    assert_eq!(Format::Markdown.ext(), "md");
    assert_eq!(Format::Html.ext(), "html");
    assert_eq!(Format::Pdf.ext(), "pdf");
    assert_eq!(Format::parse("pdf"), Some(Format::Pdf));
    assert_eq!(Format::parse("markdown"), Some(Format::Markdown));
    assert_eq!(Format::parse("md"), Some(Format::Markdown));
    assert_eq!(Format::parse("html"), Some(Format::Html));
    assert_eq!(Format::parse("docx"), None);
}

#[test]
fn markdown_finding_and_project_are_markdown() {
    let f = full_finding();
    let md = text(render_finding(&f, &one_numbers(&f), Format::Markdown, Blobs::NONE).unwrap());
    assert!(md.starts_with("Filename: SEC-001 - "), "{md}");
    assert!(md.contains("\n## Description\n"), "{md}");
    assert!(!md.contains("<html"), "{md}");

    let all = two();
    let md = text(
        render_project(
            &meta(),
            &all,
            &HashMap::new(),
            Format::Markdown,
            Blobs::NONE,
        )
        .unwrap(),
    );
    assert!(md.starts_with("# Notebin findings\n"), "{md}");
    assert!(md.contains("## Index"), "{md}");
    assert!(md.contains("Lookup ignores the owner"), "{md}");
}

#[test]
fn html_finding_and_project_are_html_documents() {
    let f = full_finding();
    let html = text(render_finding(&f, &one_numbers(&f), Format::Html, Blobs::NONE).unwrap());
    assert!(html.starts_with("<!doctype html>"), "{html}");
    assert!(html.contains("<title>SEC-001 - Notes API"), "{html}");

    let html =
        text(render_project(&meta(), &two(), &HashMap::new(), Format::Html, Blobs::NONE).unwrap());
    assert!(html.starts_with("<!doctype html>"), "{html}");
    assert!(html.contains("<title>Notebin findings</title>"), "{html}");
    assert!(html.contains("Lookup ignores the owner"), "{html}");
}

#[test]
fn markdown_split_zip_has_an_index_and_one_file_per_finding() {
    let all = two();
    let zip = render_split_zip(
        &meta(),
        &all,
        &HashMap::new(),
        Format::Markdown,
        Blobs::NONE,
    )
    .unwrap();
    let entries = unzip(&zip);
    let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    let want: Vec<String> = ["index.md".to_string()]
        .into_iter()
        .chain(all.iter().map(|f| filename(f, "md")))
        .collect();
    assert_eq!(names, want.iter().map(String::as_str).collect::<Vec<_>>());
    assert!(names[1].starts_with("SEC-001 - "), "{names:?}");
    assert!(names[2].starts_with("SEC-002 - "), "{names:?}");

    let index = &entries[0].1;
    assert!(index.contains("Notebin findings"), "{index}");
    assert!(
        index.contains("SEC-001") && index.contains("SEC-002"),
        "{index}"
    );
    // The index is the title + table only, not every finding's body.
    assert!(!index.contains("Replication Steps"), "{index}");
    assert!(!index.contains("Provenance"), "{index}");
    assert!(
        entries[1].1.contains("Replication Steps"),
        "{}",
        entries[1].1
    );
}

#[test]
fn html_split_zip_holds_html_documents_and_a_markdown_index() {
    let zip =
        render_split_zip(&meta(), &two(), &HashMap::new(), Format::Html, Blobs::NONE).unwrap();
    let entries = unzip(&zip);
    assert_eq!(entries[0].0, "index.md");
    for (name, body) in &entries[1..] {
        assert!(name.ends_with(".html"), "{name}");
        assert!(body.starts_with("<!doctype html>"), "{name}");
    }
}

/// A finding number is caller-supplied text and ends up in file names.
#[test]
fn hostile_numbers_cannot_escape_the_archive() {
    for number in [
        "../../evil",
        "a/b\\c",
        "/etc/passwd",
        "C:\\Windows\\x",
        "..",
        "....//x",
        ".hidden",
        "",
        "SEC\n001\u{0}\u{202e}",
    ] {
        let mut all = two();
        all[0].number = number.to_string();
        let zip = render_split_zip(
            &meta(),
            &all,
            &HashMap::new(),
            Format::Markdown,
            Blobs::NONE,
        )
        .unwrap();
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
        assert_eq!(z.len(), 3, "number {number:?}");
        for i in 0..z.len() {
            let f = z.by_index(i).unwrap();
            let name = f.name().to_string();
            assert!(
                !name.contains(['/', '\\', ':']) && !name.starts_with('.'),
                "number {number:?} -> entry {name:?}"
            );
            let safe = f
                .enclosed_name()
                .unwrap_or_else(|| panic!("number {number:?}: {name:?} is not an enclosed path"));
            assert_eq!(safe.components().count(), 1, "{name:?}");
            assert!(
                !safe
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_))),
                "{name:?}"
            );
        }
    }
}

#[test]
fn a_hostile_number_reads_as_a_clean_prefix_in_the_entry_name() {
    let mut all = two();
    all[0].number = "../../evil".to_string();
    let zip = render_split_zip(
        &meta(),
        &all,
        &HashMap::new(),
        Format::Markdown,
        Blobs::NONE,
    )
    .unwrap();
    let names: Vec<String> = unzip(&zip).into_iter().map(|(n, _)| n).collect();
    assert!(names[1].starts_with("evil - "), "{names:?}");
    assert_eq!(names[1], filename(&all[0], "md"));
}

/// Findings are numbered per project, so an all-projects export repeats
/// numbers; two of them with one title would otherwise be two zip entries of
/// the same name, which the format rejects.
#[test]
fn identical_numbers_and_titles_across_projects_get_distinct_entries() {
    let a = summary_record("fnd_a", Severity::High);
    let b = summary_record("fnd_b", Severity::High);
    let all = numbered(vec![input("alpha", None, a), input("beta", None, b)]);
    assert_eq!(all[0].number, all[1].number);
    assert_eq!(filename(&all[0], "md"), filename(&all[1], "md"));
    let zip = render_split_zip(
        &meta(),
        &all,
        &HashMap::new(),
        Format::Markdown,
        Blobs::NONE,
    )
    .unwrap();
    let names: Vec<String> = unzip(&zip).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names.len(), 3);
    assert_eq!(names[1], filename(&all[0], "md"));
    assert!(names[2].ends_with(" (2).md"), "{names:?}");
    assert_ne!(names[1], names[2]);
}

#[test]
fn split_zip_entries_are_dated_when_the_report_was_generated() {
    let zip = render_split_zip(
        &meta(),
        &two(),
        &HashMap::new(),
        Format::Markdown,
        Blobs::NONE,
    )
    .unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
    assert_eq!(z.len(), 3);
    for i in 0..z.len() {
        let f = z.by_index(i).unwrap();
        let t = f.last_modified().expect("a modification time");
        // meta().generated_at is 2026-09-29T12:00:00Z.
        assert_eq!(
            (t.year(), t.month(), t.day(), t.hour(), t.minute()),
            (2026, 9, 29, 12, 0),
            "{}",
            f.name()
        );
    }
}

#[test]
fn long_multibyte_titles_give_entry_names_within_the_byte_cap() {
    let mut a = summary_record("fnd_a", Severity::High);
    a.summary = "記録".repeat(120);
    let mut b = summary_record("fnd_b", Severity::High);
    b.summary = "記録".repeat(120);
    // Two projects: same number, same (cut) title, so the second entry also
    // needs a de-duplication suffix within the cap.
    let all = numbered(vec![input("alpha", None, a), input("beta", None, b)]);
    let zip = render_split_zip(&meta(), &all, &HashMap::new(), Format::Html, Blobs::NONE).unwrap();
    let names: Vec<String> = unzip(&zip).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names.len(), 3);
    for name in &names[1..] {
        assert!(name.len() <= MAX_NAME_BYTES, "{} bytes: {name}", name.len());
        assert!(name.ends_with(".html"), "{name}");
        assert!(name.starts_with("SEC-001 - 記録"), "{name}");
    }
    assert!(names[2].ends_with(" (2).html"), "{names:?}");
}

#[test]
fn a_cross_reference_to_a_finding_left_out_keeps_its_project_number() {
    let mut citing = full_report();
    citing.cross_references = OrSentinel::Value(vec![CrossRef {
        finding_id: "fnd_left_out".into(),
        relation: Relation::Sibling,
        note: None,
    }]);
    let all = numbered(vec![
        input(
            "notebin",
            None,
            full_record("fnd_cite", Severity::Critical, citing),
        ),
        input(
            "notebin",
            None,
            full_record("fnd_left_out", Severity::High, full_report()),
        ),
    ]);
    // The whole project's numbers, as the caller has them before selecting.
    let project_numbers = number_map(&all);
    let chosen = &all[..1];

    let md = text(
        render_project(
            &meta(),
            chosen,
            &project_numbers,
            Format::Markdown,
            Blobs::NONE,
        )
        .unwrap(),
    );
    assert!(md.contains("- SEC-002 (sibling)"), "{md}");
    assert!(!md.contains("fnd_left_out"), "{md}");
    let html =
        text(render_project(&meta(), chosen, &project_numbers, Format::Html, Blobs::NONE).unwrap());
    assert!(html.contains("SEC-002 (sibling)"), "{html}");

    let zip = render_split_zip(
        &meta(),
        chosen,
        &project_numbers,
        Format::Markdown,
        Blobs::NONE,
    )
    .unwrap();
    let entries = unzip(&zip);
    assert!(
        entries[1].1.contains("- SEC-002 (sibling)"),
        "{}",
        entries[1].1
    );

    // Given only the selection's own numbers, the id is all there is to show.
    let md = text(
        render_project(
            &meta(),
            chosen,
            &HashMap::new(),
            Format::Markdown,
            Blobs::NONE,
        )
        .unwrap(),
    );
    assert!(md.contains("- fnd_left_out (sibling)"), "{md}");
}

#[test]
fn an_empty_split_zip_is_just_the_index() {
    let zip =
        render_split_zip(&meta(), &[], &HashMap::new(), Format::Markdown, Blobs::NONE).unwrap();
    let names: Vec<String> = unzip(&zip).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["index.md"]);
}

// ---------------------------------------------------- without the pdf feature

/// Built with `--no-default-features`, PDF is refused with a dedicated,
/// clearly worded error by every entry point, and everything else still works.
#[cfg(not(feature = "pdf"))]
mod without_pdf {
    use super::*;
    use rupu_findings_report::ExportError;

    fn assert_unavailable<T: std::fmt::Debug>(r: Result<T, ExportError>) {
        let e = r.unwrap_err();
        assert!(matches!(e, ExportError::PdfUnavailable), "{e:?}");
        assert_eq!(
            e.to_string(),
            "this build of rupu was compiled without PDF support"
        );
    }

    #[test]
    fn every_pdf_entry_point_reports_pdf_unavailable() {
        let f = full_finding();
        assert_unavailable(render_finding(
            &f,
            &one_numbers(&f),
            Format::Pdf,
            Blobs::NONE,
        ));
        assert_unavailable(render_project(
            &meta(),
            &two(),
            &HashMap::new(),
            Format::Pdf,
            Blobs::NONE,
        ));
        assert_unavailable(render_split_zip(
            &meta(),
            &two(),
            &HashMap::new(),
            Format::Pdf,
            Blobs::NONE,
        ));
        // Even an empty selection is refused, rather than silently producing
        // an archive with only an index.
        assert_unavailable(render_project(
            &meta(),
            &[],
            &HashMap::new(),
            Format::Pdf,
            Blobs::NONE,
        ));
        assert_unavailable(render_split_zip(
            &meta(),
            &[],
            &HashMap::new(),
            Format::Pdf,
            Blobs::NONE,
        ));
    }

    #[test]
    fn markdown_and_html_still_render() {
        let f = full_finding();
        assert!(render_finding(&f, &one_numbers(&f), Format::Markdown, Blobs::NONE).is_ok());
        assert!(render_finding(&f, &one_numbers(&f), Format::Html, Blobs::NONE).is_ok());
        assert!(render_split_zip(
            &meta(),
            &two(),
            &HashMap::new(),
            Format::Markdown,
            Blobs::NONE
        )
        .is_ok());
    }
}
