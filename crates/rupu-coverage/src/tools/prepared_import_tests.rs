use super::*;
use crate::tools::report_finding::{report_finding, ReportFindingInput};
use crate::{Attribution, FindingEvidence, FindingScope, Severity, Surface};

fn report() -> FindingReport {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/finding_report/valid_full.json"
    ))
    .unwrap()
}
fn artifact(path: &str) -> crate::report::ArtifactRef {
    serde_json::from_value(serde_json::json!({"path":path})).unwrap()
}
fn setup() -> (
    tempfile::TempDir,
    CoveragePaths,
    FindingWriteOptions,
    std::path::PathBuf,
    String,
) {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let workspace = root.join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let paths = CoveragePaths::new(&workspace, "target");
    let id = report_finding(
        &paths,
        Attribution {
            run_id: "r".into(),
            model: "m".into(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
        },
        ReportFindingInput {
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::Repo,
            summary: Some("s".into()),
            severity: Some(Severity::Low),
            concern_id: None,
            evidence: Some(FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            }),
            report: None,
            asset: None,
            tags: vec![],
        },
        &FindingWriteOptions::default().with_profile(FindingProfile::Summary),
    )
    .unwrap()
    .id;
    let opts = FindingWriteOptions {
        artifact_root: Some(root.join("live")),
        ..FindingWriteOptions::default()
    };
    (temp, paths, opts, root.join("private"), id)
}

#[test]
fn captured_bytes_survive_original_change_and_claims_stay_unknown() {
    let (_temp, paths, opts, private, id) = setup();
    std::fs::write(paths.workspace.join("proof.txt"), b"reviewed bytes").unwrap();
    let mut value = report();
    value.artifacts = vec![artifact("proof.txt")];
    value.evidence[0].sha256 = Some("a".repeat(64));
    let prepared = prepare_import_report(&paths, value, &[], &opts, &private).unwrap();
    assert!(prepared.report.evidence.iter().all(|c| c.sha256.is_none()));
    assert!(!opts.artifact_root.as_ref().unwrap().exists());
    let serialized = serde_json::to_vec(&prepared).unwrap();
    let prepared: PreparedImportReport = serde_json::from_slice(&serialized).unwrap();
    let blob = prepared.blobs[0].clone();
    let (identity, sha) = capture_import_ledger(&paths).unwrap();
    std::fs::remove_file(paths.workspace.join("proof.txt")).unwrap();
    let result = attach_prepared_reports(
        &paths,
        &[PreparedAttachItem {
            finding_id: id,
            prepared,
        }],
        &opts,
        &sha,
        &identity,
        &private,
    )
    .unwrap();
    assert!(matches!(result.outcomes[0], AttachOutcome::Attached));
    assert!(result.backup.unwrap().exists());
    assert_eq!(
        std::fs::read(
            ArtifactStore::new(opts.artifact_root.as_ref().unwrap()).blob_path(&blob.sha256)
        )
        .unwrap(),
        b"reviewed bytes"
    );
}

#[test]
fn same_size_ledger_change_refuses_before_live_artifact_install() {
    let (_temp, paths, opts, private, id) = setup();
    std::fs::write(paths.workspace.join("proof"), b"proof").unwrap();
    let mut value = report();
    value.artifacts = vec![artifact("proof")];
    let prepared = prepare_import_report(&paths, value, &[], &opts, &private).unwrap();
    let (identity, sha) = capture_import_ledger(&paths).unwrap();
    let raw = std::fs::read(&paths.findings).unwrap();
    let mut changed = raw.clone();
    changed[0] = b' ';
    std::fs::write(&paths.findings, &changed).unwrap();
    let error = attach_prepared_reports(
        &paths,
        &[PreparedAttachItem {
            finding_id: id,
            prepared,
        }],
        &opts,
        &sha,
        &identity,
        &private,
    )
    .unwrap_err();
    assert!(matches!(error, PreparedImportError::LedgerChanged));
    assert_eq!(std::fs::read(&paths.findings).unwrap(), changed);
    assert!(!opts.artifact_root.as_ref().unwrap().exists());
}

#[test]
fn replaced_identical_ledger_and_symlink_parent_are_refused() {
    let (_temp, paths, opts, private, id) = setup();
    let prepared = prepare_import_report(&paths, report(), &[], &opts, &private).unwrap();
    let (identity, sha) = capture_import_ledger(&paths).unwrap();
    let raw = std::fs::read(&paths.findings).unwrap();
    std::fs::rename(&paths.findings, paths.root.join("old-ledger")).unwrap();
    std::fs::write(&paths.findings, &raw).unwrap();
    assert!(matches!(
        attach_prepared_reports(
            &paths,
            &[PreparedAttachItem {
                finding_id: id,
                prepared
            }],
            &opts,
            &sha,
            &identity,
            &private
        ),
        Err(PreparedImportError::LedgerChanged)
    ));
    let moved = paths.root.with_extension("moved");
    std::fs::rename(&paths.root, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &paths.root).unwrap();
    assert!(capture_import_ledger(&paths).is_err());
}

#[test]
fn corrupt_private_blob_cannot_attach_or_change_ledger() {
    let (_temp, paths, opts, private, id) = setup();
    std::fs::write(paths.workspace.join("proof"), b"proof").unwrap();
    let mut value = report();
    value.artifacts = vec![artifact("proof")];
    let prepared = prepare_import_report(&paths, value, &[], &opts, &private).unwrap();
    let raw = std::fs::read(&paths.findings).unwrap();
    let (identity, sha) = capture_import_ledger(&paths).unwrap();
    let blob = ArtifactStore::new(&private).blob_path(&prepared.blobs[0].sha256);
    std::fs::write(blob, b"wrong").unwrap();
    assert!(attach_prepared_reports(
        &paths,
        &[PreparedAttachItem {
            finding_id: id,
            prepared
        }],
        &opts,
        &sha,
        &identity,
        &private
    )
    .is_err());
    assert_eq!(std::fs::read(&paths.findings).unwrap(), raw);
}

#[test]
fn failed_preparation_removes_partial_captures_and_rejects_external_files() {
    let (_temp, paths, mut opts, private, _) = setup();
    std::fs::write(paths.workspace.join("proof"), b"proof").unwrap();
    let mut value = report();
    value.artifacts = vec![artifact("proof")];
    value.blocks.push(crate::report::EvidenceBlock::PcapRef {
        artifact: artifact("absent.pcap"),
        summary: "Missing second dependency".into(),
    });
    assert!(prepare_import_report(&paths, value, &[], &opts, &private).is_err());
    assert!(std::fs::read_dir(&private).unwrap().next().is_none());
    opts.artifact_max_bytes = 2;
    let mut value = report();
    value.artifacts = vec![artifact("proof")];
    assert!(prepare_import_report(&paths, value, &[], &opts, &private).is_err());
    assert!(std::fs::read_dir(&private).unwrap().next().is_none());
    assert!(!opts.artifact_root.as_ref().unwrap().exists());
}

#[test]
fn existing_full_report_is_never_overwritten_and_other_bytes_survive() {
    let (_temp, paths, opts, private, id) = setup();
    let mut raw = std::fs::read(&paths.findings).unwrap();
    raw.extend_from_slice(b"opaque non-json\r\n\xffuntouched");
    std::fs::write(&paths.findings, &raw).unwrap();
    let prepared = prepare_import_report(&paths, report(), &[], &opts, &private).unwrap();
    let item = PreparedAttachItem {
        finding_id: id,
        prepared,
    };
    let (identity, sha) = capture_import_ledger(&paths).unwrap();
    let first = attach_prepared_reports(
        &paths,
        std::slice::from_ref(&item),
        &opts,
        &sha,
        &identity,
        &private,
    )
    .unwrap();
    assert_eq!(std::fs::read(first.backup.unwrap()).unwrap(), raw);
    let after = std::fs::read(&paths.findings).unwrap();
    assert!(after.ends_with(b"opaque non-json\r\n\xffuntouched"));
    let (identity, sha) = capture_import_ledger(&paths).unwrap();
    let second =
        attach_prepared_reports(&paths, &[item], &opts, &sha, &identity, &private).unwrap();
    assert!(matches!(
        second.outcomes[0],
        AttachOutcome::AlreadyHasReport
    ));
    assert!(second.backup.is_none());
    assert_eq!(std::fs::read(&paths.findings).unwrap(), after);
}

#[test]
fn held_parent_rename_does_not_redirect_replacement() {
    let (_temp, paths, _opts, _private, _) = setup();
    let (identity, parent, mut ledger) = open_ledger(&paths).unwrap();
    let raw = read_ledger(&mut ledger).unwrap();
    let sha = digest(&raw).unwrap();
    let moved = paths.root.with_extension("moved");
    let mut substitute = || {
        std::fs::rename(&paths.root, &moved).unwrap();
        std::fs::create_dir(&paths.root).unwrap();
        std::fs::write(&paths.findings, b"foreign ledger").unwrap();
    };
    replace_prepared(
        &paths,
        &parent,
        &mut ledger,
        &raw,
        &[b"reviewed replacement".to_vec()],
        &identity,
        &sha,
        &mut substitute,
        &mut |directory| directory.sync_all(),
    )
    .unwrap();
    assert_eq!(std::fs::read(&paths.findings).unwrap(), b"foreign ledger");
    assert_eq!(
        std::fs::read(moved.join("findings.jsonl")).unwrap(),
        b"reviewed replacement"
    );
}

#[test]
fn private_capture_files_are_single_link_and_private() {
    use std::os::unix::fs::MetadataExt;
    let (_temp, paths, opts, private, _) = setup();
    std::fs::write(paths.workspace.join("proof"), b"proof").unwrap();
    let mut value = report();
    value.artifacts = vec![artifact("proof")];
    let prepared = prepare_import_report(&paths, value, &[], &opts, &private).unwrap();
    let blob = ArtifactStore::new(&private).blob_path(&prepared.blobs[0].sha256);
    assert_eq!(std::fs::metadata(&blob).unwrap().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(&blob).unwrap().nlink(), 1);
    assert_eq!(
        std::fs::metadata(blob.parent().unwrap()).unwrap().mode() & 0o777,
        0o700
    );
    assert_eq!(std::fs::metadata(private).unwrap().mode() & 0o777, 0o700);
}

#[test]
fn directory_sync_failure_after_rename_is_uncertain_and_keeps_backup() {
    let (_temp, paths, _opts, _private, _) = setup();
    let (identity, parent, mut ledger) = open_ledger(&paths).unwrap();
    let raw = read_ledger(&mut ledger).unwrap();
    let sha = digest(&raw).unwrap();
    let mut observed_after_rename = false;
    let mut fail_sync = |_: &File| {
        observed_after_rename = std::fs::read(&paths.findings).unwrap() == b"new ledger bytes";
        Err(std::io::Error::other("injected directory sync failure"))
    };
    let error = replace_prepared(
        &paths,
        &parent,
        &mut ledger,
        &raw,
        &[b"new ledger bytes".to_vec()],
        &identity,
        &sha,
        &mut || {},
        &mut fail_sync,
    )
    .unwrap_err();
    assert!(observed_after_rename);
    match error {
        PreparedImportError::Unconfirmed {
            backup: Some(path),
            reason,
        } => {
            assert!(reason.contains("injected directory sync failure"));
            assert_eq!(std::fs::read(path).unwrap(), raw);
        }
        other => panic!("post-rename failure must remain uncertain: {other}"),
    }
    assert_eq!(std::fs::read(&paths.findings).unwrap(), b"new ledger bytes");
    assert!(!std::fs::read_dir(&paths.root).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("findings.jsonl.prepared-")));
}
