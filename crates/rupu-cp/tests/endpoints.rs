/// Integration tests for the five read-only list endpoints:
/// agents, workflows, sessions, workers, coverage.
use rupu_config::PricingConfig;
use rupu_runtime::{WorkerCapabilities, WorkerKind, WorkerRecord};
use rupu_workspace::worker_store::WorkerStore;
use std::path::Path;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

async fn spawn_server(
    global: &Path,
    workspace: &Path,
) -> std::net::SocketAddr {
    let state = rupu_cp::state::AppState::new(global.into(), PricingConfig::default())
        .with_workspace_dir(workspace.into());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

// ---------------------------------------------------------------------------
// A. Agents
// ---------------------------------------------------------------------------

/// Minimal valid agent frontmatter (no unknown fields because the parser
/// uses `#[serde(deny_unknown_fields)]` on the Frontmatter struct).
const MINIMAL_AGENT_MD: &str = "---\nname: foo\ndescription: \"a test agent\"\n---\nYou are foo.\n";

#[tokio::test]
async fn agents_list_returns_seeded_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    std::fs::create_dir_all(global.join("agents")).unwrap();
    std::fs::write(global.join("agents/foo.md"), MINIMAL_AGENT_MD).unwrap();

    let addr = spawn_server(global, global).await;

    let resp = reqwest::get(format!("http://{addr}/api/agents"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("should be array");
    let names: Vec<&str> = arr.iter().filter_map(|v| v["name"].as_str()).collect();
    assert!(names.contains(&"foo"), "expected 'foo' in {names:?}");
}

#[tokio::test]
async fn agents_list_empty_when_no_agents_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path(), tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/agents"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn agent_detail_includes_system_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    std::fs::create_dir_all(global.join("agents")).unwrap();
    std::fs::write(global.join("agents/foo.md"), MINIMAL_AGENT_MD).unwrap();

    let addr = spawn_server(global, global).await;

    let resp = reqwest::get(format!("http://{addr}/api/agents/foo"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"].as_str(), Some("foo"));
    assert!(
        body["system_prompt"].as_str().is_some(),
        "system_prompt field missing"
    );
}

#[tokio::test]
async fn agent_detail_404_for_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path(), tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/agents/no-such-agent"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// ---------------------------------------------------------------------------
// B. Workflows
// ---------------------------------------------------------------------------

const MINIMAL_WORKFLOW_YAML: &str = r#"
name: wf
steps:
  - id: step1
    agent: foo
    prompt: do stuff
"#;

#[tokio::test]
async fn workflows_list_returns_seeded_workflow() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    std::fs::create_dir_all(global.join("workflows")).unwrap();
    std::fs::write(global.join("workflows/wf.yaml"), MINIMAL_WORKFLOW_YAML).unwrap();

    let addr = spawn_server(global, global).await;

    let resp = reqwest::get(format!("http://{addr}/api/workflows"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("should be array");
    let names: Vec<&str> = arr.iter().filter_map(|v| v["name"].as_str()).collect();
    assert!(names.contains(&"wf"), "expected 'wf' in {names:?}");
    let scopes: Vec<&str> = arr.iter().filter_map(|v| v["scope"].as_str()).collect();
    assert!(scopes.iter().all(|&s| s == "global"), "all scopes should be global");
}

#[tokio::test]
async fn workflows_list_empty_when_no_workflows_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path(), tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/workflows"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn workflow_detail_returns_parsed_workflow_and_yaml() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    std::fs::create_dir_all(global.join("workflows")).unwrap();
    std::fs::write(global.join("workflows/wf.yaml"), MINIMAL_WORKFLOW_YAML).unwrap();

    let addr = spawn_server(global, global).await;

    let resp = reqwest::get(format!("http://{addr}/api/workflows/wf"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["workflow"]["name"].as_str(), Some("wf"));
    assert!(body["yaml"].as_str().is_some(), "yaml field missing");
    let steps = body["workflow"]["steps"].as_array().expect("steps array");
    assert_eq!(steps.len(), 1);
}

#[tokio::test]
async fn workflow_detail_404_for_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path(), tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/workflows/no-such-wf"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// ---------------------------------------------------------------------------
// C. Sessions
// ---------------------------------------------------------------------------

fn minimal_session_json(id: &str) -> String {
    serde_json::json!({
        "session_id": id,
        "agent_name": "foo",
        "model": "claude-sonnet-4-6",
        "status": "active",
        "total_turns": 3,
        "created_at": "2026-06-16T00:00:00Z",
        "updated_at": "2026-06-16T01:00:00Z",
        "active_run_id": null,
        "target": null,
        // message_history deliberately absent
    })
    .to_string()
}

#[tokio::test]
async fn sessions_list_returns_active_session_with_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let sess_dir = global.join("sessions").join("sess1");
    std::fs::create_dir_all(&sess_dir).unwrap();
    std::fs::write(sess_dir.join("session.json"), minimal_session_json("sess1")).unwrap();

    let addr = spawn_server(global, global).await;

    let resp = reqwest::get(format!("http://{addr}/api/sessions"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("should be array");
    assert!(!arr.is_empty(), "expected at least one session");
    let item = arr.iter().find(|v| v["session_id"].as_str() == Some("sess1"))
        .expect("sess1 not found");
    assert_eq!(item["scope"].as_str(), Some("active"));
    // message_history must NOT be present
    assert!(item.get("message_history").is_none(), "message_history should not be present");
}

#[tokio::test]
async fn sessions_list_returns_archived_session_with_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let archive_dir = global.join("sessions-archive").join("sess2");
    std::fs::create_dir_all(&archive_dir).unwrap();
    std::fs::write(archive_dir.join("session.json"), minimal_session_json("sess2")).unwrap();

    let addr = spawn_server(global, global).await;

    let resp = reqwest::get(format!("http://{addr}/api/sessions"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().unwrap();
    let item = arr.iter().find(|v| v["session_id"].as_str() == Some("sess2"))
        .expect("sess2 not found in archive");
    assert_eq!(item["scope"].as_str(), Some("archived"));
}

#[tokio::test]
async fn sessions_list_empty_when_no_sessions_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path(), tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/sessions"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn session_detail_404_for_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path(), tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/sessions/does-not-exist"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn session_detail_returns_active_session() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let sess_dir = global.join("sessions").join("sess3");
    std::fs::create_dir_all(&sess_dir).unwrap();
    std::fs::write(sess_dir.join("session.json"), minimal_session_json("sess3")).unwrap();

    let addr = spawn_server(global, global).await;

    let resp = reqwest::get(format!("http://{addr}/api/sessions/sess3"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["session_id"].as_str(), Some("sess3"));
    assert_eq!(body["scope"].as_str(), Some("active"));
    assert!(body.get("message_history").is_none());
}

#[tokio::test]
async fn session_detail_500_for_corrupt_session_json() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    // Directory exists but session.json contains invalid JSON.
    let sess_dir = global.join("sessions").join("sess-corrupt");
    std::fs::create_dir_all(&sess_dir).unwrap();
    std::fs::write(sess_dir.join("session.json"), b"not valid json {{{{").unwrap();

    let addr = spawn_server(global, global).await;

    let resp = reqwest::get(format!("http://{addr}/api/sessions/sess-corrupt"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 500, "corrupt session.json should yield 500, not 404");
}

// ---------------------------------------------------------------------------
// D. Workers
// ---------------------------------------------------------------------------

fn seed_worker(store: &WorkerStore, id: &str) {
    let worker = WorkerRecord {
        version: WorkerRecord::VERSION,
        worker_id: id.to_string(),
        kind: WorkerKind::Cli,
        name: "test-worker".to_string(),
        host: "localhost".to_string(),
        capabilities: WorkerCapabilities::default(),
        registered_at: "2026-06-16T00:00:00Z".to_string(),
        last_seen_at: "2026-06-16T01:00:00Z".to_string(),
    };
    store.save(&worker).unwrap();
}

#[tokio::test]
async fn workers_list_returns_seeded_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let store = WorkerStore {
        root: global.join("autoflows").join("workers"),
    };
    seed_worker(&store, "worker_local_test_cli");

    let addr = spawn_server(global, global).await;

    let resp = reqwest::get(format!("http://{addr}/api/workers"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("should be array");
    let ids: Vec<&str> = arr.iter().filter_map(|v| v["worker_id"].as_str()).collect();
    assert!(
        ids.contains(&"worker_local_test_cli"),
        "expected worker_local_test_cli in {ids:?}"
    );
}

#[tokio::test]
async fn workers_list_empty_when_no_workers_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path(), tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/workers"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// E. Coverage
// ---------------------------------------------------------------------------

/// Register a workspace TOML pointing at `path` so registry-driven coverage
/// aggregation can discover its `.rupu/coverage/`.
fn seed_workspace_toml(global: &Path, id: &str, path: &Path) {
    let dir = global.join("workspaces");
    std::fs::create_dir_all(&dir).unwrap();
    let toml = format!(
        "id = \"{id}\"\npath = \"{}\"\ncreated_at = \"2026-06-16T00:00:00Z\"\n",
        path.to_str().unwrap()
    );
    std::fs::write(dir.join(format!("{id}.toml")), toml).unwrap();
}

fn seed_coverage_target(workspace: &Path, target_id: &str) {
    let paths = rupu_coverage::CoveragePaths::new(workspace, target_id);
    paths.ensure_dir().unwrap();
    // Write two concern assertion lines.
    let line = serde_json::json!({
        "concern_id": "stride:spoofing",
        "file_path": "src/auth.rs",
        "status": "clean",
        "evidence": { "summary": "ok", "line_ranges": [], "finding_ids": [] },
        "run_id": "r1",
        "model": "m",
        "surface": "workflow",
        "declared_at": "2026-06-16T00:00:00Z"
    })
    .to_string();
    std::fs::write(&paths.concerns, format!("{line}\n{line}\n")).unwrap();
}

#[tokio::test]
async fn coverage_list_returns_seeded_target() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let workspace = tmp.path(); // same dir is fine for tests

    seed_coverage_target(workspace, "tgt1");
    seed_workspace_toml(global, "ws_e", workspace);

    let addr = spawn_server(global, workspace).await;

    let resp = reqwest::get(format!("http://{addr}/api/coverage"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("should be array");
    let item = arr
        .iter()
        .find(|v| v["target_id"].as_str() == Some("tgt1"))
        .expect("tgt1 not found");
    assert!(
        item["assertion_lines"].as_u64().unwrap_or(0) > 0,
        "assertion_lines should be > 0"
    );
    assert!(item["findings"].as_u64().is_some());
}

#[tokio::test]
async fn coverage_list_empty_when_no_coverage_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path(), tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/coverage"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn coverage_detail_returns_target_data() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let workspace = tmp.path();

    seed_coverage_target(workspace, "tgt2");
    seed_workspace_toml(global, "ws_e2", workspace);

    let addr = spawn_server(global, workspace).await;

    let resp = reqwest::get(format!("http://{addr}/api/coverage/tgt2?ws_id=ws_e2"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["target_id"].as_str(), Some("tgt2"));
    assert!(body["assertions"].is_array(), "assertions should be array");
    assert!(body["findings"].is_array(), "findings should be array");
}

#[tokio::test]
async fn coverage_detail_404_for_unknown_target() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path(), tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/coverage/no-such-target"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}
