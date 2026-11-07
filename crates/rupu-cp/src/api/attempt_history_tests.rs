use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use rupu_orchestrator::runs::RunRecord;
use serde_json::{json, Value};
use std::fs;
use tower::ServiceExt;

fn artifact_bytes(root: &std::path::Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(artifact_bytes(&path));
        } else {
            out.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    out
}

fn fixture() -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let state = AppState::new(tmp.path().into(), rupu_config::PricingConfig::default());
    add_run(&state, "run_workflow", "example");
    (tmp, state)
}

fn add_run(state: &AppState, id: &str, name: &str) {
    let record: RunRecord = serde_json::from_value(json!({
        "id":id, "workflow_name":name, "status":"completed", "inputs":{},
        "workspace_id":"ws_fixture", "workspace_path":"/fixture",
        "transcript_dir":"/nonexistent/transcripts", "started_at":"2026-11-06T10:00:00Z"
    }))
    .unwrap();
    state
        .run_store
        .create(record, "name: example\nsteps: []\n")
        .unwrap();
}

fn row(id: &str, unit: Option<usize>, predecessor: Option<&str>) -> AttemptRecord {
    AttemptRecord {
        v: 1,
        step_id: "review".into(),
        unit_index: unit,
        sub_id: None,
        agent_run_id: id.into(),
        transcript_path: format!("/nonexistent/{id}.jsonl").into(),
        host: None,
        continued_from: predecessor.map(str::to_owned),
        started_at: "2026-11-06T10:00:01Z".parse().unwrap(),
    }
}

async fn get(state: &AppState, uri: &str) -> (StatusCode, Value) {
    let response = routes()
        .merge(super::super::host_info::routes())
        .with_state(state.clone())
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn attempt_history_preserves_attempts_and_exact_nullable_dto() {
    let (_tmp, state) = fixture();
    for record in [
        row("run_initial", Some(0), None),
        row("run_restart", Some(0), None),
        row("run_continued", Some(0), Some("run_restart")),
        row("run_other", Some(1), None),
    ] {
        state
            .run_store
            .append_attempt("run_workflow", &record)
            .unwrap();
    }
    let before = artifact_bytes(&state.global_dir);
    let (status, page) = get(
        &state,
        "/api/runs/run_workflow/attempts?step_id=review&unit_index=0&limit=2",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["recorded_matching_total"], 3);
    assert_eq!(
        page["rows"][0],
        json!({
            "ledger_line":1, "step_id":"review", "unit_index":0, "sub_id":null,
            "agent_run_id":"run_initial", "host":null, "continued_from":null,
            "transcript_path":"/nonexistent/run_initial.jsonl", "started_at":"2026-11-06T10:00:01Z"
        })
    );
    assert_eq!(page["rows"][1]["agent_run_id"], "run_restart");
    let cursor = page["next_cursor"].as_str().unwrap();
    let (_, tail) = get(
        &state,
        &format!("/api/runs/run_workflow/attempts?step_id=review&unit_index=0&cursor={cursor}"),
    )
    .await;
    assert_eq!(tail["snapshot"], page["snapshot"]);
    assert_eq!(tail["rows"][0]["agent_run_id"], "run_continued");
    assert_eq!(tail["rows"][0]["continued_from"], "run_restart");
    assert!(tail["next_cursor"].is_null());
    assert_eq!(artifact_bytes(&state.global_dir), before);
    assert!(state.launcher.is_none());
}

#[tokio::test]
async fn attempt_history_binds_cursor_to_owner_run_filters_and_mac() {
    let (_tmp, state) = fixture();
    let (_other_tmp, other) = fixture();
    add_run(&state, "run_other", "example");
    for id in ["run_one", "run_two"] {
        state
            .run_store
            .append_attempt("run_workflow", &row(id, Some(0), None))
            .unwrap();
    }
    let (_, page) = get(&state, "/api/runs/run_workflow/attempts?limit=1").await;
    let cursor = page["next_cursor"].as_str().unwrap();
    for (s, uri, expected) in [
        (
            &other,
            format!("/api/runs/run_workflow/attempts?cursor={cursor}"),
            StatusCode::BAD_REQUEST,
        ),
        (
            &state,
            format!("/api/runs/run_other/attempts?cursor={cursor}"),
            StatusCode::BAD_REQUEST,
        ),
        (
            &state,
            format!("/api/runs/run_workflow/attempts?unit_index=0&cursor={cursor}"),
            StatusCode::BAD_REQUEST,
        ),
        (
            &state,
            format!("/api/runs/run_workflow/attempts?host=remote&cursor={cursor}"),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            &state,
            format!("/api/runs/run_workflow/attempts?cursor={cursor}x"),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(get(s, &uri).await.0, expected, "{uri}");
    }
}

#[tokio::test]
async fn attempt_history_archive_reads_preserve_cursor_and_artifacts() {
    let (_tmp, state) = fixture();
    for id in ["run_one", "run_two"] {
        state
            .run_store
            .append_attempt("run_workflow", &row(id, None, None))
            .unwrap();
    }
    let source = state.run_store.root.join("run_workflow");
    let original: Vec<_> = ["run.json", "workflow.yaml", "attempts.jsonl"]
        .iter()
        .filter(|name| source.join(name).is_file())
        .map(|name| (*name, fs::read(source.join(name)).unwrap()))
        .collect();
    let (_, first) = get(&state, "/api/runs/run_workflow/attempts?limit=1").await;
    let archive = state.run_store.archive_root();
    fs::create_dir_all(&archive).unwrap();
    fs::rename(&source, archive.join("run_workflow")).unwrap();
    let cursor = first["next_cursor"].as_str().unwrap();
    let (status, tail) = get(
        &state,
        &format!("/api/runs/run_workflow/attempts?cursor={cursor}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{tail}");
    assert_eq!(tail["rows"][0]["agent_run_id"], "run_two");
    assert!(!source.exists(), "read must not restore an archived run");
    for (name, bytes) in original {
        assert_eq!(
            fs::read(archive.join("run_workflow").join(name)).unwrap(),
            bytes
        );
    }
}

#[tokio::test]
async fn attempt_history_distinguishes_missing_empty_partial_and_nonworkflow() {
    let (_tmp, state) = fixture();
    let path = state.run_store.root.join("run_workflow/attempts.jsonl");
    // Fresh RunStore runs have an empty ledger; remove it to model legacy data.
    fs::remove_file(&path).unwrap();
    let (_, missing) = get(&state, "/api/runs/run_workflow/attempts").await;
    assert_eq!(missing["evidence"]["ledger"], "missing");
    assert!(missing["recorded_matching_total"].is_null());
    fs::write(&path, b"").unwrap();
    let (_, empty) = get(&state, "/api/runs/run_workflow/attempts").await;
    assert_eq!(empty["evidence"]["ledger"], "present");
    assert_eq!(empty["recorded_matching_total"], 0);
    let mut bytes = serde_json::to_vec(&row("run_real", None, None)).unwrap();
    bytes.extend_from_slice(b"\n{bad}\n{\"unfinished\":");
    fs::write(&path, bytes).unwrap();
    let (_, partial) = get(&state, "/api/runs/run_workflow/attempts").await;
    assert_eq!(partial["evidence"]["parsing"], "partial");
    assert_eq!(partial["evidence"]["malformed_rows"], 1);
    assert_eq!(partial["evidence"]["execution_history_complete"], "unknown");
    assert_eq!(partial["recorded_matching_total"], 1);
    assert!(partial["rows"][0].get("loop_iteration").is_none());
    assert!(partial["rows"][0].get("status").is_none());
    add_run(&state, "run_agent", "agent:example");
    assert_eq!(
        get(&state, "/api/runs/run_agent/attempts").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&state, "/api/runs/run_absent/attempts").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn attempt_history_capability_available_without_execution_adapters() {
    let (_tmp, state) = fixture();
    let (status, info) = get(&state, "/api/host/info").await;
    assert_eq!(status, StatusCode::OK);
    assert!(info["features"]
        .as_array()
        .unwrap()
        .contains(&json!(CAPABILITY)));
    assert!(state.launcher.is_none());
    assert!(state.session_sender.is_none());
    assert_eq!(
        get(&state, "/api/runs/run_workflow/attempts?host=local")
            .await
            .0,
        StatusCode::OK
    );
}

#[cfg(unix)]
#[tokio::test]
async fn attempt_history_rejects_ledger_symlink_without_reading_target() {
    let (tmp, state) = fixture();
    let outside = tmp.path().join("unrelated");
    fs::write(&outside, b"protected source").unwrap();
    fs::remove_file(state.run_store.root.join("run_workflow/attempts.jsonl")).unwrap();
    std::os::unix::fs::symlink(
        &outside,
        state.run_store.root.join("run_workflow/attempts.jsonl"),
    )
    .unwrap();
    assert_eq!(
        get(&state, "/api/runs/run_workflow/attempts").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(fs::read(outside).unwrap(), b"protected source");
}
