use super::*;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use rupu_orchestrator::runs::RunRecord;
use std::{fs, path::PathBuf};
use tower::ServiceExt;

const ID: &str = "run_fixture";
const WS: &str = "ws_fixture";
fn fixture() -> (tempfile::TempDir, AppState, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let s = AppState::new(
        tmp.path().canonicalize().unwrap(),
        rupu_config::PricingConfig::default(),
    );
    let run: RunRecord=serde_json::from_value(json!({"id":ID,"workflow_name":"review","status":"completed","inputs":{},"workspace_id":WS,"workspace_path":"/fixture","transcript_dir":"/fixture/transcripts","started_at":"2026-11-09T10:00:00Z"})).unwrap();
    s.run_store
        .create(run, "name: review\nsteps: []\n")
        .unwrap();
    let path = s.run_store.root.join(ID);
    (tmp, s, path)
}
fn item(index: usize) -> Value {
    json!({"index":index,"item":index,"sub_id":format!("child-{index}"),"rendered_prompt":"review","run_id":format!("run_child_{index}"),"transcript_path":format!("/fixture/{index}.jsonl"),"output":"done","success":true})
}
fn result(n: usize) -> Value {
    json!({"step_id":"same-step","run_id":"run_agent","transcript_path":"/fixture/agent.jsonl","output":"summary","success":true,"skipped":false,"rendered_prompt":"review","kind":"for_each","items":(0..n).map(item).collect::<Vec<_>>(),"findings":[{"source":"worker","severity":"high","title":"finding","body":"evidence"}],"finished_at":"2026-11-09T10:01:00Z"})
}
fn lines(values: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in values {
        bytes.extend(serde_json::to_vec(value).unwrap());
        bytes.push(b'\n');
    }
    bytes
}
async fn request(s: &AppState, path: &str) -> (StatusCode, Value) {
    let response = routes()
        .with_state(s.clone())
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
#[test]
fn fanout_children_are_separate_bounded_exact_parent_pages() {
    let (_tmp, s, path) = fixture();
    fs::write(path.join("step_results.jsonl"), lines(&[result(3000)])).unwrap();
    let c = capture::capture(&s, ID, WS).unwrap();
    assert_eq!(c.header["workflow_source"]["state"], "available");
    assert_eq!(c.header["sections"]["steps"]["source_total"], 0);
    let page = c.page("results", None, None, 50).unwrap();
    let row = &page["records"][0];
    assert_eq!(row["items_count"], 3000);
    assert!(row["value"].get("items").is_none());
    assert_eq!(row["omitted_fields"], json!(["items", "findings"]));
    let mut cursor = None;
    let mut indices = Vec::new();
    loop {
        let p = c
            .page("items", Some("result:0"), cursor.as_deref(), 100)
            .unwrap();
        assert!(serde_json::to_vec(&p).unwrap().len() <= MAX_PAGE);
        indices.extend(
            p["records"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["value"]["index"].as_u64().unwrap()),
        );
        cursor = p["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(indices, (0..3000).collect::<Vec<u64>>());
    assert_eq!(
        c.page("findings", Some("result:0"), None, 50).unwrap()["records"][0]["value"]["severity"],
        "high"
    );
}
#[test]
fn duplicate_steps_keep_record_offsets_and_cursors_cannot_cross_parents() {
    let (_tmp, s, path) = fixture();
    let first = lines(&[result(2)]);
    let offset = first.len();
    fs::write(
        path.join("step_results.jsonl"),
        [first, lines(&[result(2)])].concat(),
    )
    .unwrap();
    let c = capture::capture(&s, ID, WS).unwrap();
    let p = c.page("results", None, None, 50).unwrap();
    assert_eq!(p["records"].as_array().unwrap().len(), 2);
    assert_eq!(p["records"][1]["record_id"], format!("result:{offset}"));
    let p = c.page("items", Some("result:0"), None, 1).unwrap();
    let cursor = p["next_cursor"].as_str().unwrap();
    assert_eq!(
        c.page("items", Some(&format!("result:{offset}")), Some(cursor), 1)
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    assert!(c
        .page("findings", Some("result:0"), Some(cursor), 1)
        .is_err());
    assert!(c.page("items", None, None, 50).is_err());
    assert!(c.page("results", Some("result:0"), None, 50).is_err());
    let other = capture::capture(&s, ID, WS).unwrap();
    assert!(other
        .page("items", Some("result:0"), Some(cursor), 1)
        .is_err());
}
#[test]
fn malformed_partial_foreign_event_and_future_attempt_have_no_authority() {
    let (_tmp, s, path) = fixture();
    fs::write(path.join("step_results.jsonl"), b"{bad}\n{\"partial\":").unwrap();
    fs::write(
        path.join("events.jsonl"),
        lines(&[
            json!({"type":"run_paused","run_id":"run_other"}),
            json!({"type":"future","run_id":ID}),
            json!({"type":"run_paused","run_id":ID}),
        ]),
    )
    .unwrap();
    fs::write(path.join("attempts.jsonl"),lines(&[json!({"v":2,"step_id":"x","agent_run_id":"run_child","transcript_path":"/fixture/a","started_at":"2026-11-09T10:00:00Z"})])).unwrap();
    let c = capture::capture(&s, ID, WS).unwrap();
    let p = c.page("results", None, None, 50).unwrap();
    assert_eq!(p["records"][0]["state"], "malformed");
    assert_eq!(p["records"][1]["state"], "partial");
    assert_eq!(p["source_complete"], false);
    assert!(p["source_total"].is_null());
    let e = c.page("events", None, None, 50).unwrap();
    assert_eq!(e["records"][0]["state"], "malformed");
    assert_eq!(e["records"][1]["state"], "malformed");
    assert_eq!(e["records"][2]["state"], "available");
    assert_eq!(
        c.page("attempts", None, None, 50).unwrap()["records"][0]["state"],
        "malformed"
    );
}
#[test]
fn oversized_payload_is_explicit_unavailable_and_does_not_clip_identifiers() {
    let (_tmp, s, path) = fixture();
    let mut r = result(1);
    r["items"][0]["output"] = json!("x".repeat(MAX_ROW));
    fs::write(path.join("step_results.jsonl"), lines(&[r])).unwrap();
    let c = capture::capture(&s, ID, WS).unwrap();
    let p = c.page("items", Some("result:0"), None, 50).unwrap();
    let row = &p["records"][0];
    assert_eq!(row["state"], "oversized");
    assert!(row["value"].is_null());
    assert_eq!(row["parent_record_id"], "result:0");
    assert_eq!(c.header["unavailable_rows"], 1);
}
#[test]
fn prefix_read_is_io_bounded_and_reserves_owner_recheck() {
    let (_tmp, _s, path) = fixture();
    fs::write(path.join("large.jsonl"), vec![b'x'; 4096]).unwrap();
    let mut input = input::Input::open(&path).unwrap();
    let read = input.read("large.jsonl", 1024, true).unwrap().unwrap();
    assert_eq!(read.bytes.len(), 1024);
    assert!(!read.complete);
    assert_eq!(read.source_bytes, 4096);
    assert_eq!(input.bytes, 1024);
    input.bytes = input::MAX_INPUT - 512 * 1024 - 7;
    assert_eq!(
        input
            .read("large.jsonl", 1024, true)
            .unwrap()
            .unwrap()
            .bytes
            .len(),
        7
    );
    assert!(input.read("large.jsonl", 1024, false).is_err());
}
#[test]
fn source_absence_does_not_substitute_current_definition() {
    let (_tmp, s, path) = fixture();
    fs::remove_file(path.join("workflow.yaml")).unwrap();
    fs::create_dir_all(s.global_dir.join("workflows")).unwrap();
    fs::write(
        s.global_dir.join("workflows/review.yaml"),
        "name: wrong\nsteps: []\n",
    )
    .unwrap();
    let c = capture::capture(&s, ID, WS).unwrap();
    assert_eq!(c.header["workflow_source"]["state"], "missing");
    assert_eq!(c.header["sections"]["steps"]["complete"], false);
    assert!(c.header["sections"]["steps"]["source_total"].is_null());
}
#[test]
fn source_and_owner_limits_reject_instead_of_emitting_owner_placeholder() {
    let (_tmp, s, path) = fixture();
    fs::write(path.join("workflow.yaml"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
    assert_eq!(
        capture::capture(&s, ID, WS).err().unwrap().0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    fs::write(path.join("run.json"), vec![b'x'; 256 * 1024 + 1]).unwrap();
    assert_eq!(
        capture::capture(&s, ID, WS).err().unwrap().0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}
#[test]
fn anchored_inputs_refuse_symlink_components_and_special_files() {
    let (tmp, _s, path) = fixture();
    let target = tmp.path().join("outside");
    fs::write(&target, b"private").unwrap();
    std::os::unix::fs::symlink(&target, path.join("events.jsonl")).unwrap();
    let mut input = input::Input::open(&path).unwrap();
    assert!(input.read("events.jsonl", 1024, true).is_err());
    std::os::unix::fs::symlink(&path, tmp.path().join("link")).unwrap();
    assert!(input::Input::open(&tmp.path().join("link")).is_err());
    assert!(std::process::Command::new("mkfifo")
        .args(["-m", "600"])
        .arg(path.join("pipe"))
        .status()
        .unwrap()
        .success());
    assert!(input.read("pipe", 1024, true).is_err());
}
#[test]
fn captured_pages_remain_identical_after_live_append_or_delete() {
    let (_tmp, s, path) = fixture();
    fs::write(path.join("step_results.jsonl"), lines(&[result(2)])).unwrap();
    let c = capture::capture(&s, ID, WS).unwrap();
    let before = c.page("results", None, None, 50).unwrap();
    fs::write(
        path.join("step_results.jsonl"),
        lines(&[result(2), result(3)]),
    )
    .unwrap();
    assert_eq!(before, c.page("results", None, None, 50).unwrap());
    fs::remove_dir_all(path).unwrap();
    assert_eq!(before, c.page("results", None, None, 50).unwrap());
}
#[tokio::test]
async fn routes_require_exact_owner_local_host_and_retained_snapshot() {
    let (_tmp, s, _path) = fixture();
    let uri = format!("/api/runs/{ID}/observation?ws_id={WS}");
    let (status, header) = request(&s, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        request(&s, &format!("{uri}&host=remote")).await.0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        request(&s, &format!("/api/runs/{ID}/observation?ws_id=ws_other"))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let token = header["snapshot"].as_str().unwrap();
    let page =
        format!("/api/runs/{ID}/observation/page?ws_id={WS}&snapshot={token}&section=results");
    assert_eq!(request(&s, &page).await.0, StatusCode::OK);
    assert_eq!(
        request(&s, &page.replace(WS, "ws_other")).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(&s, &format!("{page}&limit=101")).await.0,
        StatusCode::BAD_REQUEST
    );
    s.workflow_observations.snapshots.lock().unwrap().clear();
    assert_eq!(request(&s, &page).await.0, StatusCode::CONFLICT);
}
#[tokio::test]
async fn capture_admission_and_expired_snapshots_are_explicit() {
    let (_tmp, s, _path) = fixture();
    let permit = s
        .workflow_observations
        .gate
        .clone()
        .acquire_owned()
        .await
        .unwrap();
    let uri = format!("/api/runs/{ID}/observation?ws_id={WS}&refresh=true");
    assert_eq!(request(&s, &uri).await.0, StatusCode::TOO_MANY_REQUESTS);
    drop(permit);
    let mut c = capture::capture(&s, ID, WS).unwrap();
    let token = c.token.clone();
    c.born = Instant::now() - TTL - Duration::from_secs(1);
    s.workflow_observations
        .snapshots
        .lock()
        .unwrap()
        .push(Arc::new(c));
    assert_eq!(
        request(
            &s,
            &format!("/api/runs/{ID}/observation/page?ws_id={WS}&snapshot={token}&section=steps")
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert!(s.workflow_observations.snapshots.lock().unwrap().is_empty());
}
#[test]
fn archive_is_observed_without_restore_and_owner_mismatch_is_not_inferred() {
    let (_tmp, s, path) = fixture();
    let archive = s.run_store.archive_root();
    fs::create_dir_all(&archive).unwrap();
    fs::rename(&path, archive.join(ID)).unwrap();
    let c = capture::capture(&s, ID, WS).unwrap();
    assert_eq!(c.header["archived"], true);
    assert!(!path.exists());
    assert_eq!(
        capture::capture(&s, ID, "ws_other").err().unwrap().0,
        StatusCode::NOT_FOUND
    );
}

#[test]
fn unavailable_parent_cannot_serve_children() {
    let (_tmp, s, path) = fixture();
    let mut r = result(2);
    r["step_id"] = json!("x".repeat(MAX_ROW));
    fs::write(path.join("step_results.jsonl"), lines(&[r])).unwrap();
    let c = capture::capture(&s, ID, WS).unwrap();
    assert_eq!(
        c.page("results", None, None, 50).unwrap()["records"][0]["state"],
        "oversized"
    );
    assert_eq!(
        c.page("items", Some("result:0"), None, 50).unwrap_err().0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        c.page("findings", Some("result:0"), None, 50)
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND
    );
}
#[test]
fn exact_registered_project_local_run_is_supported_without_scanning_other_projects() {
    let (tmp, s, path) = fixture();
    let project = tmp.path().canonicalize().unwrap().join("project");
    fs::create_dir_all(project.join(".rupu/runs")).unwrap();
    fs::rename(&path, project.join(".rupu/runs").join(ID)).unwrap();
    fs::create_dir_all(s.global_dir.join("workspaces")).unwrap();
    let registration =
        json!({"id":WS,"path":project.to_str().unwrap(),"created_at":"2026-11-09T10:00:00Z"});
    let registration: rupu_workspace::Workspace = serde_json::from_value(registration).unwrap();
    fs::write(
        s.global_dir.join("workspaces").join(format!("{WS}.toml")),
        toml::to_string(&registration).unwrap(),
    )
    .unwrap();
    // A directory left by an incomplete global create is not an owning record.
    fs::create_dir_all(&path).unwrap();
    // A malformed unrelated registration must never be scanned.
    fs::write(s.global_dir.join("workspaces/unrelated.toml"), "malformed").unwrap();
    let c = capture::capture(&s, ID, WS).unwrap();
    assert_eq!(c.header["store_kind"], "project_local");
    assert_eq!(c.header["run"]["workspace_id"], WS);
    assert_eq!(
        capture::capture(&s, ID, "ws_other").err().unwrap().0,
        StatusCode::NOT_FOUND
    );
    fs::write(
        s.global_dir.join("workspaces").join(format!("{WS}.toml")),
        "id = 'ws_wrong'\npath = '/fixture'\ncreated_at = 'now'\n",
    )
    .unwrap();
    assert_eq!(
        capture::capture(&s, ID, WS).err().unwrap().0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn finite_cache_evicts_old_observations_without_auto_refresh() {
    let (_tmp, s, _path) = fixture();
    let uri = format!("/api/runs/{ID}/observation?ws_id={WS}&refresh=true");
    let mut first = None;
    for _ in 0..5 {
        let (status, header) = request(&s, &uri).await;
        assert_eq!(status, StatusCode::OK);
        if first.is_none() {
            first = header["snapshot"].as_str().map(str::to_owned);
        }
    }
    assert_eq!(s.workflow_observations.snapshots.lock().unwrap().len(), 4);
    assert_eq!(
        request(
            &s,
            &format!(
                "/api/runs/{ID}/observation/page?ws_id={WS}&snapshot={}&section=results",
                first.unwrap()
            )
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

#[test]
fn large_fanout_aggregate_payload_keeps_exact_parent_and_child_navigation() {
    let (_tmp, s, path) = fixture();
    let mut r = result(3000);
    r["output"] = json!("aggregate".repeat(MAX_ROW));
    r["rendered_prompt"] = json!("prompt".repeat(MAX_ROW));
    r["run_outcome"] =
        json!({"parsed":null,"stdout":"large","stderr":"","exit_code":0,"duration_ms":1});
    fs::write(path.join("step_results.jsonl"), lines(&[r])).unwrap();
    let c = capture::capture(&s, ID, WS).unwrap();
    let p = c.page("results", None, None, 1).unwrap();
    let row = &p["records"][0];
    assert_eq!(row["state"], "available");
    assert_eq!(row["payload_state"], "omitted_oversized");
    assert_eq!(row["items_count"], 3000);
    assert_eq!(
        row["omitted_fields"],
        json!([
            "items",
            "findings",
            "output",
            "rendered_prompt",
            "run_outcome"
        ])
    );
    assert_eq!(row["value"]["run_id"], "run_agent");
    assert_eq!(row["value"]["transcript_path"], "/fixture/agent.jsonl");
    assert!(row["value"].get("output").is_none());
    assert_eq!(
        c.page("items", Some("result:0"), None, 100).unwrap()["captured_total"],
        3000
    );
}

#[test]
fn exact_step_index_preserves_duplicate_attempts_and_binds_cursor_filter() {
    let (_tmp, s, path) = fixture();
    let a = result(1);
    let mut b = result(1);
    b["step_id"] = json!("different");
    let first = lines(&[a.clone()]);
    let middle = lines(&[b]);
    let offset = first.len() + middle.len();
    fs::write(
        path.join("step_results.jsonl"),
        [first, middle, lines(&[a])].concat(),
    )
    .unwrap();
    let c = capture::capture(&s, ID, WS).unwrap();
    let p = c
        .filtered_page("results", None, Some("same-step"), None, 1)
        .unwrap();
    assert_eq!(p["captured_total"], 2);
    assert_eq!(p["source_total"], 2);
    assert_eq!(p["step_id"], "same-step");
    assert_eq!(p["unassigned_rows"], 0);
    let cursor = p["next_cursor"].as_str().unwrap();
    let next = c
        .filtered_page("results", None, Some("same-step"), Some(cursor), 1)
        .unwrap();
    assert_eq!(next["records"][0]["record_id"], format!("result:{offset}"));
    assert!(next["next_cursor"].is_null());
    assert_eq!(
        c.filtered_page("results", None, Some("different"), Some(cursor), 1)
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    assert!(c
        .filtered_page("results", None, None, Some(cursor), 1)
        .is_err());
    assert!(c
        .filtered_page("items", Some("result:0"), Some("same-step"), None, 1)
        .is_err());
    assert!(c.filtered_page("results", None, Some(""), None, 1).is_err());
    assert!(c
        .filtered_page("results", None, Some(&"x".repeat(1025)), None, 1)
        .is_err());
}
#[test]
fn unknown_result_membership_never_becomes_known_zero_filtered_total() {
    let (_tmp, s, path) = fixture();
    let mut bytes = lines(&[result(1)]);
    bytes.extend(b"invalid\n");
    fs::write(path.join("step_results.jsonl"), bytes).unwrap();
    let c = capture::capture(&s, ID, WS).unwrap();
    let p = c
        .filtered_page("results", None, Some("not-recorded"), None, 50)
        .unwrap();
    assert_eq!(p["captured_total"], 0);
    assert_eq!(p["source_complete"], true);
    assert_eq!(p["unassigned_rows"], 1);
    assert!(p["source_total"].is_null());
    let known = c
        .filtered_page("results", None, Some("same-step"), None, 50)
        .unwrap();
    assert_eq!(known["captured_total"], 1);
    assert!(known["source_total"].is_null());
    assert_eq!(
        c.page("results", None, None, 50).unwrap()["source_total"],
        2
    );
}
#[tokio::test]
async fn http_step_filter_echoes_exact_selector_and_rejects_cross_section_use() {
    let (_tmp, s, path) = fixture();
    fs::write(path.join("step_results.jsonl"), lines(&[result(1)])).unwrap();
    let (_, h) = request(&s, &format!("/api/runs/{ID}/observation?ws_id={WS}")).await;
    let snapshot = h["snapshot"].as_str().unwrap();
    let uri=format!("/api/runs/{ID}/observation/page?ws_id={WS}&snapshot={snapshot}&section=results&step_id=same-step");
    let (status, page) = request(&s, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["step_id"], "same-step");
    assert_eq!(page["captured_total"], 1);
    assert_eq!(
        request(&s, &uri.replace("section=results", "section=events"))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[path = "live_tests.rs"]
mod live;
