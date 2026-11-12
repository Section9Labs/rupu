use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
};
const ID: &str = "fnd_01J00000000000000000000002";
const ID2: &str = "fnd_01J00000000000000000000003";
const MARKDOWN: &str =
    include_str!("../../../../rupu-findings-report/tests/fixtures/import/notebin_markdown.md");
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::create_dir(root.join("workspaces")).unwrap();
    (dir, root)
}
fn project(global: &Path, name: &str, id: &str) -> PathBuf {
    let workspace = global.join(name);
    fs::create_dir(&workspace).unwrap();
    let record = rupu_workspace::Workspace {
        id: name.into(),
        path: workspace.to_string_lossy().into_owned(),
        repo_remote: None,
        initial_branch: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        last_run_at: None,
    };
    fs::write(
        global.join("workspaces").join(format!("{name}.toml")),
        toml::to_string(&record).unwrap(),
    )
    .unwrap();
    let paths = rupu_coverage::CoveragePaths::new(&workspace, "target");
    paths.ensure_dir().unwrap();
    let row = serde_json::json!({"id":id,"scope":"repo","summary":"recorded summary","severity":"high",
        "evidence":{"rationale":"fixture"},"declared_by":{"run_id":"run_fixture","model":"fixture","surface":"agent"},
        "declared_at":"2026-01-01T00:00:00Z","future_field":{"retain":true}});
    fs::write(&paths.findings, format!("{row}\n")).unwrap();
    paths.findings
}
fn report(global: &Path, name: &str, id: &str) -> PathBuf {
    let path = global.join(name);
    fs::write(&path, MARKDOWN.replace(ID, id)).unwrap();
    path
}
fn request(preview: &Preview, operation_id: &str) -> CommitRequest {
    CommitRequest {
        review_id: preview.review_id.clone(),
        review_digest: preview.review_digest.clone(),
        operation_id: operation_id.into(),
    }
}
#[test]
fn preview_captures_two_qualified_projects_without_mutation() {
    let (_dir, global) = fixture();
    let a = project(&global, "alpha", ID);
    let b = project(&global, "beta", ID2);
    let old_a = fs::read(&a).unwrap();
    let old_b = fs::read(&b).unwrap();
    let preview = prepare_at(
        &global,
        PreviewRequest {
            files: vec![report(&global, "a.md", ID), report(&global, "b.md", ID2)],
            directory: None,
        },
    )
    .unwrap();
    assert_eq!(
        (preview.ready, preview.project_count, preview.ledger_count),
        (2, 2, 2)
    );
    assert_eq!(preview.rows[0].target.as_ref().unwrap().ws_id, "alpha");

    assert_eq!(preview.rows[1].target.as_ref().unwrap().ws_id, "beta");
    assert_eq!(fs::read(a).unwrap(), old_a);
    assert_eq!(fs::read(b).unwrap(), old_b);
    assert!(!global.join("findings/artifacts").exists());
}
#[test]
fn commit_uses_snapshot_after_source_disappears_and_replay_does_not_write() {
    let (_dir, global) = fixture();
    let ledger = project(&global, "alpha", ID);
    let source = report(&global, "a.md", ID);
    let preview = prepare_at(
        &global,
        PreviewRequest {
            files: vec![source.clone()],
            directory: None,
        },
    )
    .unwrap();
    fs::remove_file(source).unwrap();
    let receipt = execute::commit_at(&global, request(&preview, "operation-one")).unwrap();
    assert_eq!(receipt.status, "completed");
    assert_eq!(receipt.rows[0].status, "attached");
    assert!(receipt.ledgers[0].backup.as_ref().unwrap().exists());
    assert_eq!(
        receipt.reviewed.rows[0].source_sha256,
        preview.rows[0].source_sha256
    );
    let bytes = fs::read(&ledger).unwrap();
    let decoded: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(decoded["report"].is_object());
    assert_eq!(decoded["future_field"]["retain"], true);
    for claim in decoded["report"]["evidence"].as_array().unwrap() {
        assert!(claim["sha256"].is_null());
    }
    let replay = execute::commit_at(&global, request(&preview, "operation-one")).unwrap();
    assert_eq!(replay.rows[0].status, "attached");
    assert_eq!(fs::read(&ledger).unwrap(), bytes);
    assert!(execute::commit_at(&global, request(&preview, "another-operation")).is_err());
}
#[test]
fn directory_commit_does_not_discover_new_reports() {
    let (_dir, global) = fixture();
    project(&global, "alpha", ID);
    let b = project(&global, "beta", ID2);
    let directory = global.join("reports");
    fs::create_dir(&directory).unwrap();
    report(&directory, "a.md", ID);
    let preview = prepare_at(
        &global,
        PreviewRequest {
            files: vec![],
            directory: Some(directory.clone()),
        },
    )
    .unwrap();
    report(&directory, "b.md", ID2);
    let before = fs::read(&b).unwrap();
    let op = execute::commit_at(&global, request(&preview, "directory-operation")).unwrap();
    assert_eq!(op.rows.len(), 1);
    assert_eq!(op.rows[0].status, "attached");
    assert_eq!(fs::read(b).unwrap(), before);
}
#[test]
fn changed_second_ledger_keeps_first_known_outcome_and_conflict() {
    let (_dir, global) = fixture();
    project(&global, "alpha", ID);
    let b = project(&global, "beta", ID2);
    let preview = prepare_at(
        &global,
        PreviewRequest {
            files: vec![report(&global, "a.md", ID), report(&global, "b.md", ID2)],
            directory: None,
        },
    )
    .unwrap();
    let changed = fs::read_to_string(&b)
        .unwrap()
        .replace("recorded summary", "different summary");
    fs::write(&b, &changed).unwrap();
    let op = execute::commit_at(&global, request(&preview, "partial-operation")).unwrap();
    assert_eq!(op.status, "completed");
    assert_eq!(op.rows[0].status, "attached");
    assert_eq!(op.rows[1].status, "conflict");
    assert_eq!(fs::read_to_string(b).unwrap(), changed);
    assert!(op.receipt_durable);
}
#[test]
fn duplicate_finding_targets_and_duplicate_source_reports_are_not_ready() {
    let (_dir, global) = fixture();
    project(&global, "alpha", ID);
    project(&global, "beta", ID);
    let preview = prepare_at(
        &global,
        PreviewRequest {
            files: vec![report(&global, "a.md", ID)],
            directory: None,
        },
    )
    .unwrap();
    assert_eq!(preview.ready, 0);
    assert_eq!(preview.failed, 1);
    assert!(execute::commit_at(&global, request(&preview, "must-not-reserve")).is_err());
    let store = store::Store::open(&global).unwrap();
    assert!(store
        .load_operation::<Operation>("must-not-reserve")
        .unwrap()
        .is_none());
    drop(store);
    let (_dir2, global2) = fixture();
    project(&global2, "alpha", ID);
    let duplicate = prepare_at(
        &global2,
        PreviewRequest {
            files: vec![report(&global2, "a.md", ID), report(&global2, "b.md", ID)],
            directory: None,
        },
    )
    .unwrap();
    assert_eq!(duplicate.ready, 0);
    assert_eq!(duplicate.failed, 2);
}
#[test]
fn interrupted_durable_operation_is_read_without_resuming_or_rewriting() {
    let (_dir, global) = fixture();
    let ledger = project(&global, "alpha", ID);
    let preview = prepare_at(
        &global,
        PreviewRequest {
            files: vec![report(&global, "a.md", ID)],
            directory: None,
        },
    )
    .unwrap();
    let before = fs::read(ledger.clone()).unwrap();
    let op = Operation {
        version: VERSION,
        reviewed: preview.clone(),
        operation_id: "interrupted".into(),
        review_id: preview.review_id.clone(),
        review_digest: preview.review_digest.clone(),
        owner: engine::owner(&global).unwrap(),
        status: "running".into(),
        receipt_durable: true,
        rows: vec![ResultRow {
            ordinal: 0,
            source_path: preview.rows[0].source_path.clone(),
            target: preview.rows[0].target.clone(),
            status: "unknown".into(),
            reason: None,
        }],
        ledgers: vec![],
        warnings: vec![],
    };
    let store = store::Store::open(&global).unwrap();
    store
        .reserve_operation(
            "interrupted",
            &preview.review_id,
            &preview.review_digest,
            op,
        )
        .unwrap();
    drop(store);
    let first = execute::read_operation(&global, "interrupted").unwrap();
    assert_eq!(first.status, "unknown");
    let repeated = execute::commit_at(&global, request(&preview, "interrupted")).unwrap();
    assert_eq!(repeated.status, "unknown");
    assert_eq!(fs::read(ledger).unwrap(), before);
    let store = store::Store::open(&global).unwrap();
    assert_eq!(
        store
            .load_operation::<Operation>("interrupted")
            .unwrap()
            .unwrap()
            .payload
            .status,
        "running"
    );
}
#[test]
fn malformed_unavailable_inventory_refuses_preview_instead_of_unique_visible_match() {
    let (_dir, global) = fixture();
    project(&global, "alpha", ID);
    fs::write(global.join("workspaces/broken.toml"), "[unterminated").unwrap();
    assert!(prepare_at(
        &global,
        PreviewRequest {
            files: vec![report(&global, "a.md", ID)],
            directory: None
        }
    )
    .is_err());
}
#[test]
fn changed_policy_and_wrong_manifest_never_reserve_or_import() {
    let (_dir, global) = fixture();
    let ledger = project(&global, "alpha", ID);
    let preview = prepare_at(
        &global,
        PreviewRequest {
            files: vec![report(&global, "a.md", ID)],
            directory: None,
        },
    )
    .unwrap();
    let before = fs::read(&ledger).unwrap();
    let mut wrong = request(&preview, "wrong-digest");
    wrong.review_digest = "0".repeat(64);
    assert!(execute::commit_at(&global, wrong).is_err());
    fs::write(
        global.join("config.toml"),
        "[findings]\nartifact_max_bytes = 4096\n",
    )
    .unwrap();
    assert!(execute::commit_at(&global, request(&preview, "changed-policy")).is_err());
    assert_eq!(fs::read(ledger).unwrap(), before);
}
