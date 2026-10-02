//! `attach_reports`: a report imported onto an existing summary finding.

use rupu_coverage::report::{FindingProfile, FindingReport, FindingWriteOptions};
use rupu_coverage::tools::report_finding::{report_finding, ReportFindingInput};
use rupu_coverage::tools::{attach_reports, AttachItem, AttachOutcome};
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

fn attribution() -> Attribution {
    Attribution {
        run_id: "run_old".into(),
        model: "m".into(),
        surface: Surface::Workflow,
        codename: None,
        agent: None,
        provider: None,
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
        asset: None,
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

#[test]
fn two_imports_in_the_same_second_keep_separate_backups() {
    let (_d, paths) = setup();
    let a = seed_summary(&paths);
    let b = seed_summary(&paths);
    let original = std::fs::read(&paths.findings).unwrap();

    let first = attach_reports(
        &paths,
        vec![AttachItem {
            finding_id: a,
            report: report(),
        }],
        &full_opts(),
        false,
    )
    .unwrap();
    let after_first = std::fs::read(&paths.findings).unwrap();
    let second = attach_reports(
        &paths,
        vec![AttachItem {
            finding_id: b,
            report: report(),
        }],
        &full_opts(),
        false,
    )
    .unwrap();

    let (b1, b2) = (first.backup.unwrap(), second.backup.unwrap());
    assert_ne!(
        b1, b2,
        "the second import must not overwrite the first backup"
    );
    // Each backup is the ledger as it stood before its own import.
    assert_eq!(std::fs::read(&b1).unwrap(), original);
    assert_eq!(std::fs::read(&b2).unwrap(), after_first);
}

#[test]
fn an_id_on_two_ledger_lines_is_left_alone() {
    let (_d, paths) = setup();
    let id = seed_summary(&paths);
    let line = std::fs::read_to_string(&paths.findings).unwrap();
    let doubled = format!("{line}{line}");
    std::fs::write(&paths.findings, &doubled).unwrap();

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
        [AttachOutcome::Duplicate]
    ));
    assert!(batch.backup.is_none());
    assert_eq!(std::fs::read_to_string(&paths.findings).unwrap(), doubled);
}

/// Names of the entries in `dir`, sorted.
fn listing(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// A directory with write permission removed, restored when dropped (so
/// the temp dir can be cleaned up even if an assertion fails first).
#[cfg(unix)]
struct ReadOnlyDir {
    dir: std::path::PathBuf,
    original: std::fs::Permissions,
}

#[cfg(unix)]
impl ReadOnlyDir {
    /// `None` when this process is not bound by directory permissions (root
    /// is not), in which case there is nothing to test.
    fn new(dir: &std::path::Path) -> Option<Self> {
        use std::os::unix::fs::PermissionsExt;
        let original = std::fs::metadata(dir).unwrap().permissions();
        let mut locked = original.clone();
        locked.set_mode(0o555);
        std::fs::set_permissions(dir, locked).unwrap();
        let guard = Self {
            dir: dir.to_path_buf(),
            original,
        };
        // Probe by trying to create a file, rather than reading the uid.
        let probe = dir.join("probe");
        if std::fs::write(&probe, "x").is_ok() {
            let _ = std::fs::remove_file(&probe);
            return None; // dropping `guard` restores the mode
        }
        Some(guard)
    }
}

#[cfg(unix)]
impl Drop for ReadOnlyDir {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.dir, self.original.clone());
    }
}

#[cfg(unix)]
#[test]
fn a_dry_run_takes_no_lock_and_works_on_a_read_only_directory() {
    let (_d, paths) = setup();
    let id = seed_summary(&paths);
    // Seeding leaves the lock sidecar behind; without it, taking the lock
    // would have to create it, which a read-only directory refuses.
    std::fs::remove_file(paths.root.join("findings.jsonl.lock")).unwrap();
    let before_bytes = std::fs::read(&paths.findings).unwrap();
    let before_listing = listing(&paths.root);
    let Some(_guard) = ReadOnlyDir::new(&paths.root) else {
        eprintln!("skipped: directory permissions are not enforced for this user");
        return;
    };

    let dry = attach_reports(
        &paths,
        vec![AttachItem {
            finding_id: id.clone(),
            report: report(),
        }],
        &full_opts(),
        true,
    )
    .expect("a dry run only reads");
    assert!(matches!(dry.outcomes.as_slice(), [AttachOutcome::Attached]));
    assert!(dry.backup.is_none());
    assert_eq!(std::fs::read(&paths.findings).unwrap(), before_bytes);
    assert_eq!(listing(&paths.root), before_listing);

    // A real run needs the lock and the directory, so it cannot go ahead.
    // Every item is still assessed: a report with problems lists them, and
    // one that would have attached is "not written".
    let other = {
        drop(_guard);
        let other = seed_summary(&paths);
        std::fs::remove_file(paths.root.join("findings.jsonl.lock")).unwrap();
        other
    };
    let before_bytes = std::fs::read(&paths.findings).unwrap();
    let _guard = ReadOnlyDir::new(&paths.root).expect("enforced above");
    let mut invalid = report();
    invalid.title = "   ".into();
    let real = attach_reports(
        &paths,
        vec![
            AttachItem {
                finding_id: other,
                report: invalid,
            },
            AttachItem {
                finding_id: id,
                report: report(),
            },
        ],
        &full_opts(),
        false,
    )
    .expect("the ledger is readable");
    assert!(real.write_error.is_some());
    assert!(real.backup.is_none());
    match &real.outcomes[0] {
        AttachOutcome::Rejected(e) => assert!(e.to_string().contains("report.title"), "{e}"),
        o => panic!("expected Rejected, got {o:?}"),
    }
    assert!(
        matches!(real.outcomes[1], AttachOutcome::NotWritten),
        "{:?}",
        real.outcomes[1]
    );
    assert_eq!(std::fs::read(&paths.findings).unwrap(), before_bytes);
}

#[test]
fn imported_claims_carry_no_hash() {
    let (_d, paths) = setup();
    let id = seed_summary(&paths);
    // The claim's file is in the workspace, so `report_finding` would hash
    // it. An imported claim was made against the code as it was when the
    // report was written, so it is left unhashed.
    std::fs::create_dir_all(paths.workspace.join("src/routes")).unwrap();
    std::fs::write(
        paths.workspace.join("src/routes/notes.rs"),
        "async fn get_note() {}\n",
    )
    .unwrap();
    let mut r = report();
    assert_eq!(r.evidence[0].file.as_deref(), Some("src/routes/notes.rs"));
    r.evidence[0].sha256 = Some("ab".repeat(32));
    let batch = attach_reports(
        &paths,
        vec![AttachItem {
            finding_id: id.clone(),
            report: r,
        }],
        &full_opts(),
        false,
    )
    .unwrap();
    assert!(matches!(
        batch.outcomes.as_slice(),
        [AttachOutcome::Attached]
    ));
    let rec = read_findings(&paths)
        .unwrap()
        .into_iter()
        .find(|f| f.id == id)
        .unwrap();
    let evidence = &rec.report.unwrap().evidence;
    assert!(evidence.iter().all(|c| c.sha256.is_none()), "{evidence:?}");
}

#[test]
fn a_report_cannot_cross_reference_the_finding_it_is_attached_to() {
    use rupu_coverage::report::{CrossRef, OrSentinel, Relation};
    let (_d, paths) = setup();
    let id = seed_summary(&paths);
    let other = seed_summary(&paths);
    let cross = |to: &str| CrossRef {
        finding_id: to.to_string(),
        relation: Relation::Sibling,
        note: None,
    };
    let mut to_itself = report();
    to_itself.cross_references = OrSentinel::Value(vec![cross(&other), cross(&id)]);
    let mut to_other = report();
    to_other.cross_references = OrSentinel::Value(vec![cross(&other)]);
    for dry_run in [true, false] {
        let batch = attach_reports(
            &paths,
            vec![AttachItem {
                finding_id: id.clone(),
                report: to_itself.clone(),
            }],
            &full_opts(),
            dry_run,
        )
        .unwrap();
        match batch.outcomes.as_slice() {
            [AttachOutcome::Rejected(e)] => assert!(
                e.to_string()
                    .contains("report.cross_references[1].finding_id"),
                "{e}"
            ),
            o => panic!("dry_run={dry_run}: expected Rejected, got {o:?}"),
        }
    }
    // The other finding is still a valid target, and the id is restored for
    // the items after it.
    let batch = attach_reports(
        &paths,
        vec![
            AttachItem {
                finding_id: id.clone(),
                report: to_itself,
            },
            AttachItem {
                finding_id: id.clone(),
                report: to_other,
            },
        ],
        &full_opts(),
        false,
    )
    .unwrap();
    assert!(matches!(batch.outcomes[0], AttachOutcome::Rejected(_)));
    assert!(
        matches!(batch.outcomes[1], AttachOutcome::Attached),
        "{:?}",
        batch.outcomes[1]
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_ledger_is_never_replaced() {
    let (d, paths) = setup();
    let id = seed_summary(&paths);
    // The real ledger lives elsewhere; the coverage directory holds a link.
    let real = d.path().join("elsewhere.jsonl");
    std::fs::rename(&paths.findings, &real).unwrap();
    std::os::unix::fs::symlink(&real, &paths.findings).unwrap();
    let before = std::fs::read(&real).unwrap();

    for dry_run in [true, false] {
        let batch = attach_reports(
            &paths,
            vec![AttachItem {
                finding_id: id.clone(),
                report: report(),
            }],
            &full_opts(),
            dry_run,
        )
        .unwrap();
        let err = batch.write_error.expect("refused");
        let text = err.to_string();
        assert!(text.contains("is a symlink"), "{text}");
        assert!(
            text.contains(&paths.findings.display().to_string()),
            "{text}"
        );
        assert!(matches!(
            batch.outcomes.as_slice(),
            [AttachOutcome::NotWritten]
        ));
        assert!(batch.backup.is_none());
        assert!(std::fs::symlink_metadata(&paths.findings)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(&real).unwrap(), before);
    }
    assert!(
        !listing(&paths.root)
            .iter()
            .any(|n| n.contains("pre-import")),
        "{:?}",
        listing(&paths.root)
    );
}

#[test]
fn a_dry_run_on_a_missing_ledger_creates_nothing() {
    let (d, paths) = setup();
    let batch = attach_reports(
        &paths,
        vec![AttachItem {
            finding_id: "fnd_01J00000000000000000000001".into(),
            report: report(),
        }],
        &full_opts(),
        true,
    )
    .unwrap();
    assert!(matches!(
        batch.outcomes.as_slice(),
        [AttachOutcome::NotFound]
    ));
    // Not even the coverage directory.
    assert!(listing(d.path()).is_empty());
}

fn artifact(path: &str) -> rupu_coverage::report::ArtifactRef {
    rupu_coverage::report::ArtifactRef {
        path: path.into(),
        sha256: String::new(),
        size: 0,
        kind: None,
        stored: None,
        host: None,
    }
}

#[test]
fn a_dry_run_rejects_a_missing_artifact_as_a_real_run_does() {
    let (d, paths) = setup();
    let id = seed_summary(&paths);
    let store = d.path().join("store");
    let opts = FindingWriteOptions {
        artifact_root: Some(store.clone()),
        ..full_opts()
    };
    let mut r = report();
    r.artifacts = vec![artifact("evidence/absent.txt")];
    let item = || {
        vec![AttachItem {
            finding_id: id.clone(),
            report: r.clone(),
        }]
    };
    let before = std::fs::read(&paths.findings).unwrap();

    let dry = attach_reports(&paths, item(), &opts, true).unwrap();
    let real = attach_reports(&paths, item(), &opts, false).unwrap();
    for batch in [dry, real] {
        match batch.outcomes.as_slice() {
            [AttachOutcome::Rejected(rupu_coverage::ReportFindingError::Artifact(
                rupu_coverage::report::ArtifactError::Missing { path },
            ))] => assert_eq!(path, "evidence/absent.txt"),
            o => panic!("expected Rejected(Missing), got {o:?}"),
        }
        assert!(batch.backup.is_none());
    }
    assert_eq!(std::fs::read(&paths.findings).unwrap(), before);
    assert!(!store.exists(), "no artifact store was created");
}

#[test]
fn a_dry_run_applies_the_artifact_count_and_size_limits_a_real_run_does() {
    use rupu_coverage::report::ArtifactError;
    let (d, paths) = setup();
    let id = seed_summary(&paths);
    std::fs::write(paths.workspace.join("a.txt"), "xx").unwrap();
    std::fs::write(paths.workspace.join("b.txt"), "xx").unwrap();
    let mut r = report();
    r.artifacts = vec![artifact("a.txt"), artifact("b.txt")];
    let store = d.path().join("store");
    let too_many = FindingWriteOptions {
        artifact_root: Some(store.clone()),
        artifact_max_files: 1,
        ..full_opts()
    };
    let too_large = FindingWriteOptions {
        artifact_root: Some(store.clone()),
        artifact_total_max_bytes: 3,
        ..full_opts()
    };
    for dry in [true, false] {
        let item = || {
            vec![AttachItem {
                finding_id: id.clone(),
                report: r.clone(),
            }]
        };
        let batch = attach_reports(&paths, item(), &too_many, dry).unwrap();
        assert!(
            matches!(
                batch.outcomes.as_slice(),
                [AttachOutcome::Rejected(
                    rupu_coverage::ReportFindingError::Artifact(ArtifactError::TooManyFiles {
                        max: 1
                    })
                )]
            ),
            "dry={dry}: {:?}",
            batch.outcomes
        );
        let batch = attach_reports(&paths, item(), &too_large, dry).unwrap();
        assert!(
            matches!(
                batch.outcomes.as_slice(),
                [AttachOutcome::Rejected(
                    rupu_coverage::ReportFindingError::Artifact(ArtifactError::TooLarge {
                        total: 4,
                        ..
                    })
                )]
            ),
            "dry={dry}: {:?}",
            batch.outcomes
        );
    }
}

/// A workspace whose `evidence/` directory holds a text and a binary file,
/// the report listing the directory, and options with `report_max_bytes`.
fn expanding(
    max: usize,
) -> (
    tempfile::TempDir,
    CoveragePaths,
    String,
    FindingWriteOptions,
) {
    let (d, paths) = setup();
    let id = seed_summary(&paths);
    std::fs::create_dir_all(paths.workspace.join("evidence")).unwrap();
    std::fs::write(
        paths.workspace.join("evidence/request.txt"),
        "GET /api/notes/2",
    )
    .unwrap();
    std::fs::write(paths.workspace.join("evidence/dump.bin"), [0u8, 1, 2, 3]).unwrap();
    let opts = FindingWriteOptions {
        artifact_root: Some(d.path().join("store")),
        report_max_bytes: max,
        ..full_opts()
    };
    (d, paths, id, opts)
}

fn directory_report() -> FindingReport {
    let mut r = report();
    r.artifacts = vec![artifact("evidence")];
    r
}

#[test]
fn a_dry_run_checks_the_size_of_the_report_as_it_would_be_stored() {
    // The report as a real import stores it: the directory expanded, each
    // file with its hash, size, kind and storage.
    let (_d, paths, id, opts) = expanding(256 * 1024);
    let batch = attach_reports(
        &paths,
        vec![AttachItem {
            finding_id: id.clone(),
            report: directory_report(),
        }],
        &opts,
        false,
    )
    .unwrap();
    assert!(matches!(
        batch.outcomes.as_slice(),
        [AttachOutcome::Attached]
    ));
    let stored = read_findings(&paths)
        .unwrap()
        .into_iter()
        .find(|f| f.id == id)
        .unwrap()
        .report
        .unwrap();
    assert_eq!(stored.artifacts.len(), 2);
    let size = serde_json::to_vec(&stored).unwrap().len();
    // Well under that before the directory is expanded.
    assert!(serde_json::to_vec(&directory_report()).unwrap().len() < size - 200);

    // At exactly that budget both runs attach; a byte under, both refuse.
    for (max, attaches) in [(size, true), (size - 1, false)] {
        for dry in [true, false] {
            let (_d, paths, id, opts) = expanding(max);
            let batch = attach_reports(
                &paths,
                vec![AttachItem {
                    finding_id: id,
                    report: directory_report(),
                }],
                &opts,
                dry,
            )
            .unwrap();
            match batch.outcomes.as_slice() {
                [AttachOutcome::Attached] if attaches => {}
                [AttachOutcome::Rejected(rupu_coverage::ReportFindingError::Report(v))]
                    if !attaches =>
                {
                    assert_eq!(v.0[0].path, "report.artifacts", "{v:?}");
                    assert!(v.0[0].message.contains(&format!("{size} bytes")), "{v:?}");
                }
                o => panic!("max={max} dry={dry}: {o:?}"),
            }
        }
    }
}

#[test]
fn a_dry_run_accepts_a_present_artifact_without_copying_it() {
    let (d, paths) = setup();
    let id = seed_summary(&paths);
    std::fs::create_dir_all(paths.workspace.join("evidence")).unwrap();
    std::fs::write(paths.workspace.join("evidence/request.txt"), "GET /x").unwrap();
    let store = d.path().join("store");
    let opts = FindingWriteOptions {
        artifact_root: Some(store.clone()),
        ..full_opts()
    };
    let mut r = report();
    r.artifacts = vec![artifact("evidence/request.txt")];
    let batch = attach_reports(
        &paths,
        vec![AttachItem {
            finding_id: id,
            report: r,
        }],
        &opts,
        true,
    )
    .unwrap();
    assert!(matches!(
        batch.outcomes.as_slice(),
        [AttachOutcome::Attached]
    ));
    assert!(!store.exists(), "a dry run copies nothing");
}

#[test]
fn a_dry_run_refuses_artifacts_when_there_is_no_store() {
    let (_d, paths) = setup();
    let id = seed_summary(&paths);
    std::fs::write(paths.workspace.join("out.txt"), "x").unwrap();
    let mut r = report();
    r.artifacts = vec![rupu_coverage::report::ArtifactRef {
        path: "out.txt".into(),
        sha256: String::new(),
        size: 0,
        kind: None,
        stored: None,
        host: None,
    }];
    let opts = full_opts(); // no artifact_root
    let dry = attach_reports(
        &paths,
        vec![AttachItem {
            finding_id: id.clone(),
            report: r.clone(),
        }],
        &opts,
        true,
    )
    .unwrap();
    // The same refusal a real run gives.
    let real = attach_reports(
        &paths,
        vec![AttachItem {
            finding_id: id,
            report: r,
        }],
        &opts,
        false,
    )
    .unwrap();
    for batch in [dry, real] {
        match batch.outcomes.as_slice() {
            [AttachOutcome::Rejected(rupu_coverage::ReportFindingError::Artifact(
                rupu_coverage::report::ArtifactError::NoStore,
            ))] => {}
            o => panic!("expected Rejected(NoStore), got {o:?}"),
        }
    }
}

#[test]
fn untouched_bytes_survive_crlf_non_utf8_and_a_missing_final_newline() {
    let (_d, paths) = setup();
    seed_summary(&paths);
    let b = seed_summary(&paths);
    let c = seed_summary(&paths);
    let ledger = std::fs::read_to_string(&paths.findings).unwrap();
    let mut lines = ledger.lines();
    let (la, lb, lc) = (
        lines.next().unwrap(),
        lines.next().unwrap(),
        lines.next().unwrap(),
    );

    // a: CRLF, untouched. Then a line that is not UTF-8, untouched. b: CRLF,
    // attached, must keep its CRLF. c: no final newline, attached, must stay
    // unterminated.
    let mut original = Vec::new();
    original.extend_from_slice(format!("{la}\r\n").as_bytes());
    original.extend_from_slice(b"\xff\xfe not utf-8\n");
    original.extend_from_slice(format!("{lb}\r\n").as_bytes());
    original.extend_from_slice(lc.as_bytes());
    std::fs::write(&paths.findings, &original).unwrap();

    let batch = attach_reports(
        &paths,
        vec![
            AttachItem {
                finding_id: b.clone(),
                report: report(),
            },
            AttachItem {
                finding_id: c.clone(),
                report: report(),
            },
        ],
        &full_opts(),
        false,
    )
    .unwrap();
    assert!(
        batch
            .outcomes
            .iter()
            .all(|o| matches!(o, AttachOutcome::Attached)),
        "{:?}",
        batch.outcomes
    );
    assert_eq!(std::fs::read(batch.backup.unwrap()).unwrap(), original);

    let after = std::fs::read(&paths.findings).unwrap();
    let segments: Vec<&[u8]> = after.split_inclusive(|b| *b == b'\n').collect();
    assert_eq!(segments.len(), 4);
    // Untouched: byte for byte, CRLF and all.
    assert_eq!(segments[0], format!("{la}\r\n").as_bytes());
    assert_eq!(segments[1], b"\xff\xfe not utf-8\n");
    // Replaced: each keeps its own terminator.
    assert!(segments[2].ends_with(b"\r\n"));
    assert!(
        !segments[3].ends_with(b"\n"),
        "the last line stays unterminated"
    );
    for (seg, id) in [(segments[2], &b), (segments[3], &c)] {
        let v: serde_json::Value = serde_json::from_slice(seg).unwrap();
        assert_eq!(&v["id"], id.as_str());
        assert_eq!(v["profile"], "full");
        assert_eq!(v["report"]["title"], report().title.as_str());
    }
}

#[test]
fn a_stored_report_the_lenient_load_dropped_is_never_overwritten() {
    let (_d, paths) = setup();
    let id = seed_summary(&paths);
    // Legacy-shaped: no `profile` key, so it loads as a summary record, but
    // it carries a `report` this build cannot parse.
    let mut v: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&paths.findings).unwrap().trim()).unwrap();
    v.as_object_mut().unwrap().remove("profile");
    v["report"] = serde_json::json!({ "title": "only a title" });
    let line = format!("{}\n", serde_json::to_string(&v).unwrap());
    std::fs::write(&paths.findings, &line).unwrap();
    let loaded = &read_findings(&paths).unwrap()[0];
    assert!(loaded.report.is_none(), "loads leniently");
    assert_eq!(loaded.profile, FindingProfile::Summary);

    for dry_run in [true, false] {
        let batch = attach_reports(
            &paths,
            vec![AttachItem {
                finding_id: id.clone(),
                report: report(),
            }],
            &full_opts(),
            dry_run,
        )
        .unwrap();
        assert!(matches!(
            batch.outcomes.as_slice(),
            [AttachOutcome::AlreadyHasReport]
        ));
        assert_eq!(std::fs::read_to_string(&paths.findings).unwrap(), line);
    }
}

#[test]
fn writers_wait_for_the_ledger_lock() {
    use std::sync::mpsc::{channel, RecvTimeoutError};
    use std::time::Duration;

    let (_d, paths) = setup();
    let id = seed_summary(&paths);

    // Hold the ledger lock from here, the way a writer mid-append would.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths.root.join("findings.jsonl.lock"))
        .unwrap();
    lock.lock().unwrap();

    let (tx, rx) = channel();
    let append = {
        let (paths, tx) = (paths.clone(), tx.clone());
        std::thread::spawn(move || {
            seed_summary(&paths);
            tx.send("append").unwrap();
        })
    };
    let attach = {
        let (paths, tx, id) = (paths.clone(), tx, id.clone());
        std::thread::spawn(move || {
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
                [AttachOutcome::Attached]
            ));
            tx.send("attach").unwrap();
        })
    };

    match rx.recv_timeout(Duration::from_millis(300)) {
        Err(RecvTimeoutError::Timeout) => {}
        other => panic!("a writer finished while the lock was held: {other:?}"),
    }
    drop(lock); // releases the lock

    let mut done = vec![
        rx.recv_timeout(Duration::from_secs(10))
            .expect("first writer completes"),
        rx.recv_timeout(Duration::from_secs(10))
            .expect("second writer completes"),
    ];
    done.sort_unstable();
    assert_eq!(done, ["append", "attach"]);
    append.join().unwrap();
    attach.join().unwrap();

    // Neither writer lost the other's work, whichever went first.
    let records = read_findings(&paths).unwrap();
    assert_eq!(records.len(), 2);
    let attached = records.iter().find(|r| r.id == id).unwrap();
    assert_eq!(attached.profile, FindingProfile::Full);
    assert!(attached.report.is_some());
    assert!(records.iter().any(|r| r.id != id && r.report.is_none()));
}
