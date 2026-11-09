use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use rupu_launch::ledger::{self, LedgerLine, Started};
use rupu_orchestrator::runs::RunRecord;
use rupu_tools::ChildKind;
use serde_json::{json, Value};
use std::{collections::BTreeSet, fs, io::Write};
use tower::ServiceExt;

fn fixture(count: usize) -> (tempfile::TempDir, AppState, Owner) {
    let tmp = tempfile::tempdir().unwrap();
    let state = AppState::new(
        tmp.path().canonicalize().unwrap(),
        rupu_config::PricingConfig::default(),
    );
    let record: RunRecord = serde_json::from_value(json!({
        "id":"run_root", "workflow_name":"example", "status":"completed", "inputs":{},
        "workspace_id":"ws_fixture", "workspace_path":"/fixture", "transcript_dir":"/fixture",
        "started_at":"2026-11-06T10:00:00Z"
    }))
    .unwrap();
    state
        .run_store
        .create(record, "name: example\nsteps: []\n")
        .unwrap();
    let owner = Owner {
        global: tmp.path().canonicalize().unwrap(),
        runs: state.run_store.root.canonicalize().unwrap(),
    };
    let mut file = fs::File::create(owner.runs.join("run_root/children.jsonl")).unwrap();
    for i in 0..count {
        let entry = LedgerLine::Started(Started::new(
            &format!("h_{i}"),
            ChildKind::Agent,
            "review",
            &format!("run_{i}"),
            1,
            1,
        ));
        serde_json::to_writer(&mut file, &entry).unwrap();
        file.write_all(b"\n").unwrap();
    }
    (tmp, state, owner)
}
fn begin(cache: &mut Cache, owner: &Owner, parent: Option<String>) -> Page {
    let snapshot = capture::capture(owner.clone(), "run_root", parent.clone()).unwrap();
    let cursor = cache.insert(snapshot).unwrap();
    cache.page(owner, "run_root", &parent, 73, cursor).unwrap()
}
fn status<T>(result: ApiResult<T>) -> StatusCode {
    match result {
        Err(e) => e.0,
        Ok(_) => panic!("expected error"),
    }
}
fn append(owner: &Owner, run: &str, handle: &str, child: &str) {
    let dir = owner.runs.join(run);
    fs::create_dir_all(&dir).unwrap();
    ledger::append(
        &dir,
        &LedgerLine::Started(Started::new(
            handle,
            ChildKind::Agent,
            "review",
            child,
            1,
            1,
        )),
    );
}

#[test]
fn children_pages_visit_every_one_of_five_thousand_children_without_rescan() {
    let (_tmp, _state, owner) = fixture(5000);
    let mut cache = Cache::default();
    let first = begin(&mut cache, &owner, None);
    assert_eq!(first.snapshot_matching_total, 5000);
    assert_eq!(first.evidence.run_directories_scanned, 5001);
    assert_eq!(first.evidence.history_complete, "unknown");
    let snapshot = first.snapshot.clone();
    let source_bytes = first.evidence.bytes_read;
    let allocation = cache.snapshots[0].rows.as_ptr();
    append(&owner, "run_root", "later", "run_later");
    // Captured pages also remain fixed when old sources are rewritten.
    fs::write(owner.runs.join("run_root/children.jsonl"), b"rewritten\n").unwrap();
    let mut observed: Vec<_> = first.children.into_iter().map(|r| r.run_id).collect();
    let mut next = first.next_cursor;
    while let Some(token) = next {
        let page = cache
            .page(&owner, "run_root", &None, 73, cache.decode(&token).unwrap())
            .unwrap();
        assert_eq!(page.snapshot, snapshot);
        assert_eq!(page.evidence.bytes_read, source_bytes);
        observed.extend(page.children.into_iter().map(|r| r.run_id));
        next = page.next_cursor;
    }
    assert_eq!(
        observed,
        (0..5000).map(|i| format!("run_{i}")).collect::<Vec<_>>()
    );
    assert_eq!(observed.iter().collect::<BTreeSet<_>>().len(), 5000);
    assert_eq!(cache.snapshots[0].rows.as_ptr(), allocation);
    let refresh = begin(&mut cache, &owner, None);
    assert_ne!(refresh.snapshot, snapshot);
    assert_eq!(refresh.evidence.malformed_rows, 1);
    assert_eq!(refresh.snapshot_matching_total, 0);
}

#[test]
fn children_pages_bind_mac_owner_root_and_exact_parent_filter() {
    let (_tmp, _state, owner) = fixture(100);
    let (_other, _other_state, foreign) = fixture(0);
    let mut cache = Cache::default();
    let first = begin(&mut cache, &owner, Some("run_root".into()));
    let token = first.next_cursor.unwrap();
    for (o, root, parent) in [
        (&owner, "run_root", None),
        (&owner, "run_other", Some("run_root".into())),
        (&foreign, "run_root", Some("run_root".into())),
    ] {
        assert_eq!(
            status(cache.page(o, root, &parent, 10, cache.decode(&token).unwrap())),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        status(cache.decode(&(token + "x"))),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status(capture::capture(
            owner.clone(),
            "run_root",
            Some("run_foreign".into())
        )),
        StatusCode::NOT_FOUND
    );
    append(&owner, "run_0", "nested", "sub_nested");
    let nested = begin(&mut cache, &owner, Some("run_0".into()));
    assert_eq!(nested.children.len(), 1);
    assert_eq!(nested.children[0].run_id, "sub_nested");
    assert_eq!(nested.children[0].parent_run_id, "run_0");
}

#[test]
fn children_snapshot_expiry_eviction_and_root_replacement_are_explicit() {
    let (_tmp, _state, owner) = fixture(100);
    let mut cache = Cache::default();
    let first = begin(&mut cache, &owner, None);
    let token = first.next_cursor.unwrap();
    cache.snapshots[0].created = Instant::now() - TTL;
    assert_eq!(
        status(cache.page(&owner, "run_root", &None, 1, cache.decode(&token).unwrap())),
        StatusCode::CONFLICT
    );
    let fresh = begin(&mut cache, &owner, None);
    assert_ne!(first.snapshot, fresh.snapshot);
    assert_eq!(
        status(cache.page(&owner, "run_root", &None, 1, cache.decode(&token).unwrap())),
        StatusCode::CONFLICT
    );
    let token = fresh.next_cursor.unwrap();
    let root = owner.runs.join("run_root");
    fs::rename(&root, owner.runs.join("moved")).unwrap();
    fs::create_dir(&root).unwrap();
    fs::copy(owner.runs.join("moved/run.json"), root.join("run.json")).unwrap();
    assert_eq!(
        status(cache.page(&owner, "run_root", &None, 1, cache.decode(&token).unwrap())),
        StatusCode::CONFLICT
    );
}

#[test]
fn children_cache_enforces_global_bytes_count_and_unique_ids_after_eviction() {
    let (_tmp, _state, owner) = fixture(1);
    let mut cache = Cache::default();
    let mut ids = BTreeSet::new();
    for _ in 0..MAX_SNAPSHOTS + 3 {
        let mut s = capture::capture(owner.clone(), "run_root", None).unwrap();
        s.weight = MAX_SNAPSHOT_BYTES; // Exercise accounting without allocating 64 MiB.
        let cursor = cache.insert(s).unwrap();
        assert!(ids.insert(cursor.snapshot));
        assert!(cache.snapshots.len() <= 4);
        assert!(cache.snapshots.iter().map(|s| s.weight).sum::<usize>() <= MAX_CACHE_BYTES);
    }
    cache.snapshots.clear();
    for _ in 0..MAX_SNAPSHOTS + 3 {
        begin(&mut cache, &owner, None);
    }
    assert_eq!(cache.snapshots.len(), MAX_SNAPSHOTS);
}

#[test]
fn children_capture_reports_malformed_and_partial_sources_without_claiming_history() {
    let (_tmp, _state, owner) = fixture(2);
    fs::OpenOptions::new()
        .append(true)
        .open(owner.runs.join("run_root/children.jsonl"))
        .unwrap()
        .write_all(b"bad row\n{\"unfinished\":")
        .unwrap();
    let snapshot = capture::capture(owner, "run_root", None).unwrap();
    assert_eq!(snapshot.rows.len(), 2);
    assert_eq!(snapshot.evidence.malformed_rows, 1);
    assert!(snapshot.evidence.incomplete_tail_bytes > 0);
    assert_eq!(snapshot.evidence.history_complete, "unknown");
}

#[test]
fn children_capture_rejects_missing_or_mismatched_root_and_unsafe_recorded_edges() {
    let (_tmp, _state, owner) = fixture(0);
    assert_eq!(
        status(capture::capture(owner.clone(), "run_unknown", None)),
        StatusCode::NOT_FOUND
    );
    append(&owner, "run_unowned", "h", "run_child");
    assert_eq!(
        status(capture::capture(owner.clone(), "run_unowned", None)),
        StatusCode::NOT_IMPLEMENTED
    );
    append(&owner, "run_root", "h", "../foreign");
    assert_eq!(
        status(capture::capture(owner.clone(), "run_root", None)),
        StatusCode::BAD_REQUEST
    );
    let path = owner.runs.join("run_root/run.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["id"] = json!("run_wrong");
    fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(
        status(capture::capture(owner, "run_root", None)),
        StatusCode::CONFLICT
    );
}

#[test]
fn children_capture_enforces_file_and_line_budgets_without_partial_result() {
    let (_tmp, _state, owner) = fixture(1);
    let path = owner.runs.join("run_root/children.jsonl");
    fs::File::create(&path)
        .unwrap()
        .set_len(16 * 1024 * 1024 + 1)
        .unwrap();
    assert_eq!(
        status(capture::capture(owner.clone(), "run_root", None)),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let mut oversized = vec![b'x'; 256 * 1024 + 1];
    oversized.push(b'\n');
    fs::write(&path, oversized).unwrap();
    assert_eq!(
        status(capture::capture(owner, "run_root", None)),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[cfg(unix)]
#[test]
fn children_capture_rejects_symlinked_membership_even_inside_same_store() {
    let (_tmp, _state, owner) = fixture(1);
    fs::create_dir(owner.runs.join("run_foreign")).unwrap();
    std::os::unix::fs::symlink(owner.runs.join("run_foreign"), owner.runs.join("run_0")).unwrap();
    assert_eq!(
        status(capture::capture(owner, "run_root", None)),
        StatusCode::BAD_REQUEST
    );
}

async fn request(state: &AppState, uri: &str) -> (StatusCode, Value) {
    let response = super::super::routes()
        .merge(crate::api::host_info::routes())
        .with_state(state.clone())
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn children_http_legacy_compatibility_local_owner_gate_and_typed_query() {
    let (_tmp, state, _owner) = fixture(2);
    let (status, legacy) = request(&state, "/api/runs/run_root/children").await;
    assert_eq!(status, StatusCode::OK);
    assert!(legacy.get("snapshot").is_none());
    assert!(legacy.get("truncated").is_none());
    let (status, page) = request(&state, "/api/runs/run_root/children?limit=1&host=local").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["snapshot_matching_total"], 2);
    assert_eq!(page["children"][0], legacy["children"][0]);
    assert_eq!(page["owner_host_id"], "local");
    let uri = format!(
        "/api/runs/run_root/children?cursor={}",
        page["next_cursor"].as_str().unwrap()
    );
    let (status, tail) = request(&state, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tail["children"][0], legacy["children"][1]);
    assert_eq!(
        request(&state, "/api/runs/run_root/children?host=remote")
            .await
            .0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        request(&state, "/api/runs/run_root/children?host=remote&limit=1")
            .await
            .0,
        StatusCode::NOT_IMPLEMENTED
    );
    let schema = serde_json::to_value(schemars::schema_for!(ChildrenQuery)).unwrap();
    for key in ["host", "limit", "cursor", "parent_run_id"] {
        assert!(schema["properties"].get(key).is_some(), "{key}");
    }
    let (_, host) = request(&state, "/api/host/info").await;
    assert!(host["features"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f == super::super::CAPABILITY));
    assert_eq!(
        host["children_paging_root_types"],
        json!(["recorded_run", "standalone_agent", "session_turn"])
    );
    append(&_owner, "run_unowned", "orphan", "sub_orphan");
    let (status, unsupported) = request(&state, "/api/runs/run_unowned/children?limit=50").await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(unsupported["code"], "children_root_unsupported");
    assert!(unsupported["supported_root_types"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "session_turn"));
    let (status, unknown) = request(&state, "/api/runs/run_unknown/children?limit=50").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(unknown.get("code").is_none());
    ownership_metadata(&_owner, "run_session", Some("ses_owner"));
    session_ownership(&_owner, "ses_owner", "run_session");
    let (status, session) = request(
        &state,
        "/api/runs/run_session/children?limit=50&parent_run_id=run_session",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{session}");
    assert_eq!(session["root_type"], "session_turn");
    assert_eq!(session["snapshot_matching_total"], 0);
    assert!(state.launcher.is_none());
}

#[test]
fn children_canonical_latest_handle_parent_and_legacy_shape_match_existing_catalog() {
    let (_tmp, state, owner) = fixture(0);
    let step = |run: &str| json!({"step_id":"review", "run_id":run, "transcript_path":"/fixture", "output":"", "success":true,"skipped":false,"rendered_prompt":"", "finished_at":"2026-11-06T10:00:00Z"});
    fs::write(
        owner.runs.join("run_root/step_results.jsonl"),
        format!("{}\n{}\n", step("run_first"), step("run_second")),
    )
    .unwrap();
    append(&owner, "run_first", "same_handle", "run_old");
    append(&owner, "run_second", "same_handle", "run_new");
    append(&owner, "run_new", "nested_handle", "run_nested");
    fs::create_dir_all(owner.runs.join("run_nested/sub/sub_legacy")).unwrap();
    let captured = capture::capture(owner.clone(), "run_root", None).unwrap();
    let legacy = super::super::children_of(&state.run_store, "run_root");
    assert_eq!(captured.rows, legacy.children);
    assert_eq!(
        captured
            .rows
            .iter()
            .filter(|r| r.handle.as_deref() == Some("same_handle"))
            .count(),
        1
    );
    let current = captured
        .rows
        .iter()
        .find(|r| r.handle.as_deref() == Some("same_handle"))
        .unwrap();
    assert_eq!(current.run_id, "run_new");
    assert_eq!(current.parent_run_id, "run_second");
    let legacy = captured
        .rows
        .iter()
        .find(|r| r.run_id == "sub_legacy")
        .unwrap();
    assert!(legacy.legacy);
    assert_eq!(legacy.parent_run_id, "run_nested");
    assert_eq!(captured.evidence.missing_legacy_transcripts, 1);
    assert_eq!(
        capture::capture(owner, "run_root", Some("run_second".into()))
            .unwrap()
            .rows
            .len(),
        1
    );
}

#[test]
fn children_archived_root_is_supported_and_recorded_remote_roots_are_not_local_fallbacks() {
    let (_tmp, state, owner) = fixture(1);
    let remote = json!({"step_id":"remote", "run_id":"run_remote", "host":"other-owner", "transcript_path":"/fixture", "output":"", "success":true,"skipped":false,"rendered_prompt":"", "finished_at":"2026-11-06T10:00:00Z"});
    fs::write(
        owner.runs.join("run_root/step_results.jsonl"),
        format!("{remote}\n"),
    )
    .unwrap();
    append(&owner, "run_remote", "foreign", "run_should_not_appear");
    let captured = capture::capture(owner.clone(), "run_root", None).unwrap();
    assert_eq!(captured.rows.len(), 1);
    assert_eq!(captured.evidence.remote_step_roots_not_walked, 1);
    assert_eq!(
        status(capture::capture(
            owner.clone(),
            "run_root",
            Some("run_remote".into())
        )),
        StatusCode::NOT_FOUND
    );
    fs::create_dir_all(state.run_store.archive_root()).unwrap();
    fs::rename(
        owner.runs.join("run_root"),
        state.run_store.archive_root().join("run_root"),
    )
    .unwrap();
    let archived = capture::capture(owner, "run_root", None).unwrap();
    assert_eq!(archived.rows.len(), 1);
    assert_eq!(archived.proof.kind, "recorded_run");
}

#[test]
fn children_operator_tool_has_the_actual_paging_query_schema() {
    let op = crate::mcp::ops::ops()
        .iter()
        .find(|op| op.name == "runs.children")
        .unwrap();
    let query = serde_json::to_value((op.query)().unwrap()).unwrap();
    for field in ["host", "limit", "cursor", "parent_run_id"] {
        assert!(query["properties"].get(field).is_some());
    }
    assert_eq!(op.path, "/api/runs/:id/children");
    assert_eq!(op.method, http::Method::GET);
}

#[test]
fn children_page_byte_budget_continues_without_dropping_large_rows() {
    let (_tmp, _state, owner) = fixture(20);
    let mut snapshot = capture::capture(owner.clone(), "run_root", None).unwrap();
    for row in &mut snapshot.rows {
        row.error = Some("x".repeat(250_000));
    }
    snapshot.weights = snapshot
        .rows
        .iter()
        .map(|r| row_weight(r).unwrap())
        .collect();
    snapshot.weight = 64 * 1024 + snapshot.weights.iter().sum::<usize>();
    let mut cache = Cache::default();
    let cursor = cache.insert(snapshot).unwrap();
    let first = cache.page(&owner, "run_root", &None, 200, cursor).unwrap();
    assert!(!first.children.is_empty());
    assert!(first.children.len() < 20);
    assert!(serde_json::to_vec(&first).unwrap().len() < 5 * 1024 * 1024);
    let cursor = cache.decode(first.next_cursor.as_ref().unwrap()).unwrap();
    let last = cache.page(&owner, "run_root", &None, 200, cursor).unwrap();
    assert_eq!(first.children.len() + last.children.len(), 20);
    assert!(last.next_cursor.is_none());
}

fn ownership_metadata(owner: &Owner, run: &str, session: Option<&str>) {
    let directory = owner.global.join("transcripts");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join(format!("{run}.meta.json")), serde_json::to_vec(&json!({
        "version":1,"run_id":run,"session_id":session,"workspace_path":"/fixture",
        "backend_id":"fixture","trigger_source":if session.is_some(){"session_turn"}else{"manual"}
    })).unwrap()).unwrap();
}
fn session_ownership(owner: &Owner, session: &str, run: &str) {
    let directory = owner.global.join("sessions").join(session);
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("session.json"), serde_json::to_vec(&json!({
        "session_id":session,"runs":[{"run_id":run,"transcript_path":owner.global.join("transcripts").join(format!("{run}.jsonl"))}]
    })).unwrap()).unwrap();
}

#[test]
fn children_session_and_standalone_roots_need_no_workflow_record_or_child_directory() {
    let (_tmp, _state, owner) = fixture(0);
    fs::remove_dir_all(&owner.runs).unwrap();
    ownership_metadata(&owner, "run_session", Some("ses_owner"));
    session_ownership(&owner, "ses_owner", "run_session");
    let session =
        capture::capture(owner.clone(), "run_session", Some("run_session".into())).unwrap();
    assert_eq!(session.proof.kind, "session_turn");
    assert!(session.rows.is_empty());
    assert!(!owner.runs.exists());
    ownership_metadata(&owner, "run_standalone", None);
    let standalone = capture::capture(owner.clone(), "run_standalone", None).unwrap();
    assert_eq!(standalone.proof.kind, "standalone_agent");
    assert!(standalone.rows.is_empty());
    append(&owner, "run_session", "session_child", "sub_session_child");
    let children = capture::capture(owner.clone(), "run_session", None).unwrap();
    assert_eq!(children.rows.len(), 1);
    assert_eq!(children.rows[0].parent_run_id, "run_session");
    // A queued/legacy turn can be owned before its worker writes metadata.
    session_ownership(&owner, "ses_queued", "run_queued");
    assert_eq!(
        capture::capture(owner, "run_queued", None)
            .unwrap()
            .proof
            .kind,
        "session_turn"
    );
}

#[test]
fn children_session_proof_rejects_forged_ids_foreign_locators_and_missing_membership() {
    let (_tmp, _state, owner) = fixture(0);
    ownership_metadata(&owner, "run_session", Some("ses_owner"));
    session_ownership(&owner, "ses_owner", "run_other");
    assert_eq!(
        status(capture::capture(owner.clone(), "run_session", None)),
        StatusCode::CONFLICT
    );
    session_ownership(&owner, "ses_owner", "run_session");
    let path = owner.global.join("sessions/ses_owner/session.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["session_id"] = json!("ses_foreign");
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(
        status(capture::capture(owner.clone(), "run_session", None)),
        StatusCode::CONFLICT
    );
    record["session_id"] = json!("ses_owner");
    record["runs"][0]["transcript_path"] = json!("/outside/run_session.jsonl");
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(
        status(capture::capture(owner.clone(), "run_session", None)),
        StatusCode::NOT_IMPLEMENTED
    );
    session_ownership(&owner, "ses_owner", "run_session");
    let meta = owner.global.join("transcripts/run_session.meta.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&meta).unwrap()).unwrap();
    record["run_id"] = json!("run_other");
    fs::write(meta, serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(
        status(capture::capture(owner, "run_session", None)),
        StatusCode::CONFLICT
    );
}

#[test]
fn children_session_snapshot_survives_new_turns_but_not_ownership_removal() {
    let (_tmp, _state, owner) = fixture(0);
    ownership_metadata(&owner, "run_session", Some("ses_owner"));
    session_ownership(&owner, "ses_owner", "run_session");
    append(&owner, "run_session", "a", "sub_a");
    append(&owner, "run_session", "b", "sub_b");
    let mut cache = Cache::default();
    let cursor = cache
        .insert(capture::capture(owner.clone(), "run_session", None).unwrap())
        .unwrap();
    let page = cache.page(&owner, "run_session", &None, 1, cursor).unwrap();
    let token = page.next_cursor.unwrap();
    let session = owner.global.join("sessions/ses_owner/session.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&session).unwrap()).unwrap();
    record["runs"].as_array_mut().unwrap().push(json!({"run_id":"run_new","transcript_path":owner.global.join("transcripts/run_new.jsonl")}));
    let replacement = session.with_extension("tmp");
    fs::write(&replacement, serde_json::to_vec(&record).unwrap()).unwrap();
    fs::rename(replacement, &session).unwrap();
    let next = cache
        .page(
            &owner,
            "run_session",
            &None,
            1,
            cache.decode(&token).unwrap(),
        )
        .unwrap();
    assert_eq!(next.children[0].run_id, "sub_b");
    fs::remove_file(owner.global.join("transcripts/run_session.meta.json")).unwrap();
    assert_eq!(
        status(cache.page(
            &owner,
            "run_session",
            &None,
            1,
            cache.decode(&token).unwrap()
        )),
        StatusCode::CONFLICT
    );
}

#[test]
fn children_archived_session_and_standalone_ownership_are_preserved() {
    let (_tmp, _state, owner) = fixture(0);
    ownership_metadata(&owner, "run_session", Some("ses_owner"));
    session_ownership(&owner, "ses_owner", "run_session");
    fs::create_dir_all(owner.global.join("sessions-archive")).unwrap();
    fs::rename(
        owner.global.join("sessions/ses_owner"),
        owner.global.join("sessions-archive/ses_owner"),
    )
    .unwrap();
    fs::create_dir_all(owner.global.join("transcripts/archive")).unwrap();
    fs::rename(
        owner.global.join("transcripts/run_session.meta.json"),
        owner
            .global
            .join("transcripts/archive/run_session.meta.json"),
    )
    .unwrap();
    assert_eq!(
        capture::capture(owner, "run_session", None)
            .unwrap()
            .proof
            .kind,
        "session_turn"
    );
}

#[test]
fn children_opt_in_live_session_ownership_capture_is_read_only() {
    let (Ok(global), Ok(run)) = (
        std::env::var("RUPU_CHILDREN_LIVE_GLOBAL"),
        std::env::var("RUPU_CHILDREN_LIVE_RUN"),
    ) else {
        return;
    };
    let global = PathBuf::from(global).canonicalize().unwrap();
    let owner = Owner {
        runs: global.join("runs"),
        global,
    };
    let before = owner.runs.join(&run).exists();
    let snapshot = capture::capture(owner.clone(), &run, Some(run.clone())).unwrap();
    assert_eq!(snapshot.proof.kind, "session_turn");
    assert_eq!(snapshot.evidence.history_complete, "unknown");
    assert!(snapshot.evidence.bytes_read > 0);
    assert_eq!(owner.runs.join(&run).exists(), before);
    assert!(snapshot.rows.iter().all(|row| row.parent_run_id == run));
}
