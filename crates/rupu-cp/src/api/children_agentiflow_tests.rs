use super::*;
use axum::http::StatusCode;
use rupu_launch::ledger::{self, LedgerLine, Started};
use rupu_tools::ChildKind;
use serde_json::json;
use std::{fs, io::Write};

fn fixture() -> (tempfile::TempDir, Owner, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().canonicalize().unwrap();
    let runs = global.join("runs");
    fs::create_dir(&runs).unwrap();
    let owner = Owner { global, runs };
    let dir = owner.global.join("agentiflows/af_owned");
    fs::create_dir_all(dir.join("lead")).unwrap();
    fs::write(dir.join("agentiflow.json"), json!({"id":"af_owned","name":"review","engagement_profiles":[],"trigger":"agentiflow","status":"completed","rounds":1,"goals":[],"started_at":"2026-11-09T00:00:00Z"}).to_string()).unwrap();
    fs::write(dir.join("lead/transcript.r0.jsonl"), format!("{}\n", json!({"type":"run_start","data":{"run_id":"af_owned","workspace_id":"ws_project","agent":"lead","provider":"test","model":"test","started_at":"2026-11-09T00:00:00Z","mode":"readonly"}}))).unwrap();
    let mut started = Started::new(
        "ch_actual",
        ChildKind::Subagent,
        "review",
        "sub_actual",
        1,
        1,
    );
    started.tree = Some(dir.clone());
    started.round = Some(0);
    started.transcript = Some(owner.runs.join("af_owned/sub/sub_actual/transcript.jsonl"));
    ledger::append(&dir, &LedgerLine::Started(started));
    (tmp, owner, dir)
}
fn status<T>(result: ApiResult<T>) -> StatusCode {
    result.err().expect("must reject").0
}

#[test]
fn agentiflow_root_uses_exact_record_and_ledger_without_fabricating_run_json() {
    let (_tmp, owner, dir) = fixture();
    let captured = capture::capture(owner.clone(), "af_owned", Some("af_owned".into())).unwrap();
    assert_eq!(captured.proof.kind, "agentiflow");
    assert_eq!(captured.proof.project_id.as_deref(), Some("ws_project"));
    assert_eq!(captured.rows.len(), 1);
    let child = &captured.rows[0];
    assert_eq!(child.parent_run_id, "af_owned");
    assert_eq!(child.run_id, "sub_actual");
    assert_eq!(child.handle.as_deref(), Some("ch_actual"));
    assert_eq!(child.round, Some(0));
    assert_eq!(
        child.transcript_path.as_deref(),
        owner
            .runs
            .join("af_owned/sub/sub_actual/transcript.jsonl")
            .to_str()
    );
    assert!(!owner.runs.join("af_owned/run.json").exists());
    assert!(dir.join("agentiflow.json").is_file());
}
#[test]
fn agentiflow_root_rejects_foreign_record_prologue_host_and_project_alias() {
    for target in ["record", "run", "host", "project"] {
        let (_tmp, owner, dir) = fixture();
        let path = if target == "record" {
            dir.join("agentiflow.json")
        } else {
            dir.join("lead/transcript.r0.jsonl")
        };
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match target {
            "record" => value["id"] = "af_foreign".into(),
            "run" => value["data"]["run_id"] = "af_foreign".into(),
            "host" => value["data"]["host_id"] = "peer".into(),
            _ => value["workspace_id"] = "ws_foreign".into(),
        }
        fs::write(path, format!("{value}\n")).unwrap();
        assert_eq!(
            status(capture::capture(owner, "af_owned", None)),
            StatusCode::CONFLICT,
            "{target}"
        );
    }
}
#[test]
fn agentiflow_root_never_skips_first_record_or_downloads_large_lead_body() {
    let (_tmp, owner, dir) = fixture();
    let path = dir.join("lead/transcript.r0.jsonl");
    let original = fs::read(&path).unwrap();
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(&vec![b'x'; 1024 * 1024]).unwrap();
    let captured = capture::capture(owner.clone(), "af_owned", None).unwrap();
    assert!(captured.evidence.bytes_read < 16 * 1024);
    fs::write(&path, [b"not-json\n".as_slice(), &original].concat()).unwrap();
    assert_eq!(
        status(capture::capture(owner.clone(), "af_owned", None)),
        StatusCode::CONFLICT
    );
    fs::write(&path, vec![b'x'; 256 * 1024 + 1]).unwrap();
    assert_eq!(
        status(capture::capture(owner, "af_owned", None)),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}
#[test]
fn agentiflow_root_requires_prologue_and_canonical_ledger() {
    for file in ["lead/transcript.r0.jsonl", "children.jsonl"] {
        let (_tmp, owner, dir) = fixture();
        fs::remove_file(dir.join(file)).unwrap();
        assert_eq!(
            status(capture::capture(owner, "af_owned", None)),
            StatusCode::NOT_IMPLEMENTED
        );
    }
}
#[test]
fn agentiflow_root_rejects_conflicting_run_and_tree_ownership() {
    let (_tmp, owner, dir) = fixture();
    fs::create_dir_all(owner.runs.join("af_owned")).unwrap();
    fs::write(owner.runs.join("af_owned/run.json"), "{}").unwrap();
    assert_eq!(
        status(capture::capture(owner.clone(), "af_owned", None)),
        StatusCode::CONFLICT
    );
    fs::remove_file(owner.runs.join("af_owned/run.json")).unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("children.jsonl")).unwrap()).unwrap();
    value["tree"] = "/different/root".into();
    fs::write(dir.join("children.jsonl"), format!("{value}\n")).unwrap();
    assert_eq!(
        status(capture::capture(owner, "af_owned", None)),
        StatusCode::CONFLICT
    );
}
#[test]
fn agentiflow_root_cursor_rechecks_proof_and_parent_scope() {
    let (_tmp, owner, dir) = fixture();
    let mut cache = Cache::default();
    let captured = capture::capture(owner.clone(), "af_owned", Some("af_owned".into())).unwrap();
    let token = cache.insert(captured).unwrap();
    assert_eq!(
        status(cache.page(
            &owner,
            "af_owned",
            &Some("foreign".into()),
            50,
            Cursor {
                snapshot: token.snapshot.clone(),
                position: token.position
            }
        )),
        StatusCode::BAD_REQUEST
    );
    let page = cache
        .page(
            &owner,
            "af_owned",
            &Some("af_owned".into()),
            50,
            Cursor {
                snapshot: token.snapshot.clone(),
                position: token.position,
            },
        )
        .unwrap();
    assert_eq!(page.root_type, "agentiflow");
    fs::write(dir.join("lead/transcript.r0.jsonl"), "{}\n").unwrap();
    assert_eq!(
        status(cache.page(&owner, "af_owned", &Some("af_owned".into()), 50, token)),
        StatusCode::CONFLICT
    );
}
#[test]
fn agentiflow_family_search_retains_verified_owner_and_child() {
    let (_tmp, owner, _) = fixture();
    let captured = capture::search::capture(
        owner,
        "af_owned",
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(captured.proof.kind, "agentiflow");
    assert_eq!(captured.proof.project_id.as_deref(), Some("ws_project"));
    assert_eq!(captured.members.len(), 2);
    assert_eq!(captured.members[0].run_id, "af_owned");
    assert_eq!(captured.members[1].run_id, "sub_actual");
    assert_eq!(captured.members[1].parent, Some(0));
}

#[cfg(unix)]
#[test]
fn agentiflow_root_refuses_symlinked_proof() {
    let (_tmp, owner, dir) = fixture();
    let path = dir.join("lead/transcript.r0.jsonl");
    let saved = dir.join("saved.jsonl");
    fs::rename(&path, &saved).unwrap();
    std::os::unix::fs::symlink(saved, path).unwrap();
    assert_eq!(
        status(capture::capture(owner, "af_owned", None)),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn agentiflow_root_preserves_archived_descendant_store_and_rejects_dual_ownership() {
    let (_tmp, owner, dir) = fixture();
    // Unsupported scratch sources beside the AF ledger must not add edges.
    fs::create_dir_all(dir.join("sub/sub_unrecorded")).unwrap();
    let archive = owner.runs.with_file_name("runs-archive").join("sub_actual");
    fs::create_dir_all(&archive).unwrap();
    ledger::append(
        &archive,
        &LedgerLine::Started(Started::new(
            "ch_nested",
            ChildKind::Agent,
            "nested",
            "run_nested",
            1,
            2,
        )),
    );
    let captured = capture::capture(owner.clone(), "af_owned", Some("sub_actual".into())).unwrap();
    assert_eq!(captured.rows.len(), 1);
    assert_eq!(captured.rows[0].run_id, "run_nested");
    assert_eq!(captured.rows[0].parent_run_id, "sub_actual");
    assert_eq!(captured.rows[0].round, Some(0));
    fs::create_dir_all(owner.runs.join("sub_actual")).unwrap();
    assert_eq!(
        status(capture::capture(owner, "af_owned", None)),
        StatusCode::CONFLICT
    );
}
