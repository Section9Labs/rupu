use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn fixture() -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("transcripts/archive")).unwrap();
    let state =
        AppState::new(root.clone(), rupu_config::PricingConfig::default()).with_workspace_dir(root);
    (tmp, state)
}
fn transcript(s: &AppState, id: &str, at: &str, pid: Option<u32>) {
    let root = s.global_dir.join("transcripts/archive");
    fs::write(
        root.join(format!("{id}.jsonl")),
        "private transcript contents, never exported\n",
    )
    .unwrap();
    let metadata = json!({"version":1,"run_id":id,"archived_at":at,"workspace_path":s.global_dir,"backend_id":"local_checkout","trigger_source":"run_cli","pid":pid});
    fs::write(root.join(format!("{id}.meta.json")), metadata.to_string()).unwrap();
}
fn body() -> PreviewBody {
    PreviewBody {
        kind: "transcripts".into(),
        scopes: Scopes {
            global: true,
            project_id: None,
        },
        older_than: Some("1d".into()),
        authorization_mode: None,
    }
}
fn prepared(s: &AppState) -> Prepared {
    let value = prepare(s, body()).unwrap();
    journal::load(&s.global_dir, value["operation_id"].as_str().unwrap()).unwrap()
}
fn intent(p: &Prepared) -> ExecuteBody {
    ExecuteBody {
        preview_id: p.preview_id.clone(),
        policy_digest: p.policy_digest.clone(),
        authorization_mode: p.policy.authorization_mode.clone(),
        preview_set_digest: p.preview_set_digest.clone(),
    }
}
fn old(s: &AppState, id: &str) {
    transcript(s, id, "2020-01-01T00:00:00Z", Some(2_147_483_647));
}
struct Fail {
    point: &'static str,
    nth: usize,
    seen: usize,
}
impl journal::Fault for Fail {
    fn check(&mut self, p: &str) -> ApiResult<()> {
        if p == self.point {
            self.seen += 1;
            if self.seen == self.nth {
                return Err(internal("injected disposable failure"));
            }
        }
        Ok(())
    }
}

#[test]
fn retention_policy_preview_is_non_destructive_and_exactly_scoped() {
    let (_tmp, s) = fixture();
    old(&s, "run_old");
    let p = prepared(&s);
    assert!(s
        .global_dir
        .join("transcripts/archive/run_old.jsonl")
        .exists());
    assert_eq!(p.preview.groups.len(), 1);
    assert_eq!(p.policy.roots.len(), 1);
    assert_eq!(p.policy.authorization_mode, "policy_at_execution");
    assert!(!serde_json::to_string(&p)
        .unwrap()
        .contains("private transcript contents"));
    assert!(journal::events(&p).unwrap().is_empty());
}
#[test]
fn retention_policy_selects_current_eligible_set_once_and_replay_never_deletes_again() {
    let (_tmp, s) = fixture();
    old(&s, "run_preview");
    let p = prepared(&s);
    old(&s, "run_added");
    let receipt = execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    assert_eq!(receipt["state"], "complete");
    assert_eq!(receipt["preview_group_count"], 1);
    assert_eq!(receipt["execution_group_count"], 2);
    assert_eq!(receipt["confirmed_unlinked_artifacts"], 4);
    old(&s, "run_later");
    let again = execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    assert_eq!(again["sequence"], receipt["sequence"]);
    assert!(s
        .global_dir
        .join("transcripts/archive/run_later.jsonl")
        .exists());
}
#[test]
fn retention_policy_fixed_cutoff_and_outside_scope_survive() {
    let (_tmp, s) = fixture();
    old(&s, "run_old");
    transcript(&s, "run_new", "2999-01-01T00:00:00Z", Some(2_147_483_647));
    fs::create_dir_all(s.global_dir.join("runs/run_old")).unwrap();
    fs::write(s.global_dir.join("runs/run_old/coverage.jsonl"), "retained").unwrap();
    let p = prepared(&s);
    let cutoff = p.policy.transcripts_before.clone();
    fs::write(
        s.global_dir.join("config.toml"),
        "[storage]\narchived_transcript_retention='0s'\n",
    )
    .unwrap();
    execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    assert!(s
        .global_dir
        .join("transcripts/archive/run_new.jsonl")
        .exists());
    assert!(s.global_dir.join("runs/run_old/coverage.jsonl").exists());
    assert_eq!(
        journal::load(&s.global_dir, &p.operation_id)
            .unwrap()
            .policy
            .transcripts_before,
        cutoff
    );
}
#[test]
fn retention_policy_live_unknown_session_owned_and_active_counterparts_are_kept() {
    let (_tmp, s) = fixture();
    transcript(
        &s,
        "run_live",
        "2020-01-01T00:00:00Z",
        Some(std::process::id()),
    );
    transcript(&s, "run_unknown", "2020-01-01T00:00:00Z", None);
    old(&s, "run_active");
    fs::write(s.global_dir.join("transcripts/run_active.jsonl"), "active").unwrap();
    old(&s, "run_owned");
    let path = s.global_dir.join("transcripts/archive/run_owned.meta.json");
    let mut v: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    v["session_id"] = "ses_other".into();
    fs::write(&path, v.to_string()).unwrap();
    let p = prepared(&s);
    assert_eq!(p.preview.groups.len(), 0);
    assert_eq!(p.preview.kept.len(), 3);
    execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    for id in ["run_live", "run_unknown", "run_active", "run_owned"] {
        assert!(s
            .global_dir
            .join(format!("transcripts/archive/{id}.jsonl"))
            .exists());
    }
}
#[test]
fn retention_policy_durable_admission_precedes_any_effect() {
    let (_tmp, s) = fixture();
    old(&s, "run_old");
    let p = prepared(&s);
    assert!(execute_once(
        &s,
        &p.operation_id,
        intent(&p),
        &mut Fail {
            point: "before_journal",
            nth: 1,
            seen: 0
        }
    )
    .is_err());
    assert!(journal::events(&p).unwrap().is_empty());
    assert!(s
        .global_dir
        .join("transcripts/archive/run_old.jsonl")
        .exists());
}
#[test]
fn retention_policy_crash_after_unlink_is_unknown_and_never_resumed() {
    let (_tmp, s) = fixture();
    old(&s, "run_old");
    let p = prepared(&s);
    assert!(execute_once(
        &s,
        &p.operation_id,
        intent(&p),
        &mut Fail {
            point: "after_unlink",
            nth: 1,
            seen: 0
        }
    )
    .is_err());
    let receipt = execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    assert_eq!(receipt["state"], "interrupted_unknown");
    assert!(s
        .global_dir
        .join("transcripts/archive/run_old.meta.json")
        .exists());
    let page = journal::answer(
        &p,
        ReadQuery {
            section: Some("effects".into()),
            ..Default::default()
        },
        true,
    )
    .unwrap();
    assert_eq!(page["rows"][0]["outcome"], "effect_unknown");
    assert_eq!(page["rows"][1]["outcome"], "not_attempted");
}
#[test]
fn retention_policy_partial_journal_failure_preserves_confirmed_prior_effect() {
    let (_tmp, s) = fixture();
    old(&s, "run_old");
    let p = prepared(&s);
    assert!(execute_once(
        &s,
        &p.operation_id,
        intent(&p),
        &mut Fail {
            point: "before_journal",
            nth: 5,
            seen: 0
        }
    )
    .is_err());
    let receipt = journal::answer(&p, ReadQuery::default(), true).unwrap();
    assert_eq!(receipt["confirmed_unlinked_artifacts"], 1);
    assert_eq!(receipt["state"], "interrupted_unknown");
    assert!(s
        .global_dir
        .join("transcripts/archive/run_old.meta.json")
        .exists());
}
#[test]
fn retention_policy_incomplete_execution_scan_refuses_before_effects() {
    let (_tmp, s) = fixture();
    old(&s, "run_old");
    let p = prepared(&s);
    fs::write(
        s.global_dir.join("transcripts/archive/run_old.meta.json"),
        vec![b'x'; 262145],
    )
    .unwrap();
    let receipt = execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    assert_eq!(receipt["state"], "refused");
    assert_eq!(receipt["confirmed_unlinked_artifacts"], 0);
    assert!(s
        .global_dir
        .join("transcripts/archive/run_old.jsonl")
        .exists());
}
#[test]
fn retention_policy_requires_exact_intent_and_cannot_enable_netflow() {
    let (_tmp, s) = fixture();
    let mut b = body();
    b.kind = "netflow".into();
    assert!(prepare(&s, b).is_err());
    let mut b = body();
    b.scopes.global = false;
    assert!(prepare(&s, b).is_err());
    let p = prepared(&s);
    let mut request = intent(&p);
    request.policy_digest = "foreign".into();
    assert!(execute_once(&s, &p.operation_id, request, &mut journal::NoFault).is_err());
    assert!(journal::events(&p).unwrap().is_empty());
}
#[test]
fn retention_policy_lease_blocks_concurrent_supported_archive_restore_before_dispatch() {
    let (_tmp, s) = fixture();
    old(&s, "run_old");
    let p = prepared(&s);
    let _held = lease(&s.global_dir).unwrap();
    let e = execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap_err();
    assert_eq!(e.0, StatusCode::TOO_MANY_REQUESTS);
    assert!(journal::events(&p).unwrap().is_empty());
}
#[test]
fn retention_policy_refuses_special_files_and_changed_artifacts() {
    let (_tmp, s) = fixture();
    old(&s, "run_old");
    let p = prepared(&s);
    let g = &p.preview.groups[0];
    fs::write(
        s.global_dir.join("transcripts/archive/run_old.jsonl"),
        "changed",
    )
    .unwrap();
    assert!(plan::verify_group(&s, &p.policy, g).is_err());
    let path = s.global_dir.join("transcripts/archive/run_old.meta.json");
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("/dev/null", path).unwrap();
    assert!(plan::select(&s, &p.policy).is_err());
}
#[test]
fn retention_policy_effect_pages_bind_operation_section_and_published_frontier() {
    let (_tmp, s) = fixture();
    old(&s, "run_one");
    old(&s, "run_two");
    let p = prepared(&s);
    let preview = journal::answer(
        &p,
        ReadQuery {
            section: Some("preview".into()),
            limit: Some(1),
            ..Default::default()
        },
        false,
    )
    .unwrap();
    let token = preview["next_cursor"].as_str().unwrap().to_string();
    execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    let page = journal::answer(
        &p,
        ReadQuery {
            section: Some("preview".into()),
            cursor: Some(token.clone()),
            limit: Some(1),
            ..Default::default()
        },
        true,
    )
    .unwrap();
    assert_eq!(page["sequence"], 0);
    assert!(page["execution_group_count"].is_null());
    assert!(journal::answer(
        &p,
        ReadQuery {
            section: Some("effects".into()),
            cursor: Some(token),
            ..Default::default()
        },
        true
    )
    .is_err());
}
#[tokio::test]
async fn retention_policy_http_readonly_refuses_new_audit_write_and_keeps_legacy_preview() {
    let (_tmp, s) = fixture();
    let app = crate::server::router(s, None);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/maintenance/retention/preview?host=local")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"kind":"transcripts","scopes":{"global":true},"older_than":"1d"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    let _ = response.into_body().collect().await.unwrap();
    let legacy = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/maintenance/prune?host=local")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"kind":"transcripts","older_than":"1d","dry_run":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(legacy.status(), StatusCode::OK);
}

fn archived_session(s: &AppState, id: &str) -> PathBuf {
    let dir = s.global_dir.join("sessions-archive").join(id);
    fs::create_dir_all(&dir).unwrap();
    let record = json!({"version":5,"session_id":id,"agent_name":"review","provider_name":"test","model":"test","agent_system_prompt":"private system prompt","max_turns":1,"permission_mode":"readonly","no_stream":false,"workspace_id":"ws_fixture","workspace_path":s.global_dir,"transcripts_dir":s.global_dir.join("transcripts"),"created_at":"2020-01-01T00:00:00Z","updated_at":"2020-01-01T00:00:00Z","status":"idle","runs":[]});
    fs::write(dir.join("session.json"), record.to_string()).unwrap();
    dir
}
#[test]
fn retention_policy_archived_session_declares_and_removes_only_owned_tree() {
    let (_tmp, s) = fixture();
    let dir = archived_session(&s, "ses_old");
    fs::write(dir.join("notes.txt"), "private notes").unwrap();
    let mut request = body();
    request.kind = "sessions".into();
    let response = prepare(&s, request).unwrap();
    let p = journal::load(&s.global_dir, response["operation_id"].as_str().unwrap()).unwrap();
    assert_eq!(p.preview.groups.len(), 1);
    assert_eq!(p.preview.groups[0].artifacts.len(), 3);
    let receipt = execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    assert_eq!(receipt["state"], "complete");
    assert!(!dir.exists());
    assert!(!serde_json::to_string(&p)
        .unwrap()
        .contains("private system prompt"));
}
#[test]
fn retention_policy_restored_session_is_excluded_from_fresh_execution() {
    let (_tmp, s) = fixture();
    let dir = archived_session(&s, "ses_old");
    let mut b = body();
    b.kind = "sessions".into();
    let response = prepare(&s, b).unwrap();
    let p = journal::load(&s.global_dir, response["operation_id"].as_str().unwrap()).unwrap();
    fs::create_dir_all(s.global_dir.join("sessions")).unwrap();
    fs::rename(&dir, s.global_dir.join("sessions/ses_old")).unwrap();
    let receipt = execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    assert_eq!(receipt["execution_group_count"], 0);
    assert_eq!(receipt["confirmed_unlinked_artifacts"], 0);
    assert!(s.global_dir.join("sessions/ses_old/session.json").exists());
}
#[test]
fn retention_policy_project_scope_rebind_refuses_without_deleting() {
    let (_tmp, s) = fixture();
    let project = tempfile::tempdir().unwrap();
    let path = project.path().canonicalize().unwrap();
    fs::create_dir_all(path.join(".rupu/transcripts/archive")).unwrap();
    fs::create_dir_all(s.global_dir.join("workspaces")).unwrap();
    let registry = s.global_dir.join("workspaces/ws_project.toml");
    fs::write(
        &registry,
        format!(
            "id='ws_project'\npath='{}'\ncreated_at='2020-01-01T00:00:00Z'\n",
            path.display()
        ),
    )
    .unwrap();
    let mut b = body();
    b.scopes = Scopes {
        global: false,
        project_id: Some("ws_project".into()),
    };
    let response = prepare(&s, b).unwrap();
    let p = journal::load(&s.global_dir, response["operation_id"].as_str().unwrap()).unwrap();
    fs::write(
        &registry,
        "id='foreign'\npath='/different'\ncreated_at='2020-01-01T00:00:00Z'\n",
    )
    .unwrap();
    let receipt = execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    assert_eq!(receipt["state"], "refused");
    assert_eq!(receipt["confirmed_unlinked_artifacts"], 0);
}
#[test]
fn retention_policy_corrupt_durable_tail_never_replays() {
    use std::io::Write;
    let (_tmp, s) = fixture();
    old(&s, "run_old");
    let p = prepared(&s);
    let path = s
        .global_dir
        .join("maintenance/retention")
        .join(&p.operation_id)
        .join("events.jsonl");
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(b"{incomplete").unwrap();
    assert!(execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).is_err());
    assert!(s
        .global_dir
        .join("transcripts/archive/run_old.jsonl")
        .exists());
}

#[test]
fn retention_policy_incomplete_tail_preserves_prior_confirmed_effect_receipt() {
    use std::io::Write;
    let (_tmp, s) = fixture();
    old(&s, "run_old");
    let p = prepared(&s);
    execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    let path = s
        .global_dir
        .join("maintenance/retention")
        .join(&p.operation_id)
        .join("events.jsonl");
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b"{partial")
        .unwrap();
    let receipt = journal::answer(&p, ReadQuery::default(), true).unwrap();
    assert_eq!(receipt["state"], "interrupted_unknown");
    assert_eq!(receipt["confirmed_unlinked_artifacts"], 2);
    assert_eq!(receipt["journal_complete"], false);
    assert_eq!(receipt["executable"], false);
}

#[test]
fn retention_policy_missing_jsonl_never_authorizes_foreign_sidecar_deletion() {
    let (_tmp, s) = fixture();
    let dir = archived_session(&s, "ses_old");
    let path = dir.join("session.json");
    let mut v: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    v["runs"] = json!([{"run_id":"run_owned","prompt":"","transcript_path":s.global_dir.join("transcripts/archive/run_owned.jsonl"),"started_at":"2020-01-01T00:00:00Z","completed_at":"2020-01-01T00:00:01Z","status":"ok","duration_ms":1}]);
    fs::write(&path, v.to_string()).unwrap();
    old(&s, "run_owned");
    fs::remove_file(s.global_dir.join("transcripts/archive/run_owned.jsonl")).unwrap();
    let sidecar = s.global_dir.join("transcripts/archive/run_owned.meta.json");
    let mut m: Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
    m["session_id"] = "ses_foreign".into();
    fs::write(&sidecar, m.to_string()).unwrap();
    let mut request = body();
    request.kind = "sessions".into();
    let response = prepare(&s, request).unwrap();
    let p = journal::load(&s.global_dir, response["operation_id"].as_str().unwrap()).unwrap();
    assert_eq!(p.preview.groups.len(), 0);
    assert_eq!(p.preview.kept.len(), 1);
    execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    assert!(sidecar.exists());
    assert!(path.exists());
}

#[test]
fn retention_policy_matching_session_sidecar_requires_positive_dead_process() {
    for pid in [Some(std::process::id()), None] {
        let (_tmp, s) = fixture();
        let dir = archived_session(&s, "ses_old");
        let path = dir.join("session.json");
        let mut v: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        v["runs"] = json!([{"run_id":"run_owned","prompt":"","transcript_path":s.global_dir.join("transcripts/archive/run_owned.jsonl"),"started_at":"2020-01-01T00:00:00Z","completed_at":"2020-01-01T00:00:01Z","status":"ok","duration_ms":1}]);
        fs::write(&path, v.to_string()).unwrap();
        transcript(&s, "run_owned", "2020-01-01T00:00:00Z", pid);
        let sidecar = s.global_dir.join("transcripts/archive/run_owned.meta.json");
        let mut m: Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
        m["session_id"] = "ses_old".into();
        fs::write(&sidecar, m.to_string()).unwrap();
        let mut request = body();
        request.kind = "sessions".into();
        let response = prepare(&s, request).unwrap();
        let p = journal::load(&s.global_dir, response["operation_id"].as_str().unwrap()).unwrap();
        assert!(p.preview.groups.is_empty());
        assert_eq!(p.preview.kept.len(), 1);
        execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
        assert!(sidecar.exists());
        assert!(path.exists());
    }
}

fn ledger(s: &AppState, id: &str) -> PathBuf {
    old(s, id);
    fs::create_dir_all(s.global_dir.join("netflow")).unwrap();
    let path = s.global_dir.join("netflow").join(format!("{id}.jsonl"));
    fs::write(&path, "fixture ledger, no real traffic\n").unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_577_836_800))
        .unwrap();
    path
}
fn ledger_prepared(s: &AppState) -> Prepared {
    let mut b = body();
    b.kind = "netflow".into();
    b.authorization_mode = Some("exact_preview_set".into());
    let answer = prepare(s, b).unwrap();
    journal::load(&s.global_dir, answer["operation_id"].as_str().unwrap()).unwrap()
}
#[test]
fn reviewed_ledgers_never_add_new_candidates_or_remove_transcript_evidence() {
    let (_tmp, s) = fixture();
    let selected = ledger(&s, "run_selected");
    let p = ledger_prepared(&s);
    let later = ledger(&s, "run_later");
    assert_eq!(p.preview.groups.len(), 1);
    assert!(p.preview_set_digest.is_some());
    let result = execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    assert_eq!(result["authorization_mode"], "exact_preview_set");
    assert_eq!(result["preview_selection"], "exact_reviewed_candidate_set");
    assert_eq!(result["confirmed_unlinked_artifacts"], 1);
    assert!(!selected.exists());
    assert!(later.exists());
    assert!(s
        .global_dir
        .join("transcripts/archive/run_selected.jsonl")
        .exists());
    assert!(s
        .global_dir
        .join("transcripts/archive/run_selected.meta.json")
        .exists());
    fs::write(&selected, "new incarnation").unwrap();
    let replay = execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    assert_eq!(replay["sequence"], result["sequence"]);
    assert!(selected.exists());
}
#[test]
fn reviewed_ledgers_changed_replaced_or_newly_active_are_kept() {
    for change in ["append", "replace", "active", "metadata", "symlink"] {
        let (_tmp, s) = fixture();
        let path = ledger(&s, "run_changed");
        let p = ledger_prepared(&s);
        match change {
            "append" => {
                use std::io::Write;
                fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap()
                    .write_all(b"extra")
                    .unwrap();
            }
            "replace" => {
                fs::remove_file(&path).unwrap();
                fs::write(&path, "replacement").unwrap();
            }
            "active" => {
                fs::write(s.global_dir.join("transcripts/run_changed.jsonl"), "active").unwrap()
            }
            "metadata" => {
                let meta = s
                    .global_dir
                    .join("transcripts/archive/run_changed.meta.json");
                let mut raw: Value = serde_json::from_slice(&fs::read(&meta).unwrap()).unwrap();
                raw["pid"] = json!(std::process::id());
                fs::write(meta, raw.to_string()).unwrap();
            }
            _ => {
                fs::remove_file(&path).unwrap();
                std::os::unix::fs::symlink(
                    s.global_dir.join("transcripts/archive/run_changed.jsonl"),
                    &path,
                )
                .unwrap();
            }
        }
        let result = execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
        assert_eq!(result["state"], "partial", "{change}");
        assert_eq!(result["confirmed_unlinked_artifacts"], 0);
        assert!(fs::symlink_metadata(path).is_ok());
    }
}
#[test]
fn reviewed_ledgers_unknown_owners_and_legacy_shared_ledgers_never_become_candidates() {
    for change in [
        "worker",
        "session",
        "unknown_pid",
        "live_pid",
        "backend",
        "missing_meta",
        "hardlink",
        "workflow",
    ] {
        let (_tmp, s) = fixture();
        let path = ledger(&s, "run_unsupported");
        let meta = s
            .global_dir
            .join("transcripts/archive/run_unsupported.meta.json");
        let mut raw: Value = serde_json::from_slice(&fs::read(&meta).unwrap()).unwrap();
        match change {
            "worker" => raw["worker_id"] = json!("worker_elsewhere"),
            "session" => raw["session_id"] = json!("ses_fixture"),
            "unknown_pid" => raw["pid"] = Value::Null,
            "live_pid" => raw["pid"] = json!(std::process::id()),
            "backend" => raw["backend_id"] = json!("remote"),
            "hardlink" => fs::hard_link(&path, s.global_dir.join("linked-ledger")).unwrap(),
            "workflow" => fs::create_dir_all(s.global_dir.join("runs/run_unsupported")).unwrap(),
            _ => (),
        }
        fs::write(&meta, raw.to_string()).unwrap();
        if change == "missing_meta" {
            fs::remove_file(meta).unwrap();
        }
        fs::write(s.global_dir.join("netflow/flows.jsonl"), "legacy shared").unwrap();
        let p = ledger_prepared(&s);
        assert!(p.preview.groups.is_empty(), "{change}");
        assert_eq!(p.preview.kept.len(), 2);
        assert!(path.exists());
    }
}
#[test]
fn reviewed_ledgers_exact_intent_and_expiration_fail_before_admission() {
    let (_tmp, s) = fixture();
    let path = ledger(&s, "run_exact");
    let mut p = ledger_prepared(&s);
    let mut wrong = intent(&p);
    wrong.preview_set_digest = Some("0".repeat(64));
    assert!(execute_once(&s, &p.operation_id, wrong, &mut journal::NoFault).is_err());
    let mut wrong = intent(&p);
    wrong.authorization_mode = "policy_at_execution".into();
    assert!(execute_once(&s, &p.operation_id, wrong, &mut journal::NoFault).is_err());
    assert!(journal::events(&p).unwrap().is_empty());
    p.policy.expires_at = Utc::now() - chrono::Duration::seconds(1);
    p.policy_digest = digest(&serde_json::to_vec(&p.policy).unwrap());
    let stored = s
        .global_dir
        .join("maintenance/retention")
        .join(&p.operation_id)
        .join("prepared.json");
    disk::atomic(&stored, &serde_json::to_vec(&p).unwrap()).unwrap();
    assert!(execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).is_err());
    assert!(path.exists() && journal::events(&p).unwrap().is_empty());
}
#[test]
fn reviewed_ledgers_lost_post_unlink_receipt_is_unknown_and_never_replayed() {
    let (_tmp, s) = fixture();
    let path = ledger(&s, "run_lost");
    let p = ledger_prepared(&s);
    let mut fault = Fail {
        point: "after_unlink",
        nth: 1,
        seen: 0,
    };
    assert!(execute_once(&s, &p.operation_id, intent(&p), &mut fault).is_err());
    assert!(!path.exists());
    fs::write(&path, "new ledger after interrupted attempt").unwrap();
    let reconciled = execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    assert_eq!(reconciled["state"], "interrupted_unknown");
    assert_eq!(reconciled["confirmed_unlinked_artifacts"], 0);
    assert!(path.exists());
}
#[test]
fn reviewed_ledgers_refuse_symlinked_scope_and_nonpositive_age() {
    let (_tmp, s) = fixture();
    ledger(&s, "run_exact");
    let mut b = body();
    b.kind = "netflow".into();
    b.authorization_mode = Some("exact_preview_set".into());
    b.older_than = Some("0s".into());
    assert!(prepare(&s, b).is_err());
    let original = s.global_dir.join("netflow");
    let moved = s.global_dir.join("other-netflow");
    fs::rename(&original, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &original).unwrap();
    let mut b = body();
    b.kind = "netflow".into();
    b.authorization_mode = Some("exact_preview_set".into());
    assert!(prepare(&s, b).is_err());
    assert!(moved.join("run_exact.jsonl").exists());
}
#[test]
fn reviewed_ledgers_project_selection_and_changed_registration_stay_exact() {
    let (_tmp, s) = fixture();
    let outside = ledger(&s, "run_global");
    let project = tempfile::tempdir().unwrap();
    let project = project.path().canonicalize().unwrap();
    let local = project.join(".rupu");
    fs::create_dir_all(local.join("transcripts/archive")).unwrap();
    let local_state = AppState::new(local.clone(), rupu_config::PricingConfig::default());
    let selected = ledger(&local_state, "run_project");
    let meta = local.join("transcripts/archive/run_project.meta.json");
    let mut raw: Value = serde_json::from_slice(&fs::read(&meta).unwrap()).unwrap();
    raw["project_root"] = json!(project);
    fs::write(&meta, raw.to_string()).unwrap();
    fs::create_dir_all(s.global_dir.join("workspaces")).unwrap();
    let registry = s.global_dir.join("workspaces/ws_project.toml");
    fs::write(
        &registry,
        format!(
            "id='ws_project'\npath='{}'\ncreated_at='2020-01-01T00:00:00Z'\n",
            project.display()
        ),
    )
    .unwrap();
    let mut b = body();
    b.kind = "netflow".into();
    b.authorization_mode = Some("exact_preview_set".into());
    b.scopes = Scopes {
        global: false,
        project_id: Some("ws_project".into()),
    };
    let result = prepare(&s, b).unwrap();
    let p = journal::load(&s.global_dir, result["operation_id"].as_str().unwrap()).unwrap();
    assert_eq!(p.preview.groups.len(), 1);
    assert_eq!(p.preview.groups[0].id, "run_project");
    fs::write(
        &registry,
        "id='ws_project'\npath='/changed'\ncreated_at='2020-01-01T00:00:00Z'\n",
    )
    .unwrap();
    let result = execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    assert_eq!(result["state"], "refused");
    assert!(selected.exists() && outside.exists());
}
#[test]
fn reviewed_ledgers_concurrent_admission_has_one_dispatch() {
    let (_tmp, s) = fixture();
    let path = ledger(&s, "run_concurrent");
    let p = ledger_prepared(&s);
    struct Gate {
        entered: std::sync::mpsc::SyncSender<()>,
        release: std::sync::mpsc::Receiver<()>,
        once: bool,
    }
    impl journal::Fault for Gate {
        fn check(&mut self, point: &str) -> ApiResult<()> {
            if point == "before_journal" && !self.once {
                self.once = true;
                self.entered.send(()).map_err(internal)?;
                self.release
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .map_err(internal)?;
            }
            Ok(())
        }
    }
    let (enter, entered) = std::sync::mpsc::sync_channel(1);
    let (release, released) = std::sync::mpsc::sync_channel(1);
    std::thread::scope(|threads| {
        let worker_state = &s;
        let worker_prepared = &p;
        let worker = threads.spawn(move || {
            execute_once(
                worker_state,
                &worker_prepared.operation_id,
                intent(worker_prepared),
                &mut Gate {
                    entered: enter,
                    release: released,
                    once: false,
                },
            )
        });
        entered
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let other =
            execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap_err();
        assert_eq!(other.0, StatusCode::TOO_MANY_REQUESTS);
        release.send(()).unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap()["confirmed_unlinked_artifacts"],
            1
        );
    });
    assert!(!path.exists());
    assert_eq!(
        journal::events(&p)
            .unwrap()
            .iter()
            .filter(|e| e["outcome"] == "dispatching")
            .count(),
        1
    );
}
#[tokio::test]
async fn reviewed_ledgers_http_requires_local_writable_scope() {
    let (_tmp, s) = fixture();
    let path = ledger(&s, "run_protected");
    let app = crate::server::router(s, None);
    for (host, status) in [
        ("local", StatusCode::NOT_IMPLEMENTED),
        ("remote", StatusCode::NOT_IMPLEMENTED),
    ] {
        let response = app.clone().oneshot(Request::builder().method("POST")
            .uri(format!("/api/maintenance/retention/preview?host={host}"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"kind":"netflow","authorization_mode":"exact_preview_set","scopes":{"global":true},"older_than":"1d"}"#)).unwrap()).await.unwrap();
        assert_eq!(response.status(), status);
        assert!(path.exists());
    }
}

fn current_cli_ledger(s: &AppState, id: &str) -> (PathBuf, PathBuf) {
    let path = ledger(s, id);
    let metadata = s
        .global_dir
        .join("transcripts/archive")
        .join(format!("{id}.meta.json"));
    let mut raw: Value = serde_json::from_slice(&fs::read(&metadata).unwrap()).unwrap();
    raw["worker_id"] = json!("worker_fixture_cli");
    fs::write(&metadata, raw.to_string()).unwrap();
    let worker = rupu_runtime::WorkerRecord {
        version: rupu_runtime::WorkerRecord::VERSION,
        worker_id: "worker_fixture_cli".into(),
        kind: rupu_runtime::WorkerKind::Cli,
        name: "fixture".into(),
        host: whoami::fallible::hostname().unwrap(),
        capabilities: rupu_runtime::WorkerCapabilities {
            backends: vec!["local_checkout".into()],
            ..Default::default()
        },
        registered_at: "2020-01-01T00:00:00Z".into(),
        last_seen_at: "2020-01-01T00:00:00Z".into(),
    };
    let record = s
        .global_dir
        .join("autoflows/workers/worker_fixture_cli.toml");
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    fs::write(&record, toml::to_string(&worker).unwrap()).unwrap();
    (path, record)
}
#[test]
fn reviewed_ledgers_include_exact_current_cli_worker_but_preserve_owner_evidence() {
    let (_tmp, s) = fixture();
    let (path, record) = current_cli_ledger(&s, "run_cli_fixture");
    let p = ledger_prepared(&s);
    assert_eq!(p.preview.groups.len(), 1);
    assert!(p.preview.groups[0]
        .checks
        .iter()
        .any(|v| v.path == record && v.digest.is_some()));
    let result = execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    assert_eq!(result["confirmed_unlinked_artifacts"], 1);
    assert!(!path.exists());
    assert!(record.exists());
    assert!(s
        .global_dir
        .join("transcripts/archive/run_cli_fixture.meta.json")
        .exists());
    assert!(s
        .global_dir
        .join("transcripts/archive/run_cli_fixture.jsonl")
        .exists());
}
#[test]
fn reviewed_ledgers_unproven_or_conflicting_workers_stay_kept() {
    for kind in [
        "hostname",
        "kind",
        "capability",
        "identity",
        "version",
        "missing",
        "host_collision",
        "symlink",
        "oversized",
    ] {
        let (_tmp, s) = fixture();
        let (path, record) = current_cli_ledger(&s, "run_worker_fixture");
        let mut worker: rupu_runtime::WorkerRecord =
            toml::from_str(&fs::read_to_string(&record).unwrap()).unwrap();
        match kind {
            "hostname" => worker.host.push_str("-different-host"),
            "kind" => worker.kind = rupu_runtime::WorkerKind::AutoflowServe,
            "capability" => worker.capabilities.backends.clear(),
            "identity" => worker.worker_id = "worker_other".into(),
            "version" => worker.version += 1,
            _ => (),
        }
        fs::write(&record, toml::to_string(&worker).unwrap()).unwrap();
        match kind {
            "missing" => fs::remove_file(&record).unwrap(),
            "host_collision" => {
                fs::create_dir_all(s.global_dir.join("hosts")).unwrap();
                fs::write(
                    s.global_dir.join("hosts/worker_fixture_cli.toml"),
                    "not a local worker",
                )
                .unwrap();
            }
            "symlink" => {
                let other = s.global_dir.join("worker-copy");
                fs::rename(&record, &other).unwrap();
                std::os::unix::fs::symlink(other, &record).unwrap();
            }
            "oversized" => fs::write(&record, vec![b'x'; 65537]).unwrap(),
            _ => (),
        }
        if kind == "oversized" {
            let mut b = body();
            b.kind = "netflow".into();
            b.authorization_mode = Some("exact_preview_set".into());
            assert!(prepare(&s, b).is_err());
        } else {
            let p = ledger_prepared(&s);
            assert!(p.preview.groups.is_empty(), "{kind}");
        }
        assert!(path.exists());
    }
}
#[test]
fn reviewed_ledgers_writer_contention_and_changed_worker_never_delete() {
    use rupu_netflow::ledger::lease::LedgerLease;
    for kind in ["busy", "worker_changed", "host_collision"] {
        let (_tmp, s) = fixture();
        let (path, record) = current_cli_ledger(&s, "run_busy_fixture");
        let p = ledger_prepared(&s);
        assert_eq!(p.preview.groups.len(), 1);
        let held = if kind == "busy" {
            Some(LedgerLease::shared(&path).unwrap())
        } else {
            None
        };
        if kind == "worker_changed" {
            let mut raw = fs::read_to_string(&record).unwrap();
            raw.push_str("\n# refreshed registry\n");
            fs::write(&record, raw).unwrap();
        }
        if kind == "host_collision" {
            fs::create_dir_all(s.global_dir.join("hosts")).unwrap();
            fs::write(
                s.global_dir.join("hosts/worker_fixture_cli.toml"),
                "conflict",
            )
            .unwrap();
        }
        let result = execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
        assert_eq!(result["state"], "partial", "{kind}");
        assert_eq!(result["confirmed_unlinked_artifacts"], 0);
        assert!(path.exists());
        drop(held);
    }
    let (_tmp, s) = fixture();
    let (path, _) = current_cli_ledger(&s, "run_preview_busy");
    let _held = LedgerLease::shared(&path).unwrap();
    let p = ledger_prepared(&s);
    assert!(p.preview.groups.is_empty());
}

#[test]
fn reviewed_ledgers_unchanged_registration_alias_retarget_has_zero_effects() {
    let (_tmp, s) = fixture();
    let first = s.global_dir.join("checkout-a");
    let second = s.global_dir.join("checkout-b");
    fs::create_dir_all(first.join(".rupu/transcripts/archive")).unwrap();
    fs::create_dir_all(second.join(".rupu")).unwrap();
    let local = AppState::new(first.join(".rupu"), rupu_config::PricingConfig::default());
    let selected = ledger(&local, "run_alias_fixture");
    let meta = first.join(".rupu/transcripts/archive/run_alias_fixture.meta.json");
    let mut raw: Value = serde_json::from_slice(&fs::read(&meta).unwrap()).unwrap();
    raw["project_root"] = json!(first);
    fs::write(meta, raw.to_string()).unwrap();
    let alias = s.global_dir.join("registered-checkout");
    std::os::unix::fs::symlink(&first, &alias).unwrap();
    fs::create_dir_all(s.global_dir.join("workspaces")).unwrap();
    let registry = s.global_dir.join("workspaces/ws_alias.toml");
    let exact = format!(
        "id='ws_alias'\npath='{}'\ncreated_at='2020-01-01T00:00:00Z'\n",
        alias.display()
    );
    fs::write(&registry, &exact).unwrap();
    let mut b = body();
    b.kind = "netflow".into();
    b.authorization_mode = Some("exact_preview_set".into());
    b.scopes = Scopes {
        global: false,
        project_id: Some("ws_alias".into()),
    };
    let reply = prepare(&s, b).unwrap();
    let p = journal::load(&s.global_dir, reply["operation_id"].as_str().unwrap()).unwrap();
    assert_eq!(p.preview.groups.len(), 1);
    fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(&second, &alias).unwrap();
    assert_eq!(fs::read_to_string(registry).unwrap(), exact);
    let reply = execute_once(&s, &p.operation_id, intent(&p), &mut journal::NoFault).unwrap();
    assert_eq!(reply["state"], "refused");
    assert_eq!(reply["confirmed_unlinked_artifacts"], 0);
    assert!(selected.exists());
}
