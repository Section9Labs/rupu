//! `attach_reports`: a report imported onto an existing summary finding.

use rupu_coverage::report::{FindingProfile, FindingReport, FindingWriteOptions};
use rupu_coverage::tools::report_finding::{report_finding, ReportFindingInput};
use rupu_coverage::tools::{attach_reports, AttachItem, AttachOutcome};
use rupu_coverage::{
    read_findings, Attribution, CoveragePaths, FindingEvidence, FindingScope, Severity, Surface,
};

fn report() -> FindingReport {
    serde_json::from_str(include_str!("fixtures/finding_report/valid_full.json")).unwrap()
}

fn attribution() -> Attribution {
    Attribution {
        run_id: "run_old".into(),
        model: "m".into(),
        surface: Surface::Workflow,
    }
}

/// A summary finding written the way pre-full-profile agents wrote them.
fn seed_summary(paths: &CoveragePaths) -> String {
    let input = ReportFindingInput {
        file_path: Some("src/routes/notes.rs".into()),
        line_range: Some([40, 58]),
        target_ref: None,
        scope: FindingScope::Line,
        summary: Some("Note lookup ignores the owner".into()),
        severity: Some(Severity::High),
        concern_id: Some("authz-idor".into()),
        evidence: Some(FindingEvidence {
            code_excerpt: Some("store.find_by_id(id)".into()),
            rationale: "The id is the only key.".into(),
            references: vec![],
        }),
        report: None,
    };
    let opts = FindingWriteOptions::default().with_profile(FindingProfile::Summary);
    report_finding(paths, attribution(), input, &opts)
        .unwrap()
        .id
}

fn setup() -> (tempfile::TempDir, CoveragePaths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = CoveragePaths::new(dir.path(), "tgt1");
    (dir, paths)
}

fn full_opts() -> FindingWriteOptions {
    FindingWriteOptions::default().with_profile(FindingProfile::Full)
}

#[test]
fn attaches_onto_a_summary_finding_and_keeps_everything_else() {
    let (_d, paths) = setup();
    let a = seed_summary(&paths);
    let b = seed_summary(&paths);
    let before = std::fs::read_to_string(&paths.findings).unwrap();

    let batch = attach_reports(
        &paths,
        vec![AttachItem {
            finding_id: b.clone(),
            report: report(),
        }],
        &full_opts(),
        false,
    )
    .unwrap();

    assert!(matches!(
        batch.outcomes.as_slice(),
        [AttachOutcome::Attached]
    ));
    let backup = batch.backup.expect("a backup is kept");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), before);
    let after = std::fs::read_to_string(&paths.findings).unwrap();
    // The untouched line is byte-identical.
    assert_eq!(after.lines().next(), before.lines().next());

    let records = read_findings(&paths).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].id, a);
    assert!(records[0].report.is_none());
    let r = &records[1];
    assert_eq!(r.id, b);
    assert_eq!(r.profile, FindingProfile::Full);
    assert_eq!(r.report.as_ref().unwrap().title, report().title);
    // Derived exactly as `report_finding` derives them for a full finding.
    assert_eq!(r.summary, report().title);
    assert_eq!(r.severity, Severity::Critical);
    assert_eq!(r.evidence.rationale, report().root_cause);
    // Identity and provenance are the original record's.
    assert_eq!(r.declared_by.run_id, "run_old");
    assert_eq!(r.concern_id.as_deref(), Some("authz-idor"));
    assert_eq!(r.file_path.as_deref(), Some("src/routes/notes.rs"));
}

#[test]
fn a_dry_run_validates_and_writes_nothing() {
    let (_d, paths) = setup();
    let id = seed_summary(&paths);
    let before = std::fs::read_to_string(&paths.findings).unwrap();
    let batch = attach_reports(
        &paths,
        vec![AttachItem {
            finding_id: id,
            report: report(),
        }],
        &full_opts(),
        true,
    )
    .unwrap();
    assert!(matches!(
        batch.outcomes.as_slice(),
        [AttachOutcome::Attached]
    ));
    assert!(batch.backup.is_none());
    assert_eq!(std::fs::read_to_string(&paths.findings).unwrap(), before);
    let entries: Vec<_> = std::fs::read_dir(&paths.root)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.contains("pre-import"))
        .collect();
    assert!(entries.is_empty(), "{entries:?}");
}

#[test]
fn outcomes_for_unknown_full_invalid_and_repeated_ids() {
    let (_d, paths) = setup();
    let id = seed_summary(&paths);
    let full = attach_reports(
        &paths,
        vec![AttachItem {
            finding_id: id.clone(),
            report: report(),
        }],
        &full_opts(),
        false,
    )
    .unwrap();
    assert!(matches!(
        full.outcomes.as_slice(),
        [AttachOutcome::Attached]
    ));

    let other = seed_summary(&paths);
    let mut bad = report();
    bad.title = "   ".into();
    let batch = attach_reports(
        &paths,
        vec![
            AttachItem {
                finding_id: "fnd_missing".into(),
                report: report(),
            },
            AttachItem {
                finding_id: id,
                report: report(),
            },
            AttachItem {
                finding_id: other.clone(),
                report: bad,
            },
            AttachItem {
                finding_id: other.clone(),
                report: report(),
            },
            AttachItem {
                finding_id: other,
                report: report(),
            },
        ],
        &full_opts(),
        false,
    )
    .unwrap();
    assert!(matches!(batch.outcomes[0], AttachOutcome::NotFound));
    assert!(matches!(batch.outcomes[1], AttachOutcome::AlreadyHasReport));
    match &batch.outcomes[2] {
        AttachOutcome::Rejected(e) => assert!(e.to_string().contains("report.title"), "{e}"),
        o => panic!("expected Rejected, got {o:?}"),
    }
    // The first valid item for an id attaches; a second one for the same
    // id in the same batch is refused rather than silently overwriting.
    assert!(matches!(batch.outcomes[3], AttachOutcome::Attached));
    assert!(matches!(batch.outcomes[4], AttachOutcome::Duplicate));
}

#[test]
fn a_full_finding_whose_report_no_longer_parses_is_never_rewritten() {
    let (_d, paths) = setup();
    // A full-profile line whose report fails to parse loads with
    // `report: None`. Rewriting it would destroy the stored report.
    let id = seed_summary(&paths);
    let mut v: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&paths.findings).unwrap().trim()).unwrap();
    v["profile"] = serde_json::json!("full");
    v["report"] = serde_json::json!({ "title": "only a title" });
    let line = format!("{}\n", serde_json::to_string(&v).unwrap());
    std::fs::write(&paths.findings, &line).unwrap();
    assert!(
        read_findings(&paths).unwrap()[0].report.is_none(),
        "loads leniently"
    );
    let batch = attach_reports(
        &paths,
        vec![AttachItem {
            finding_id: id,
            report: report(),
        }],
        &full_opts(),
        false,
    )
    .unwrap();
    assert!(matches!(
        batch.outcomes.as_slice(),
        [AttachOutcome::AlreadyHasReport]
    ));
    assert_eq!(std::fs::read_to_string(&paths.findings).unwrap(), line);
}

#[test]
fn unparseable_lines_and_unknown_keys_survive_a_rewrite() {
    let (_d, paths) = setup();
    let id = seed_summary(&paths);
    let mut ledger = std::fs::read_to_string(&paths.findings).unwrap();
    // A future key on the record, and a line nothing can parse.
    ledger = ledger.replacen("{\"id\"", "{\"future_key\":7,\"id\"", 1);
    ledger.push_str("not json at all\n");
    std::fs::write(&paths.findings, &ledger).unwrap();

    attach_reports(
        &paths,
        vec![AttachItem {
            finding_id: id,
            report: report(),
        }],
        &full_opts(),
        false,
    )
    .unwrap();
    let after = std::fs::read_to_string(&paths.findings).unwrap();
    assert!(after.contains("\"future_key\":7"), "{after}");
    assert!(after.ends_with("not json at all\n"), "{after}");
}

#[test]
fn a_new_finding_can_still_be_appended_after_an_attach() {
    let (_d, paths) = setup();
    let id = seed_summary(&paths);
    attach_reports(
        &paths,
        vec![AttachItem {
            finding_id: id,
            report: report(),
        }],
        &full_opts(),
        false,
    )
    .unwrap();
    seed_summary(&paths);
    assert_eq!(read_findings(&paths).unwrap().len(), 2);
    assert!(paths.root.join("findings.jsonl.lock").exists());
}
