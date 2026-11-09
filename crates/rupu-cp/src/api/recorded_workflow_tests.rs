use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::fs;
use tower::ServiceExt;

const YAML: &str = "name: old-review\nsteps: []\n";
fn fixture() -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let state = AppState::new(
        tmp.path().canonicalize().unwrap(),
        rupu_config::PricingConfig::default(),
    );
    let record: RunRecord =
        serde_json::from_value(json!({"id":"run_recorded","workflow_name":"old-review",
        "status":"completed","inputs":{},"workspace_id":"ws_fixture","workspace_path":"/fixture",
        "transcript_dir":"/fixture/transcripts","started_at":"2026-11-06T10:00:00Z"}))
        .unwrap();
    state.run_store.create(record, YAML).unwrap();
    (tmp, state)
}
async fn request(s: &AppState, method: &str, query: &str) -> (StatusCode, Value) {
    let response = routes()
        .with_state(s.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(format!("/api/runs/run_recorded/workflow-source{query}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}
fn files(path: &FsPath) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut result = std::collections::BTreeMap::new();
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(files(&path));
        } else {
            result.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    result
}
#[tokio::test]
async fn exact_snapshot_is_read_only_and_independent_of_current_definition() {
    let (tmp, s) = fixture();
    fs::create_dir_all(tmp.path().join("workflows")).unwrap();
    fs::write(
        tmp.path().join("workflows/old-review.yaml"),
        "name: replacement\nsteps: []",
    )
    .unwrap();
    let before = files(tmp.path());
    let (status, result) = request(&s, "GET", "?host=local&ws_id=ws_fixture").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["yaml"], YAML);
    assert_eq!(result["run_id"], "run_recorded");
    assert_eq!(result["workspace_id"], "ws_fixture");
    assert_eq!(result["host_id"], "local");
    assert_eq!(result["source_kind"], "run_snapshot");
    assert_eq!(
        result["snapshot_revision"],
        format!("sha256:{:x}", Sha256::digest(YAML.as_bytes()))
    );
    assert!(result.get("revision").is_none());
    assert_eq!(files(tmp.path()), before);
}
#[tokio::test]
async fn archived_snapshot_does_not_restore_run() {
    let (tmp, s) = fixture();
    s.run_store.archive("run_recorded").unwrap();
    let before = files(tmp.path());
    let (status, result) = request(&s, "GET", "?ws_id=ws_fixture").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["archived"], true);
    assert_eq!(result["yaml"], YAML);
    assert_eq!(files(tmp.path()), before);
}
#[tokio::test]
async fn rejects_remote_project_mismatch_missing_owner_and_mutations() {
    let (_tmp, s) = fixture();
    assert_eq!(
        request(&s, "GET", "?host=remote&ws_id=ws_fixture").await.0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        request(&s, "GET", "?host=local&ws_id=ws_other").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&s, "GET", "?host=local").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&s, "GET", "?ws_id=ws_fixture&unexpected=yes")
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&s, "POST", "?ws_id=ws_fixture").await.0,
        StatusCode::METHOD_NOT_ALLOWED
    );
}
#[tokio::test]
async fn missing_snapshot_never_falls_back_and_agent_is_not_workflow() {
    let (tmp, s) = fixture();
    fs::remove_file(s.run_store.workflow_snapshot_path("run_recorded")).unwrap();
    fs::create_dir_all(tmp.path().join("workflows")).unwrap();
    fs::write(tmp.path().join("workflows/old-review.yaml"), YAML).unwrap();
    assert_eq!(
        request(&s, "GET", "?ws_id=ws_fixture").await.0,
        StatusCode::NOT_FOUND
    );
    let path = s.run_store.run_json_path("run_recorded");
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["workflow_name"] = json!("agent:reviewer");
    fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(
        request(&s, "GET", "?ws_id=ws_fixture").await.0,
        StatusCode::BAD_REQUEST
    );
}
#[tokio::test]
async fn bounded_source_record_and_invalid_utf8_fail_closed() {
    let (_tmp, s) = fixture();
    let path = s.run_store.workflow_snapshot_path("run_recorded");
    for (body, status) in [
        (
            vec![b'x'; MAX_SOURCE_BYTES + 1],
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (vec![255], StatusCode::CONFLICT),
        (vec![], StatusCode::NOT_FOUND),
    ] {
        fs::write(&path, body).unwrap();
        assert_eq!(request(&s, "GET", "?ws_id=ws_fixture").await.0, status);
    }
    fs::write(
        s.run_store.run_json_path("run_recorded"),
        vec![b'x'; MAX_RECORD_BYTES + 1],
    )
    .unwrap();
    assert_eq!(
        request(&s, "GET", "?ws_id=ws_fixture").await.0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}
#[cfg(unix)]
#[tokio::test]
async fn symlink_source_or_owner_never_discloses_another_file() {
    let (tmp, s) = fixture();
    let path = s.run_store.workflow_snapshot_path("run_recorded");
    fs::remove_file(&path).unwrap();
    let unrelated = tmp.path().join("unrelated");
    fs::write(&unrelated, "not workflow source").unwrap();
    std::os::unix::fs::symlink(&unrelated, &path).unwrap();
    assert_eq!(
        request(&s, "GET", "?ws_id=ws_fixture").await.0,
        StatusCode::CONFLICT
    );
    let run = s.run_store.root.join("run_recorded");
    let moved = tmp.path().join("moved");
    fs::rename(&run, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &run).unwrap();
    assert_eq!(
        request(&s, "GET", "?ws_id=ws_fixture").await.0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn missing_snapshot_preserves_actual_graph_results_without_fabricated_declaration() {
    let (_tmp, s) = fixture();
    let row: rupu_orchestrator::runs::StepResultRecord = serde_json::from_value(json!({
        "step_id":"inspect","run_id":"run_agent","transcript_path":"/fixture/agent.jsonl",
        "output":"recorded result","success":true,"skipped":false,"rendered_prompt":"inspect",
        "finished_at":"2026-11-06T10:01:00Z"
    })).unwrap();
    s.run_store.append_step_result("run_recorded", &row).unwrap();
    fs::remove_file(s.run_store.workflow_snapshot_path("run_recorded")).unwrap();
    let response = super::super::graph::routes()
        .with_state(s.clone())
        .oneshot(
            Request::builder()
                .uri("/api/runs/run_recorded/graph?host=local")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let graph: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(graph["workflow"]["steps"], json!([]));
    assert_eq!(graph["workflow"]["edges"], json!([]));
    assert_eq!(graph["workflow_source_available"], false);
    assert!(graph["workflow_source_notice"].as_str().is_some());
    assert_eq!(graph["step_results"][0]["run_id"], "run_agent");
    assert_eq!(graph["step_results"][0]["step_id"], "inspect");
    assert_eq!(
        graph["step_results"][0]["transcript_path"],
        "/fixture/agent.jsonl"
    );
    assert_eq!(
        request(&s, "GET", "?ws_id=ws_fixture").await.0,
        StatusCode::NOT_FOUND
    );
}
