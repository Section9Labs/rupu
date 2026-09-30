mod common;

use common::*;
use rupu_coverage::report::{CrossRef, OrSentinel, Relation};
use rupu_coverage::Severity;
use rupu_findings_report::blocks::{finding_blocks, project_blocks, Block};
use rupu_findings_report::markdown::render;
use rupu_findings_report::model::ReportMeta;
use rupu_findings_report::number::number_map;
use std::collections::HashMap;

fn full_md() -> String {
    let f = full_finding();
    render(&finding_blocks(&f, &number_map(std::slice::from_ref(&f))))
}

fn h2_lines(md: &str) -> Vec<&str> {
    md.lines().filter(|l| l.starts_with("## ")).collect()
}

#[test]
fn full_finding_leads_with_filename_then_title_and_has_the_key_lines() {
    let md = full_md();
    let mut lines = md.lines();
    assert_eq!(
        lines.next().unwrap(),
        "Filename: SEC-001 - Notes API returns another user's note by id.pdf"
    );
    assert_eq!(lines.next().unwrap(), "");
    assert_eq!(
        lines.next().unwrap(),
        "# Notes API returns another user's note by id"
    );
    for want in [
        "**Identifier:** SEC-001",
        "**Owner:** Unknown",
        "**Existing Ticket References:** None Provided",
        "**Impact:** High",
        "**Likelihood:** High",
        "**Risk Rating:** Critical",
        "## Root Cause",
        "## Call Chain / Attack Flow",
        "```diff",
        "## Replication Steps",
        "Step 1: Sign up as user A",
        "**CVSS v3 Base Score:** Unknown",
        "**Risk Factor:** High",
    ] {
        assert!(md.contains(want), "missing {want:?} in:\n{md}");
    }
    assert!(md.ends_with('\n') && !md.ends_with("\n\n"));
}

#[test]
fn sections_appear_in_the_standard_order() {
    let md = full_md();
    assert_eq!(
        h2_lines(&md),
        [
            "## Description",
            "## Impact",
            "## Location",
            "## Root Cause",
            "## Call Chain / Attack Flow",
            "## Evidence",
            "## Remediation",
            "## Recommended Patch",
            "## CI/CD Detection",
            "## Regression Test",
            "## Cross-References",
            "## References",
            "## Replication Steps",
            "## Provenance",
        ]
    );
    // The CVSS / Risk Factor fields sit between References and Replication.
    let refs = md.find("## References").unwrap();
    let cvss = md.find("**CVSS v3 Base Score:**").unwrap();
    let steps = md.find("## Replication Steps").unwrap();
    assert!(refs < cvss && cvss < steps);
}

#[test]
fn artifacts_section_sits_between_replication_and_provenance() {
    let mut report = full_report();
    report.artifacts = vec![rupu_coverage::report::ArtifactRef {
        path: "poc/exploit.py".into(),
        sha256: "0123456789abcdef0123456789abcdef".into(),
        size: 2048,
        kind: None,
        stored: Some(rupu_coverage::report::ArtifactStorage::Copied),
        host: None,
    }];
    let f = numbered(vec![input(
        "notebin",
        None,
        full_record("fnd_a", Severity::High, report),
    )])
    .remove(0);
    let md = render(&finding_blocks(&f, &number_map(std::slice::from_ref(&f))));
    let h = h2_lines(&md);
    assert_eq!(
        &h[h.len() - 3..],
        ["## Replication Steps", "## Artifacts", "## Provenance"]
    );
    assert!(md.contains("| Path | SHA-256 (first 12 chars) | Size | Stored |"));
    assert!(md.contains("| poc/exploit.py | 0123456789ab | 2048 B | Copied |"));
}

#[test]
fn provenance_lists_the_record_and_dash_for_a_missing_workflow() {
    let md = full_md();
    let prov = &md[md.find("## Provenance").unwrap()..];
    for want in [
        "**Finding ID:** fnd_full",
        "**Project:** notebin",
        "**Workflow:** audit",
        "**Run:** run_01",
        "**Model:** claude-x",
        "**Declared:** 2026-03-01T10:00:00Z",
    ] {
        assert!(prov.contains(want), "missing {want:?} in:\n{prov}");
    }
    let f = numbered(vec![input(
        "notebin",
        None,
        full_record("fnd_full", Severity::High, full_report()),
    )])
    .remove(0);
    let md = render(&finding_blocks(&f, &HashMap::new()));
    assert!(md.contains("**Workflow:** —"));
}

#[test]
fn a_cross_reference_renders_the_display_number_not_the_finding_id() {
    let mut report = full_report();
    report.cross_references = OrSentinel::Value(vec![
        CrossRef {
            finding_id: "fnd_other".into(),
            relation: Relation::Duplicate,
            note: Some("same handler".into()),
        },
        CrossRef {
            finding_id: "fnd_elsewhere".into(),
            relation: Relation::Sibling,
            note: None,
        },
    ]);
    let f = numbered(vec![input(
        "notebin",
        None,
        full_record("fnd_self", Severity::High, report),
    )])
    .remove(0);
    let numbers: HashMap<String, String> =
        HashMap::from([("fnd_other".to_string(), "SEC-002".to_string())]);
    let md = render(&finding_blocks(&f, &numbers));
    assert!(md.contains("- SEC-002 (duplicate) — same handler"), "{md}");
    assert!(!md.contains("fnd_other"), "{md}");
    // Not in the set being exported: the id is all there is to show.
    assert!(md.contains("- fnd_elsewhere (sibling)"), "{md}");
}

#[test]
fn a_summary_finding_renders_the_note_and_no_full_report_sections() {
    let f = numbered(vec![input(
        "notebin",
        Some("audit"),
        summary_record("fnd_sum", Severity::High),
    )])
    .remove(0);
    let md = render(&finding_blocks(&f, &HashMap::new()));
    assert!(md.starts_with(
        "Filename: SEC-001 - Lookup ignores the owner.pdf\n\n# Lookup ignores the owner\n"
    ));
    assert!(md.contains("_Summary finding — no full report was recorded._"));
    assert!(md.contains("**Severity:** High"));
    assert!(md.contains("**Location:** src/store/notes.rs:88-97"));
    assert!(md.contains("**Concern:** authz-idor"));
    assert!(md.contains("## Rationale"));
    assert!(md.contains("store.find_by_id(id)"));
    assert!(md.contains("- CWE-639"));
    assert!(!md.contains("## Root Cause"));
    assert!(!md.contains("## Replication Steps"));
    assert_eq!(
        h2_lines(&md),
        ["## Rationale", "## References", "## Provenance"]
    );
}

#[test]
fn a_summary_finding_without_a_locator_or_concern_shows_dashes() {
    let mut r = summary_record("fnd_sum", Severity::Low);
    r.file_path = None;
    r.line_range = None;
    r.concern_id = None;
    r.evidence.code_excerpt = None;
    r.evidence.references.clear();
    let f = numbered(vec![input("notebin", None, r)]).remove(0);
    let md = render(&finding_blocks(&f, &HashMap::new()));
    assert!(md.contains("**Location:** —"));
    assert!(md.contains("**Concern:** —"));
    assert_eq!(h2_lines(&md), ["## Rationale", "## Provenance"]);
}

#[test]
fn a_project_renders_an_index_and_a_rule_between_findings() {
    let all = numbered(vec![
        input(
            "notebin",
            Some("audit"),
            full_record("fnd_full", Severity::Critical, full_report()),
        ),
        input(
            "notebin",
            Some("audit"),
            summary_record("fnd_sum", Severity::High),
        ),
    ]);
    let meta = ReportMeta {
        title: "Notebin findings".into(),
        generated_at: ts("2026-09-29T12:00:00Z"),
        scope: "Project notebin · severity ≥ high".into(),
    };
    let md = render(&project_blocks(&meta, &all));
    assert!(md.starts_with("# Notebin findings\n"), "{md}");
    for want in [
        "**Generated:** 2026-09-29T12:00:00Z",
        "**Scope:** Project notebin · severity ≥ high",
        "**Findings:** 2 — critical 1, high 1",
        "## Index",
        "| Number | Severity | Title | Project | Finding ID | Profile |",
        "| --- | --- | --- | --- | --- | --- |",
        "| SEC-001 | Critical | Notes API returns another user's note by id | notebin | fnd_full | full |",
        "| SEC-002 | High | Lookup ignores the owner | notebin | fnd_sum | summary |",
    ] {
        assert!(md.contains(want), "missing {want:?} in:\n{md}");
    }
    // One rule before each finding; the diff's `--- a/…` line is not a rule.
    assert_eq!(md.lines().filter(|l| *l == "---").count(), 2, "{md}");
    let first = md.find("Filename: SEC-001").unwrap();
    let second = md.find("Filename: SEC-002").unwrap();
    let index = md.find("## Index").unwrap();
    assert!(index < first && first < second);
    assert!(md[..first].trim_end().ends_with("---"));
    assert!(md[first..second].trim_end().ends_with("---"));
}

#[test]
fn an_empty_project_reports_zero_findings() {
    let meta = ReportMeta {
        title: "Nothing".into(),
        generated_at: ts("2026-09-29T12:00:00Z"),
        scope: "all".into(),
    };
    let md = render(&project_blocks(&meta, &[]));
    assert!(md.contains("**Findings:** 0"));
    assert!(!md.contains("---\n"));
}

#[test]
fn code_fences_grow_past_any_backtick_run_in_the_text() {
    let md = render(&[Block::Code {
        lang: Some("md".into()),
        text: "a ``` b\n".into(),
    }]);
    assert_eq!(md, "````md\na ``` b\n````\n");
    let md = render(&[Block::Code {
        lang: None,
        text: "x ```` y".into(),
    }]);
    assert_eq!(md, "`````\nx ```` y\n`````\n");
    let md = render(&[Block::Code {
        lang: None,
        text: "plain".into(),
    }]);
    assert_eq!(md, "```\nplain\n```\n");
}

#[test]
fn values_cannot_break_out_of_their_line_or_table_cell() {
    let md = render(&[
        Block::Title("one\n# two".into()),
        Block::Fields(vec![("Owner".into(), "a\n\n---\nb".into())]),
        Block::Table {
            headers: vec!["A|B".into()],
            rows: vec![vec!["x|y\nz".into()]],
        },
        Block::Note("line\nbreak".into()),
    ]);
    assert!(md.contains("# one # two\n"), "{md}");
    assert!(md.contains("**Owner:** a --- b  \n"), "{md}");
    assert!(md.contains("| A\\|B |\n| --- |\n| x\\|y z |\n"), "{md}");
    assert!(md.contains("_line break_\n"), "{md}");
}

#[test]
fn consecutive_steps_end_in_hard_breaks_so_they_stay_on_separate_lines() {
    let md = render(&[Block::Steps(vec!["first".into(), "second".into()])]);
    assert_eq!(md, "Step 1: first  \nStep 2: second\n");
}
