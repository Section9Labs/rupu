//! `verify_finding`: a verification verdict merged onto a finding that
//! already carries a report, by a locked rewrite of its ledger line.

use rupu_coverage::report::{
    FindingProfile, FindingReport, FindingWriteOptions, Verification, VerificationStatus,
};
use rupu_coverage::tools::report_finding::{report_finding, ReportFindingInput};
use rupu_coverage::tools::{verify_finding, VerifyError, VerifyInput};
use rupu_coverage::{
    read_findings, Attribution, CoveragePaths, FindingEvidence, FindingScope, Severity, Surface,
};

fn report() -> FindingReport {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap()
}

fn attribution(run_id: &str) -> Attribution {
    Attribution {
        run_id: run_id.into(),
        model: "m".into(),
        surface: Surface::Workflow,
        codename: None,
        agent: None,
        provider: None,
    }
}

fn setup() -> (tempfile::TempDir, CoveragePaths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = CoveragePaths::new(dir.path(), "tgt1");
    (dir, paths)
}

/// A full-report finding filed by `run_id`, written the way an agent writes
/// one.
fn seed_full(paths: &CoveragePaths, run_id: &str) -> String {
    let input = ReportFindingInput {
        file_path: None,
        line_range: None,
        target_ref: None,
        scope: FindingScope::Repo,
        summary: None,
        severity: None,
        concern_id: None,
        evidence: None,
        report: Some(report()),
        asset: None,
    };
    let opts = FindingWriteOptions::default().with_profile(FindingProfile::Full);
    report_finding(paths, attribution(run_id), input, &opts)
        .expect("a valid full report records")
        .id
}

/// A summary finding: it has no report to verify.
fn seed_summary(paths: &CoveragePaths, run_id: &str) -> String {
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
        asset: None,
    };
    let opts = FindingWriteOptions::default().with_profile(FindingProfile::Summary);
    report_finding(paths, attribution(run_id), input, &opts)
        .unwrap()
        .id
}

fn verdict(id: &str, by_run: &str, status: VerificationStatus) -> VerifyInput {
    VerifyInput {
        finding_id: id.into(),
        status,
        by_run: by_run.into(),
        by_agent: Some("verifier".into()),
        notes: Some("reproduced against the handler".into()),
    }
}

/// The ledger's lines, each with its terminator.
fn lines(paths: &CoveragePaths) -> Vec<Vec<u8>> {
    std::fs::read(&paths.findings)
        .unwrap()
        .split_inclusive(|b| *b == b'\n')
        .map(<[u8]>::to_vec)
        .collect()
}

fn names(paths: &CoveragePaths) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(&paths.root)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

/// Nothing a failed or refused verification may leave behind.
fn assert_no_rewrite_litter(paths: &CoveragePaths) {
    let litter: Vec<_> = names(paths)
        .into_iter()
        .filter(|n| n.contains("pre-") || n.contains("-tmp"))
        .collect();
    assert!(litter.is_empty(), "{litter:?}");
}

#[test]
fn verifies_a_report_finding_and_leaves_every_other_line_byte_identical() {
    let (_d, paths) = setup();
    let before_summary = seed_summary(&paths, "run_X");
    let target = seed_full(&paths, "run_A");
    let after_full = seed_full(&paths, "run_Z");
    let before = lines(&paths);
    assert_eq!(before.len(), 3);

    verify_finding(
        &paths,
        &verdict(&target, "run_B", VerificationStatus::Confirmed),
    )
    .unwrap();

    let after = lines(&paths);
    assert_eq!(after.len(), 3);
    // Only the target's line differs, and the rest are the same bytes.
    assert_eq!(after[0], before[0]);
    assert_eq!(after[2], before[2]);
    assert_ne!(after[1], before[1]);
    assert!(after[1].ends_with(b"\n"));

    let records = read_findings(&paths).unwrap();
    assert_eq!(records[0].id, before_summary);
    assert_eq!(records[2].id, after_full);
    assert!(records[2].report.as_ref().unwrap().verification.is_none());
    let r = &records[1];
    assert_eq!(r.id, target);
    assert_eq!(
        r.report.as_ref().unwrap().verification,
        Some(Verification {
            status: VerificationStatus::Confirmed,
            by_run: Some("run_B".into()),
            by_agent: Some("verifier".into()),
            notes: Some("reproduced against the handler".into()),
        })
    );
    // Identity and provenance are the filing run's, not the verifier's.
    assert_eq!(r.declared_by.run_id, "run_A");
}

#[test]
fn only_the_verification_key_of_the_report_changes() {
    let (_d, paths) = setup();
    let target = seed_full(&paths, "run_A");
    let before: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&paths.findings).unwrap()).unwrap();

    verify_finding(
        &paths,
        &verdict(&target, "run_B", VerificationStatus::Disputed),
    )
    .unwrap();

    let mut after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&paths.findings).unwrap()).unwrap();
    assert_eq!(after["report"]["verification"]["status"], "disputed");
    after["report"]
        .as_object_mut()
        .unwrap()
        .remove("verification");
    assert_eq!(after, before);
}

#[test]
fn a_verification_without_agent_or_notes_writes_neither_key() {
    let (_d, paths) = setup();
    let target = seed_full(&paths, "run_A");
    verify_finding(
        &paths,
        &VerifyInput {
            finding_id: target,
            status: VerificationStatus::Inconclusive,
            by_run: "run_B".into(),
            by_agent: None,
            notes: None,
        },
    )
    .unwrap();
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&paths.findings).unwrap()).unwrap();
    assert_eq!(
        v["report"]["verification"],
        serde_json::json!({"status": "inconclusive", "by_run": "run_B"})
    );
}

#[test]
fn a_later_verdict_replaces_the_earlier_one_whole() {
    let (_d, paths) = setup();
    let target = seed_full(&paths, "run_A");
    verify_finding(
        &paths,
        &verdict(&target, "run_B", VerificationStatus::Confirmed),
    )
    .unwrap();
    verify_finding(
        &paths,
        &VerifyInput {
            finding_id: target.clone(),
            status: VerificationStatus::Disputed,
            by_run: "run_C".into(),
            by_agent: None,
            notes: None,
        },
    )
    .unwrap();
    let records = read_findings(&paths).unwrap();
    // Nothing of the first verdict (its agent, its notes) lingers.
    assert_eq!(
        records[0].report.as_ref().unwrap().verification,
        Some(Verification {
            status: VerificationStatus::Disputed,
            by_run: Some("run_C".into()),
            by_agent: None,
            notes: None,
        })
    );
}

#[test]
fn self_verification_is_refused_and_the_ledger_is_untouched() {
    let (_d, paths) = setup();
    let target = seed_full(&paths, "run_A");
    let before = std::fs::read(&paths.findings).unwrap();

    let err = verify_finding(
        &paths,
        &verdict(&target, "run_A", VerificationStatus::Confirmed),
    )
    .unwrap_err();
    assert!(matches!(err, VerifyError::SelfVerification), "{err:?}");
    assert_eq!(std::fs::read(&paths.findings).unwrap(), before);
    assert_no_rewrite_litter(&paths);
}

#[test]
fn an_unknown_id_is_not_found_and_the_ledger_is_untouched() {
    let (_d, paths) = setup();
    seed_full(&paths, "run_A");
    let before = std::fs::read(&paths.findings).unwrap();

    let err = verify_finding(
        &paths,
        &verdict("find_nope", "run_B", VerificationStatus::Confirmed),
    )
    .unwrap_err();
    assert!(matches!(err, VerifyError::NotFound), "{err:?}");
    assert_eq!(std::fs::read(&paths.findings).unwrap(), before);
    assert_no_rewrite_litter(&paths);
}

#[test]
fn a_missing_ledger_is_not_found_and_none_is_created() {
    let (_d, paths) = setup();
    let err = verify_finding(
        &paths,
        &verdict("find_nope", "run_B", VerificationStatus::Confirmed),
    )
    .unwrap_err();
    assert!(matches!(err, VerifyError::NotFound), "{err:?}");
    assert!(!paths.findings.exists());
}

#[test]
fn a_finding_without_a_report_cannot_be_verified() {
    let (_d, paths) = setup();
    let summary = seed_summary(&paths, "run_A");
    let before = std::fs::read(&paths.findings).unwrap();

    let err = verify_finding(
        &paths,
        &verdict(&summary, "run_B", VerificationStatus::Confirmed),
    )
    .unwrap_err();
    assert!(matches!(err, VerifyError::NoReport), "{err:?}");
    assert_eq!(std::fs::read(&paths.findings).unwrap(), before);
    assert_no_rewrite_litter(&paths);
}

#[test]
fn the_unverified_status_is_not_a_verdict() {
    let (_d, paths) = setup();
    let target = seed_full(&paths, "run_A");
    let before = std::fs::read(&paths.findings).unwrap();

    let err = verify_finding(
        &paths,
        &verdict(&target, "run_B", VerificationStatus::Unverified),
    )
    .unwrap_err();
    assert!(matches!(err, VerifyError::BadStatus), "{err:?}");
    assert_eq!(std::fs::read(&paths.findings).unwrap(), before);
    assert_no_rewrite_litter(&paths);
}

#[test]
fn a_blank_verifier_run_id_is_refused_and_the_ledger_is_untouched() {
    let (_d, paths) = setup();
    let target = seed_full(&paths, "run_A");
    let before = std::fs::read(&paths.findings).unwrap();

    // An empty `by_run` would pass the self-verification guard (Some("") !=
    // "run_A") and the evaluator's independence check — a silent hole. Refuse
    // it at the chokepoint, leaving the ledger untouched.
    for blank in ["", "   "] {
        let err = verify_finding(
            &paths,
            &verdict(&target, blank, VerificationStatus::Confirmed),
        )
        .unwrap_err();
        assert!(matches!(err, VerifyError::MissingVerifier), "{err:?}");
    }
    assert_eq!(std::fs::read(&paths.findings).unwrap(), before);
    assert_no_rewrite_litter(&paths);
}

#[test]
fn an_id_on_two_ledger_lines_is_left_alone() {
    let (_d, paths) = setup();
    let target = seed_full(&paths, "run_A");
    let line = std::fs::read_to_string(&paths.findings).unwrap();
    let doubled = format!("{line}{line}");
    std::fs::write(&paths.findings, &doubled).unwrap();

    let err = verify_finding(
        &paths,
        &verdict(&target, "run_B", VerificationStatus::Confirmed),
    )
    .unwrap_err();
    assert!(matches!(err, VerifyError::Ambiguous), "{err:?}");
    assert_eq!(std::fs::read_to_string(&paths.findings).unwrap(), doubled);
    assert_no_rewrite_litter(&paths);
}

#[test]
fn unreadable_lines_unknown_keys_and_line_endings_survive_a_rewrite() {
    let (_d, paths) = setup();
    let first = seed_full(&paths, "run_A");
    let second = seed_full(&paths, "run_Z");
    let ledger = std::fs::read_to_string(&paths.findings).unwrap();
    let mut it = ledger.lines();
    let (l1, l2) = (it.next().unwrap(), it.next().unwrap());
    // Line 1: a CRLF terminator and a key this build does not know. Then a
    // line that is not JSON, a line that is not UTF-8, the second finding
    // with no final newline.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(
        l1.replacen("{\"id\"", "{\"future_key\":7,\"id\"", 1)
            .as_bytes(),
    );
    bytes.extend_from_slice(b"\r\n");
    bytes.extend_from_slice(b"not json at all\n");
    bytes.extend_from_slice(b"\xff\xfe not utf-8\n");
    bytes.extend_from_slice(l2.as_bytes());
    std::fs::write(&paths.findings, &bytes).unwrap();
    let before = lines(&paths);

    verify_finding(
        &paths,
        &verdict(&first, "run_B", VerificationStatus::Confirmed),
    )
    .unwrap();
    let after = lines(&paths);
    assert_eq!(after.len(), 4);
    // The rewritten line keeps its own terminator and its unknown key.
    assert!(after[0].ends_with(b"\r\n"), "{:?}", after[0]);
    let text = String::from_utf8(after[0].clone()).unwrap();
    assert!(text.contains("\"future_key\":7"), "{text}");
    assert!(text.contains("\"verification\""), "{text}");
    // Every other line is the same bytes, including the unterminated last.
    assert_eq!(&after[1..], &before[1..]);
    assert!(!after[3].ends_with(b"\n"));

    // The unterminated last line can be the one rewritten, too.
    verify_finding(
        &paths,
        &verdict(&second, "run_B", VerificationStatus::Disputed),
    )
    .unwrap();
    let last = lines(&paths);
    assert!(!last[3].ends_with(b"\n"));
    assert!(String::from_utf8(last[3].clone())
        .unwrap()
        .contains("\"disputed\""));
    assert_eq!(&last[1..3], &before[1..3]);
}

#[test]
fn a_report_this_build_cannot_parse_still_takes_the_verdict() {
    let (_d, paths) = setup();
    let target = seed_full(&paths, "run_A");
    // A newer build's report carries a key this one does not know: the
    // record loads without a report, but the line has one.
    let ledger = std::fs::read_to_string(&paths.findings).unwrap();
    let ledger = ledger.replacen("\"report\":{", "\"report\":{\"newer_key\":1,", 1);
    std::fs::write(&paths.findings, &ledger).unwrap();
    assert!(read_findings(&paths).unwrap()[0].report.is_none());

    verify_finding(
        &paths,
        &verdict(&target, "run_B", VerificationStatus::Confirmed),
    )
    .unwrap();
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&paths.findings).unwrap()).unwrap();
    assert_eq!(v["report"]["newer_key"], 1);
    assert_eq!(v["report"]["verification"]["status"], "confirmed");
}

#[test]
fn a_new_finding_can_still_be_appended_after_a_verification() {
    let (_d, paths) = setup();
    let target = seed_full(&paths, "run_A");
    verify_finding(
        &paths,
        &verdict(&target, "run_B", VerificationStatus::Confirmed),
    )
    .unwrap();
    seed_full(&paths, "run_A");
    let records = read_findings(&paths).unwrap();
    assert_eq!(records.len(), 2);
    assert!(records[0].report.as_ref().unwrap().verification.is_some());
    assert!(paths.root.join("findings.jsonl.lock").exists());
}

#[test]
fn the_ledger_before_each_verification_is_kept_as_one_rolling_backup() {
    let (_d, paths) = setup();
    let target = seed_full(&paths, "run_A");
    let backup = paths.root.join("findings.jsonl.pre-verify");
    let original = std::fs::read(&paths.findings).unwrap();

    verify_finding(
        &paths,
        &verdict(&target, "run_B", VerificationStatus::Confirmed),
    )
    .unwrap();
    let after_first = std::fs::read(&paths.findings).unwrap();
    assert_eq!(std::fs::read(&backup).unwrap(), original);

    verify_finding(
        &paths,
        &verdict(&target, "run_C", VerificationStatus::Disputed),
    )
    .unwrap();
    // One backup, always the ledger as it stood before the latest rewrite;
    // a verification leaves no import backup, and no staging file.
    assert_eq!(std::fs::read(&backup).unwrap(), after_first);
    let litter: Vec<_> = names(&paths)
        .into_iter()
        .filter(|n| n.contains("pre-import") || n.contains("-tmp"))
        .collect();
    assert!(litter.is_empty(), "{litter:?}");
}

#[cfg(unix)]
#[test]
fn a_symlinked_ledger_is_never_replaced() {
    let (_d, paths) = setup();
    let target = seed_full(&paths, "run_A");
    let real = paths.root.join("real-findings.jsonl");
    std::fs::rename(&paths.findings, &real).unwrap();
    std::os::unix::fs::symlink(&real, &paths.findings).unwrap();
    let before = std::fs::read(&real).unwrap();

    let err = verify_finding(
        &paths,
        &verdict(&target, "run_B", VerificationStatus::Confirmed),
    )
    .unwrap_err();
    match err {
        VerifyError::Io(e) => assert!(e.to_string().contains("symlink"), "{e}"),
        e => panic!("expected an I/O refusal, got {e:?}"),
    }
    assert!(std::fs::symlink_metadata(&paths.findings)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read(&real).unwrap(), before);
}

#[test]
fn appends_racing_a_stream_of_verifications_all_land() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    const APPENDERS: usize = 3;
    const PER_APPENDER: usize = 12;

    let (_d, paths) = setup();
    let target = seed_full(&paths, "run_A");
    let done = AtomicUsize::new(0);

    std::thread::scope(|s| {
        for _ in 0..APPENDERS {
            s.spawn(|| {
                for _ in 0..PER_APPENDER {
                    seed_summary(&paths, "run_racer");
                }
                done.fetch_add(1, Ordering::SeqCst);
            });
        }
        // Rewrites the ledger over and over while the others append to it.
        let mut n = 0;
        while done.load(Ordering::SeqCst) < APPENDERS || n < 3 {
            let status = if n % 2 == 0 {
                VerificationStatus::Confirmed
            } else {
                VerificationStatus::Disputed
            };
            verify_finding(&paths, &verdict(&target, &format!("run_V{n}"), status))
                .expect("the lock keeps an append from landing mid-rewrite");
            n += 1;
        }
    });

    let records = read_findings(&paths).unwrap();
    // The target and every appended finding, none lost, none doubled.
    assert_eq!(records.len(), 1 + APPENDERS * PER_APPENDER);
    let ids: std::collections::HashSet<_> = records.iter().map(|r| r.id.clone()).collect();
    assert_eq!(ids.len(), records.len());
    let v = records
        .iter()
        .find(|r| r.id == target)
        .unwrap()
        .report
        .as_ref()
        .unwrap()
        .verification
        .clone()
        .expect("the last verdict is recorded");
    assert!(matches!(
        v.status,
        VerificationStatus::Confirmed | VerificationStatus::Disputed
    ));
}
