use super::*;
use rupu_launch::ledger::{self, LedgerLine, Started};
use rupu_tools::ChildKind;
use serde_json::{json, Value};
use std::{fs, io::Write};
fn start(run: &str, project: &str) -> String {
    json!({"type":"run_start","data":{"run_id":run,"workspace_id":project,"agent":"review","provider":"test","model":"test","started_at":"2026-11-08T00:00:00Z","mode":"readonly"}}).to_string()
}
fn event(content: &str) -> String {
    json!({"type":"assistant_message","data":{"content":content}}).to_string()
}
fn fixture() -> (tempfile::TempDir, AppState, SearchQuery) {
    let tmp = tempfile::tempdir().unwrap();
    let state = AppState::new(
        tmp.path().canonicalize().unwrap(),
        rupu_config::PricingConfig::default(),
    );
    let record=serde_json::from_value(json!({"id":"run_root","workflow_name":"test","status":"completed","inputs":{},"workspace_id":"ws_fixture","workspace_path":"/fixture","transcript_dir":"/fixture","started_at":"2026-11-08T00:00:00Z"})).unwrap();
    state
        .run_store
        .create(record, "name: test\nsteps: []\n")
        .unwrap();
    (
        tmp,
        state,
        SearchQuery {
            project: "ws_fixture".into(),
            q: "needle".into(),
            host: None,
            limit: Some(1),
            cursor: None,
        },
    )
}
fn child(state: &AppState, parent: &str, run: &str, project: &str, body: &str) -> PathBuf {
    let dir = state.run_store.root.join(parent);
    fs::create_dir_all(&dir).unwrap();
    let transcript = state
        .global_dir
        .join("transcripts")
        .join(format!("{run}.jsonl"));
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(&transcript, format!("{}\n{}", start(run, project), body)).unwrap();
    let mut row = Started::new(&format!("h_{run}"), ChildKind::Agent, "review", run, 1, 1);
    row.transcript = Some(transcript.clone());
    ledger::append(&dir, &LedgerLine::Started(row));
    transcript
}
fn find(state: &AppState, q: &SearchQuery) -> ApiResult<SearchPage> {
    search(state, "run_root", q, &AtomicBool::new(false))
}
fn status<T>(v: ApiResult<T>) -> StatusCode {
    match v {
        Err(e) => e.0,
        Ok(_) => panic!("expected error"),
    }
}
#[tokio::test]
async fn nested_hit_has_exact_proof_and_direct_existing_page_cursor() {
    let (_tmp, s, q) = fixture();
    child(
        &s,
        "run_root",
        "run_parent",
        "ws_fixture",
        &event("nothing"),
    );
    child(
        &s,
        "run_parent",
        "run_child",
        "ws_fixture",
        &format!("invalid\n{}\n{}", event("a needle here"), event("later")),
    );
    let page = find(&s, &q).unwrap();
    assert_eq!(page.hits.len(), 1);
    let hit = &page.hits[0];
    assert_eq!(hit.run_id, "run_child");
    assert_eq!(hit.authorization_run_id, "run_root");
    assert_eq!(hit.position, 3);
    assert_eq!(hit.page_cursor.position, 3);
    assert_eq!(hit.first_run_start["data"]["workspace_id"], "ws_fixture");
    assert_eq!(hit.provenance.len(), 2);
    assert_eq!(hit.provenance[1].parent_run_id, "run_parent");
    let Json(p) = page::get_page(
        State(s.clone()),
        Query(PageQuery {
            path: hit.transcript_path.clone(),
            host: Some("local".into()),
            run: Some(hit.authorization_run_id.clone()),
            limit: Some(1),
            cursor: Some(hit.page_cursor.token.clone()),
        }),
    )
    .await
    .unwrap();
    let p = serde_json::to_value(p).unwrap();
    assert_eq!(p["snapshot"], hit.page_cursor.snapshot);
    assert_eq!(p["records"][0]["position"], 3);
    assert_eq!(p["records"][0]["event"]["data"]["content"], "a needle here");
}
#[test]
fn continuation_binds_owner_root_query_project_and_authenticates_bytes() {
    let (_tmp, s, mut q) = fixture();
    child(
        &s,
        "run_root",
        "run_child",
        "ws_fixture",
        &format!("{}\n{}", event("needle1"), event("needle2")),
    );
    let first = find(&s, &q).unwrap();
    q.cursor = first.next_cursor;
    assert!(q.cursor.is_some());
    let second = find(&s, &q).unwrap();
    assert_eq!(second.hits[0].position, 3);
    let mut other = q.clone();
    other.q = "different".into();
    assert_eq!(status(find(&s, &other)), StatusCode::BAD_REQUEST);
    other = q.clone();
    other.project = "wrong".into();
    assert_eq!(status(find(&s, &other)), StatusCode::BAD_REQUEST);
    other = q.clone();
    other.cursor.as_mut().unwrap().push('x');
    assert_eq!(status(find(&s, &other)), StatusCode::BAD_REQUEST);
    let (_tmp2, s2, _) = fixture();
    assert_eq!(status(find(&s2, &q)), StatusCode::BAD_REQUEST);
    assert_eq!(
        status(search(&s, "run_other", &q, &AtomicBool::new(false))),
        StatusCode::BAD_REQUEST
    );
}
#[test]
fn appended_transcript_invalidates_search_continuation() {
    let (_tmp, s, mut q) = fixture();
    let path = child(
        &s,
        "run_root",
        "run_child",
        "ws_fixture",
        &format!("{}\n{}", event("needle1"), event("needle2")),
    );
    q.cursor = find(&s, &q).unwrap().next_cursor;
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    assert_eq!(status(find(&s, &q)), StatusCode::CONFLICT);
}
#[test]
fn canonical_project_mismatch_returns_no_member_data() {
    let (_tmp, s, mut q) = fixture();
    child(&s, "run_root", "run_child", "ws_fixture", &event("needle"));
    q.project = "wrong".into();
    assert_eq!(status(find(&s, &q)), StatusCode::CONFLICT);
}
#[test]
fn later_prologue_wrong_project_or_wrong_run_cannot_authorize_hit() {
    for (body, run, project) in [
        (
            format!(
                "broken\n{}\n{}",
                start("run_child", "ws_fixture"),
                event("needle")
            ),
            "run_child",
            "ws_fixture",
        ),
        (
            format!("{}\n{}", start("run_child", "wrong"), event("needle")),
            "run_child",
            "wrong",
        ),
        (
            format!("{}\n{}", start("another", "ws_fixture"), event("needle")),
            "another",
            "ws_fixture",
        ),
    ] {
        let (_tmp, s, q) = fixture();
        let path = child(&s, "run_root", "run_child", project, &event("needle"));
        fs::write(path, body).unwrap();
        let page = find(&s, &q).unwrap();
        assert!(page.hits.is_empty(), "{run}");
        assert_eq!(page.coverage.unverified_transcripts, 1);
        assert!(!page.has_more);
    }
}
#[test]
fn empty_page_can_continue_without_false_complete_history() {
    let (_tmp, s, mut q) = fixture();
    let body = format!(
        "{}{}",
        format!("{}\n", event("hay")).repeat(RECORDS + 1),
        event("needle")
    );
    child(&s, "run_root", "run_child", "ws_fixture", &body);
    let first = find(&s, &q).unwrap();
    assert!(first.hits.is_empty());
    assert!(first.has_more);
    assert_eq!(first.coverage.history_complete, "unknown");
    q.cursor = first.next_cursor;
    let second = find(&s, &q).unwrap();
    assert_eq!(second.hits.len(), 1);
    assert!(!second.has_more);
}
#[test]
fn malformed_and_oversized_records_preserve_nonblank_positions() {
    let (_tmp, s, q) = fixture();
    child(
        &s,
        "run_root",
        "run_child",
        "ws_fixture",
        &format!(
            " \ninvalid\n{}\n{}",
            "x".repeat(256 * 1024 + 1),
            event("needle")
        ),
    );
    let page = find(&s, &q).unwrap();
    assert_eq!(page.hits[0].position, 4);
    assert_eq!(page.coverage.malformed_records, 1);
    assert_eq!(page.coverage.oversized_records, 1);
}
#[test]
fn unbounded_single_line_is_explicit_limit_not_completed_empty_result() {
    let (_tmp, s, mut q) = fixture();
    child(
        &s,
        "run_root",
        "run_child",
        "ws_fixture",
        &"x".repeat(SCAN_BYTES as usize + 1),
    );
    let first = find(&s, &q).unwrap();
    assert!(first.has_more);
    q.cursor = first.next_cursor;
    assert_eq!(status(find(&s, &q)), StatusCode::PAYLOAD_TOO_LARGE);
}
#[test]
fn archived_root_keeps_active_step_and_nested_child_membership() {
    let (_tmp, s, q) = fixture();
    let path = child(&s, "run_step", "run_nested", "ws_fixture", &event("needle"));
    let step = path.with_file_name("run_step.jsonl");
    fs::write(
        &step,
        format!("{}\n{}", start("run_step", "ws_fixture"), event("no match")),
    )
    .unwrap();
    let row = json!({"step_id":"inspect","run_id":"run_step","transcript_path":step,"output":"","success":true,"skipped":false,"rendered_prompt":"","finished_at":"2026-11-08T00:00:00Z"});
    fs::write(
        s.run_store.root.join("run_root/step_results.jsonl"),
        format!("{row}\n"),
    )
    .unwrap();
    fs::create_dir_all(s.run_store.archive_root()).unwrap();
    fs::rename(
        s.run_store.root.join("run_root"),
        s.run_store.archive_root().join("run_root"),
    )
    .unwrap();
    let page = find(&s, &q).unwrap();
    assert_eq!(page.hits[0].run_id, "run_nested");
    assert_eq!(page.hits[0].provenance[0].source, "workflow_step");
}
#[test]
fn changed_canonical_project_invalidates_cached_family() {
    let (_tmp, s, mut q) = fixture();
    child(
        &s,
        "run_root",
        "run_child",
        "ws_fixture",
        &format!("{}\n{}", event("needle1"), event("needle2")),
    );
    q.cursor = find(&s, &q).unwrap().next_cursor;
    let path = s.run_store.root.join("run_root/run.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["workspace_id"] = json!("other");
    fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(status(find(&s, &q)), StatusCode::CONFLICT);
}
#[test]
fn source_enumeration_does_not_read_huge_legacy_transcript() {
    let (_tmp, s, _) = fixture();
    let path = s
        .run_store
        .root
        .join("run_root/sub/run_old/transcript.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let f = File::create(&path).unwrap();
    f.set_len(1024 * 1024 * 1024).unwrap();
    let captured =
        membership::capture(owner(&s).unwrap(), "run_root", &AtomicBool::new(false)).unwrap();
    assert_eq!(captured.members.len(), 2);
    assert_eq!(captured.members[1].source, "legacy_subdirectory");
}
#[test]
fn explicit_remote_invalid_and_cancelled_scopes_are_refused() {
    let (_tmp, s, mut q) = fixture();
    q.host = Some("peer".into());
    assert_eq!(status(find(&s, &q)), StatusCode::NOT_IMPLEMENTED);
    q.host = None;
    q.q.clear();
    assert_eq!(status(find(&s, &q)), StatusCode::BAD_REQUEST);
    assert_eq!(
        status(membership::capture(
            owner(&s).unwrap(),
            "run_root",
            &AtomicBool::new(true)
        )),
        StatusCode::CONFLICT
    );
}
#[test]
fn matching_is_literal_case_sensitive_on_values_not_keys() {
    assert_eq!(match_excerpt(&json!({"needle":"no"}), "needle"), None);
    assert_eq!(match_excerpt(&json!({"text":"Needle"}), "needle"), None);
    assert!(match_excerpt(&json!({"result":{"array":["needle"]}}), "needle").is_some());
}
#[test]
fn mcp_search_uses_exact_http_route_and_required_scope_schema() {
    let op = crate::mcp::ops::ops()
        .iter()
        .find(|op| op.name == "runs.transcript_search")
        .unwrap();
    assert_eq!(op.path, "/api/runs/:id/transcripts/search");
    assert_eq!(op.method, http::Method::GET);
    let schema = serde_json::to_value((op.query)().unwrap()).unwrap();
    for key in ["project", "q", "host", "limit", "cursor"] {
        assert!(schema["properties"].get(key).is_some());
    }
    assert!(schema["required"]
        .as_array()
        .unwrap()
        .contains(&json!("project")));
    assert!(schema["required"].as_array().unwrap().contains(&json!("q")));
}

#[test]
fn archived_standalone_root_project_comes_from_first_record() {
    let (_tmp, s, mut q) = fixture();
    let directory = s.global_dir.join("transcripts/archive");
    fs::create_dir_all(&directory).unwrap();
    let meta = json!({"version":1,"run_id":"run_alone","workspace_path":"/fixture","backend_id":"local_checkout","trigger_source":"run_cli"});
    fs::write(directory.join("run_alone.meta.json"), meta.to_string()).unwrap();
    fs::write(
        directory.join("run_alone.jsonl"),
        format!(
            "{}\n{}\n{}",
            start("run_alone", "ws_fixture"),
            event("needle1"),
            event("needle2")
        ),
    )
    .unwrap();
    let page = search(&s, "run_alone", &q, &AtomicBool::new(false)).unwrap();
    assert_eq!(page.hits[0].run_id, "run_alone");
    assert!(page.hits[0].provenance.is_empty());
    q.cursor = page.next_cursor;
    let path = directory.join("run_alone.jsonl");
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    assert_eq!(
        status(search(&s, "run_alone", &q, &AtomicBool::new(false))),
        StatusCode::CONFLICT
    );
}
#[test]
fn canonical_session_turn_without_child_directory_can_search_itself() {
    let (_tmp, s, q) = fixture();
    let path = s.global_dir.join("transcripts/run_turn.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        format!("{}\n{}", start("run_turn", "ws_fixture"), event("needle")),
    )
    .unwrap();
    let directory = s.global_dir.join("sessions-archive/ses_test");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("session.json"),json!({"session_id":"ses_test","workspace_id":"ws_fixture","runs":[{"run_id":"run_turn","transcript_path":path}]}).to_string()).unwrap();
    let page = search(&s, "run_turn", &q, &AtomicBool::new(false)).unwrap();
    assert_eq!(page.hits[0].run_id, "run_turn");
    assert!(page.hits[0].provenance.is_empty());
}
#[test]
fn missing_member_and_incomplete_ledger_are_visible_coverage() {
    let (_tmp, s, q) = fixture();
    let path = child(&s, "run_root", "run_child", "ws_fixture", &event("needle"));
    fs::remove_file(path).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(s.run_store.root.join("run_root/children.jsonl"))
        .unwrap()
        .write_all(b"{\"t\":")
        .unwrap();
    let page = find(&s, &q).unwrap();
    assert!(page.hits.is_empty());
    assert_eq!(page.coverage.unavailable_transcripts, 2);
    assert!(page.coverage.membership_incomplete_tail_bytes > 0);
    assert_eq!(page.coverage.history_complete, "unknown");
}
#[test]
fn symlinked_recorded_membership_source_is_not_followed() {
    let (_tmp, s, q) = fixture();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("children.jsonl"), b"secret").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("children.jsonl"),
        s.run_store.root.join("run_root/children.jsonl"),
    )
    .unwrap();
    assert_eq!(status(find(&s, &q)), StatusCode::BAD_REQUEST);
}
#[test]
fn expired_snapshot_and_foreign_member_locator_are_rejected() {
    let (_tmp, s, mut q) = fixture();
    child(
        &s,
        "run_root",
        "run_child",
        "ws_fixture",
        &format!("{}\n{}", event("needle1"), event("needle2")),
    );
    let first = find(&s, &q).unwrap();
    q.cursor = first.next_cursor;
    let mut guard = cache().lock().unwrap();
    let entry = guard
        .rows
        .iter_mut()
        .find(|s| s.id == first.snapshot)
        .unwrap();
    Arc::get_mut(entry).unwrap().created = Instant::now() - TTL - Duration::from_secs(1);
    drop(guard);
    assert_eq!(status(find(&s, &q)), StatusCode::CONFLICT);
}
#[tokio::test]
async fn http_endpoint_matches_direct_search() {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let (_tmp, s, q) = fixture();
    child(&s, "run_root", "run_child", "ws_fixture", &event("needle"));
    let response = super::super::routes()
        .with_state(s.clone())
        .oneshot(
            Request::builder()
                .uri("/api/runs/run_root/transcripts/search?project=ws_fixture&q=needle&limit=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let direct = serde_json::to_value(find(&s, &q).unwrap()).unwrap();
    for key in [
        "schema_version",
        "owner_host_id",
        "root_run_id",
        "project_id",
        "query",
        "hits",
        "coverage",
    ] {
        assert_eq!(payload[key], direct[key]);
    }
}

#[test]
fn scope_reads_only_exact_bounded_project_record_not_catalog() {
    let (_tmp, s, q) = fixture();
    let registry = s.global_dir.join("workspaces");
    fs::create_dir_all(&registry).unwrap();
    for n in 0..1000 {
        fs::write(
            registry.join(format!("ws_unrelated_{n}.toml")),
            b"invalid unrelated record",
        )
        .unwrap();
    }
    let unrelated = registry.join("ws_huge.toml");
    File::create(unrelated)
        .unwrap()
        .set_len(1024 * 1024 * 1024)
        .unwrap();
    child(&s, "run_root", "run_child", "ws_fixture", &event("needle"));
    assert_eq!(find(&s, &q).unwrap().hits.len(), 1);
    File::create(registry.join("ws_fixture.toml"))
        .unwrap()
        .set_len(65537)
        .unwrap();
    assert_eq!(status(find(&s, &q)), StatusCode::PAYLOAD_TOO_LARGE);
}
#[test]
fn project_registry_scope_changes_and_traversal_are_rejected() {
    let (_tmp, s, mut q) = fixture();
    let registry = s.global_dir.join("workspaces");
    fs::create_dir_all(&registry).unwrap();
    fs::write(
        registry.join("ws_fixture.toml"),
        format!(
            "id = \"ws_fixture\"\npath = {:?}\n",
            s.global_dir.to_string_lossy()
        ),
    )
    .unwrap();
    child(
        &s,
        "run_root",
        "run_child",
        "ws_fixture",
        &format!("{}\n{}", event("needle1"), event("needle2")),
    );
    q.cursor = find(&s, &q).unwrap().next_cursor;
    fs::write(
        registry.join("ws_fixture.toml"),
        format!(
            "id = \"ws_fixture\"\npath = {:?}\n#changed\n",
            s.global_dir.to_string_lossy()
        ),
    )
    .unwrap();
    assert_eq!(status(find(&s, &q)), StatusCode::CONFLICT);
    q.cursor = None;
    q.project = "../outside".into();
    assert_eq!(status(find(&s, &q)), StatusCode::BAD_REQUEST);
}
#[test]
fn registry_and_transcript_fifos_are_refused_without_waiting() {
    use crate::api::fs_open::test_support::{mkfifo, within_deadline};
    let (_tmp, s, q) = fixture();
    let registry = s.global_dir.join("workspaces");
    fs::create_dir_all(&registry).unwrap();
    let fifo = registry.join("ws_fixture.toml");
    if !mkfifo(&fifo) {
        return;
    }
    let state = s.clone();
    let query = q.clone();
    assert_eq!(
        within_deadline(move || status(find(&state, &query))),
        StatusCode::CONFLICT
    );
    fs::remove_file(fifo).unwrap();
    let path = child(&s, "run_root", "run_child", "ws_fixture", &event("needle"));
    fs::remove_file(&path).unwrap();
    assert!(mkfifo(&path));
    assert_eq!(
        within_deadline(move || status(find(&s, &q))),
        StatusCode::BAD_REQUEST
    );
}

fn agentiflow_fixture() -> (tempfile::TempDir, AppState, SearchQuery, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let s = AppState::new(
        tmp.path().canonicalize().unwrap(),
        rupu_config::PricingConfig::default(),
    );
    fs::create_dir_all(&s.run_store.root).unwrap();
    let dir = s.global_dir.join("agentiflows/af_family");
    fs::create_dir_all(dir.join("lead")).unwrap();
    fs::write(dir.join("agentiflow.json"), json!({"id":"af_family","name":"review","engagement_profiles":[],"trigger":"agentiflow","status":"completed","rounds":2,"goals":[],"started_at":"2026-11-09T00:00:00Z"}).to_string()).unwrap();
    fs::write(dir.join("children.jsonl"), "").unwrap();
    for round in 0..2 {
        fs::write(
            dir.join(format!("lead/transcript.r{round}.jsonl")),
            format!(
                "{}\n{}\n",
                start("af_family", "ws_fixture"),
                event(&format!("needle lead {round}"))
            ),
        )
        .unwrap();
    }
    let q = SearchQuery {
        project: "ws_fixture".into(),
        q: "needle".into(),
        host: Some("local".into()),
        limit: Some(50),
        cursor: None,
    };
    (tmp, s, q, dir)
}
fn flow_find(s: &AppState, q: &SearchQuery) -> ApiResult<SearchPage> {
    search(s, "af_family", q, &AtomicBool::new(false))
}
fn flow_child(s: &AppState, dir: &std::path::Path, run: &str, kind: ChildKind) {
    let path = s
        .global_dir
        .join("transcripts")
        .join(format!("{run}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        format!("{}\n{}\n", start(run, "ws_fixture"), event("needle child")),
    )
    .unwrap();
    let mut row = Started::new(&format!("h_{run}"), kind, "review", run, 1, 1);
    row.transcript = Some(path);
    row.tree = Some(dir.to_path_buf());
    row.round = Some(1);
    ledger::append(dir, &LedgerLine::Started(row));
}
#[tokio::test]
async fn agentiflow_rounds_and_nested_workers_have_exact_direct_locators() {
    let (_tmp, s, q, dir) = agentiflow_fixture();
    flow_child(&s, &dir, "run_workflow", ChildKind::Workflow);
    child(
        &s,
        "run_workflow",
        "sub_nested",
        "ws_fixture",
        &event("needle nested"),
    );
    let page = flow_find(&s, &q).unwrap();
    assert_eq!(page.root_type, "agentiflow");
    assert_eq!(page.hits.len(), 4);
    assert_eq!(page.coverage.members_total, 4);
    assert_eq!(page.coverage.history_complete, "unknown");
    let rounds: Vec<_> = page.hits.iter().filter_map(|h| h.lead_round).collect();
    assert_eq!(rounds, [0, 1]);
    let nested = page.hits.iter().find(|h| h.run_id == "sub_nested").unwrap();
    assert_eq!(nested.authorization_run_id, "af_family");
    assert_eq!(nested.provenance.len(), 2);
    assert_eq!(nested.provenance[0].parent_run_id, "af_family");
    assert_eq!(nested.provenance[1].parent_run_id, "run_workflow");
    for hit in &page.hits {
        let Json(window) = page::get_page(
            State(s.clone()),
            Query(PageQuery {
                path: hit.transcript_path.clone(),
                host: Some("local".into()),
                run: Some("af_family".into()),
                limit: Some(1),
                cursor: Some(hit.page_cursor.token.clone()),
            }),
        )
        .await
        .unwrap();
        let raw = serde_json::to_value(window).unwrap();
        assert_eq!(raw["snapshot"], hit.page_cursor.snapshot);
        assert_eq!(raw["records"][0]["position"], hit.position);
    }
    assert!(!s.run_store.root.join("af_family/run.json").exists());
}
#[test]
fn agentiflow_search_rejects_contradictory_tree_and_noncanonical_rounds() {
    for invalid in ["tree", "round_alias", "round_overflow"] {
        let (_tmp, s, q, dir) = agentiflow_fixture();
        match invalid {
            "tree" => {
                let mut row =
                    Started::new("h_wrong", ChildKind::Agent, "review", "run_wrong", 1, 1);
                row.tree = Some(s.global_dir.join("another-flow"));
                ledger::append(&dir, &LedgerLine::Started(row));
            }
            "round_alias" => {
                fs::copy(
                    dir.join("lead/transcript.r1.jsonl"),
                    dir.join("lead/transcript.r01.jsonl"),
                )
                .unwrap();
            }
            _ => {
                fs::write(dir.join("lead/transcript.r4294967296.jsonl"), "{}\n").unwrap();
            }
        }
        assert_eq!(status(flow_find(&s, &q)), StatusCode::CONFLICT, "{invalid}");
    }
}
#[test]
fn agentiflow_foreign_round_is_counted_unverified_never_a_hit() {
    let (_tmp, s, q, dir) = agentiflow_fixture();
    fs::write(
        dir.join("lead/transcript.r1.jsonl"),
        format!(
            "{}\n{}\n",
            start("af_other", "ws_fixture"),
            event("needle foreign")
        ),
    )
    .unwrap();
    let page = flow_find(&s, &q).unwrap();
    assert_eq!(page.hits.len(), 1);
    assert_eq!(page.hits[0].lead_round, Some(0));
    assert_eq!(page.coverage.unverified_transcripts, 1);
}
#[test]
fn agentiflow_round_continuation_binds_root_project_query_and_file_stamp() {
    let (_tmp, s, mut q, dir) = agentiflow_fixture();
    q.limit = Some(1);
    let first = flow_find(&s, &q).unwrap();
    q.cursor = first.next_cursor;
    let second = flow_find(&s, &q).unwrap();
    assert_eq!(second.hits[0].lead_round, Some(1));
    let mut foreign = q.clone();
    foreign.project = "ws_other".into();
    assert_eq!(status(flow_find(&s, &foreign)), StatusCode::BAD_REQUEST);
    foreign = q.clone();
    foreign.q = "other".into();
    assert_eq!(status(flow_find(&s, &foreign)), StatusCode::BAD_REQUEST);
    fs::OpenOptions::new()
        .append(true)
        .open(dir.join("lead/transcript.r1.jsonl"))
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    assert_eq!(status(flow_find(&s, &q)), StatusCode::CONFLICT);
}
#[test]
fn agentiflow_source_growth_never_claims_new_round_is_in_previous_capture() {
    let (_tmp, s, mut q, dir) = agentiflow_fixture();
    q.limit = Some(1);
    let first = flow_find(&s, &q).unwrap();
    fs::copy(
        dir.join("lead/transcript.r1.jsonl"),
        dir.join("lead/transcript.r2.jsonl"),
    )
    .unwrap();
    q.cursor = first.next_cursor;
    let second = flow_find(&s, &q).unwrap();
    assert_eq!(second.coverage.members_total, 2);
    assert_eq!(second.hits[0].lead_round, Some(1));
    assert_eq!(second.coverage.history_complete, "unknown");
    q.cursor = None;
    q.limit = Some(50);
    assert_eq!(flow_find(&s, &q).unwrap().coverage.members_total, 3);
}
#[test]
fn agentiflow_search_never_accepts_missing_or_changed_root_proof() {
    let (_tmp, s, mut q, dir) = agentiflow_fixture();
    q.limit = Some(1);
    q.cursor = flow_find(&s, &q).unwrap().next_cursor;
    fs::write(dir.join("agentiflow.json"), "{}\n").unwrap();
    assert_eq!(status(flow_find(&s, &q)), StatusCode::CONFLICT);
}
#[cfg(unix)]
#[test]
fn agentiflow_search_refuses_symlinked_lead_round() {
    let (_tmp, s, q, dir) = agentiflow_fixture();
    std::os::unix::fs::symlink(
        dir.join("lead/transcript.r1.jsonl"),
        dir.join("lead/transcript.r2.jsonl"),
    )
    .unwrap();
    assert_eq!(status(flow_find(&s, &q)), StatusCode::BAD_REQUEST);
}

#[test]
fn agentiflow_lead_prologue_does_not_drop_foreign_host_or_alias_claims() {
    for target in ["host_id", "run_id", "workspace_id"] {
        let (_tmp, s, q, dir) = agentiflow_fixture();
        let mut raw: Value = serde_json::from_str(&start("af_family", "ws_fixture")).unwrap();
        raw[target] = json!("foreign");
        fs::write(
            dir.join("lead/transcript.r1.jsonl"),
            format!("{raw}\n{}\n", event("needle foreign")),
        )
        .unwrap();
        let page = flow_find(&s, &q).unwrap();
        assert_eq!(page.coverage.unverified_transcripts, 1, "{target}");
        assert_eq!(page.hits.len(), 1, "{target}");
    }
}
