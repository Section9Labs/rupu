use std::path::Path;

use chrono::Utc;
use rupu_orchestrator::runs::{RunRecord, RunStatus, RunStore};
use std::collections::BTreeMap;

async fn spawn_server(dir: &Path) -> std::net::SocketAddr {
    let state =
        rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn seed_workspace_toml(dir: &Path, id: &str, path: &str, created_at: &str, last_run_at: Option<&str>) {
    std::fs::create_dir_all(dir).unwrap();
    let last_run_line = match last_run_at {
        Some(ts) => format!("\nlast_run_at = \"{ts}\""),
        None => String::new(),
    };
    let toml = format!(
        "id = \"{id}\"\npath = \"{path}\"\ncreated_at = \"{created_at}\"{last_run_line}\n"
    );
    std::fs::write(dir.join(format!("{id}.toml")), toml).unwrap();
}

/// Build a RunRecord scoped to `ws_id` with the given id + status, rooted at
/// `proj_path` (the project dir the coverage data lives under).
fn seed_scoped_run(
    id: &str,
    ws_id: &str,
    proj_path: &Path,
    status: RunStatus,
) -> RunRecord {
    RunRecord {
        id: id.into(),
        workflow_name: "test-workflow".into(),
        status,
        inputs: BTreeMap::new(),
        event: None,
        workspace_id: ws_id.into(),
        workspace_path: proj_path.to_path_buf(),
        transcript_dir: proj_path.join(".rupu/transcripts"),
        started_at: Utc::now(),
        finished_at: if status == RunStatus::Running {
            None
        } else {
            Some(Utc::now())
        },
        error_message: None,
        awaiting_step_id: None,
        approval_prompt: None,
        awaiting_since: None,
        expires_at: None,
        issue_ref: None,
        issue: None,
        parent_run_id: None,
        backend_id: None,
        worker_id: None,
        artifact_manifest_path: None,
        runner_pid: None,
        source_wake_id: None,
        active_step_id: None,
        active_step_kind: None,
        active_step_agent: None,
        active_step_transcript_path: None,
    }
}

/// Seed a coverage target dir with a concerns.jsonl + findings.jsonl under
/// `<proj>/.rupu/coverage/<target>/`. No catalog.yaml is written, so an audit
/// of this target finds zero concerns and `assessed_pct` stays null.
fn seed_coverage_target(proj: &Path, target: &str) {
    let dir = proj.join(".rupu").join("coverage").join(target);
    std::fs::create_dir_all(&dir).unwrap();

    // A couple of concern-assertion lines (cheap activity signal).
    let concerns = "\
{\"concern_id\":\"ssrf\",\"file_path\":\"src/a.rs\",\"status\":\"clean\"}\n\
{\"concern_id\":\"xss\",\"file_path\":\"src/b.rs\",\"status\":\"clean\"}\n";
    std::fs::write(dir.join("concerns.jsonl"), concerns).unwrap();

    // One finding line in the FindingRecord shape.
    let finding = "{\"id\":\"f1\",\"file_path\":\"src/a.rs\",\"line_range\":[1,5],\
\"scope\":\"line\",\"summary\":\"thing\",\"severity\":\"high\",\
\"concern_id\":\"ssrf\",\
\"evidence\":{\"rationale\":\"because\"},\
\"declared_by\":{\"run_id\":\"run_x\",\"model\":\"claude\",\"surface\":\"workflow\"},\
\"declared_at\":\"2026-06-19T00:00:00Z\"}\n";
    std::fs::write(dir.join("findings.jsonl"), finding).unwrap();
}

/// GET /api/projects/:ws_id rollup: runs bucketed, coverage aggregated.
#[tokio::test]
async fn get_project_rollup_aggregates_runs_and_coverage() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();

    // Workspace record pointing at the real project dir.
    seed_workspace_toml(
        &tmp.path().join("workspaces"),
        "ws_test",
        proj.to_str().unwrap(),
        "2026-06-19T00:00:00Z",
        None,
    );

    // Two runs scoped to ws_test: one Running, one Completed.
    let store = RunStore::new(tmp.path().join("runs"));
    store
        .create(
            seed_scoped_run("run_a", "ws_test", &proj, RunStatus::Running),
            "name: test-workflow\nsteps: []\n",
        )
        .unwrap();
    store
        .create(
            seed_scoped_run("run_b", "ws_test", &proj, RunStatus::Completed),
            "name: test-workflow\nsteps: []\n",
        )
        .unwrap();

    // A coverage target with concerns + one finding.
    seed_coverage_target(&proj, "tgt");

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/projects/ws_test"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();

    assert_eq!(body["runs"]["total"].as_u64(), Some(2));
    assert_eq!(body["runs"]["running"].as_u64(), Some(1));
    assert_eq!(
        body["recent_runs"].as_array().map(|a| a.len()),
        Some(2),
        "recent_runs should carry both scoped runs"
    );

    assert!(
        body["coverage"]["targets"].as_u64().unwrap_or(0) >= 1,
        "expected at least one coverage target; got {:?}",
        body["coverage"]["targets"]
    );
    assert!(
        body["coverage"]["findings"].as_u64().unwrap_or(0) >= 1,
        "expected at least one finding; got {:?}",
        body["coverage"]["findings"]
    );
    // No catalog seeded → assessed_pct is null. (Documenting the contract.)
    assert!(
        body["coverage"]["assessed_pct"].is_null(),
        "no-catalog target should leave assessed_pct null; got {:?}",
        body["coverage"]["assessed_pct"]
    );

    // project sub-object echoes the row shape.
    assert_eq!(body["project"]["ws_id"].as_str(), Some("ws_test"));
    assert_eq!(body["project"]["name"].as_str(), Some("proj"));
}

/// GET /api/projects/:ws_id for an unknown id → 404.
#[tokio::test]
async fn get_project_unknown_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/projects/unknown"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

/// GET /api/projects/:ws_id/runs → only the scoped runs.
#[tokio::test]
async fn get_project_runs_returns_scoped_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();

    seed_workspace_toml(
        &tmp.path().join("workspaces"),
        "ws_test",
        proj.to_str().unwrap(),
        "2026-06-19T00:00:00Z",
        None,
    );

    let store = RunStore::new(tmp.path().join("runs"));
    store
        .create(
            seed_scoped_run("run_a", "ws_test", &proj, RunStatus::Running),
            "name: test-workflow\nsteps: []\n",
        )
        .unwrap();
    store
        .create(
            seed_scoped_run("run_b", "ws_test", &proj, RunStatus::Completed),
            "name: test-workflow\nsteps: []\n",
        )
        .unwrap();
    // A run in a DIFFERENT workspace must not leak in.
    store
        .create(
            seed_scoped_run("run_other", "ws_other", &proj, RunStatus::Completed),
            "name: test-workflow\nsteps: []\n",
        )
        .unwrap();

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/projects/ws_test/runs"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert_eq!(arr.len(), 2, "expected 2 scoped runs; got {arr:?}");
    let ids: Vec<&str> = arr.iter().filter_map(|r| r["id"].as_str()).collect();
    assert!(ids.contains(&"run_a") && ids.contains(&"run_b"));
    assert!(!ids.contains(&"run_other"), "other-workspace run leaked in");
}

/// GET /api/projects returns 200 with the seeded workspace.
/// The `name` field should be the path basename ("proj").
#[tokio::test]
async fn list_projects_returns_seeded_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let ws_dir = tmp.path().join("workspaces");

    seed_workspace_toml(
        &ws_dir,
        "ws_test",
        "/tmp/proj",
        "2026-06-19T00:00:00Z",
        None,
    );

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/projects"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");

    let entry = arr
        .iter()
        .find(|r| r["ws_id"].as_str() == Some("ws_test"))
        .expect("expected to find ws_test in the array");

    assert_eq!(
        entry["name"].as_str(),
        Some("proj"),
        "name should be the path basename"
    );
    assert_eq!(entry["path"].as_str(), Some("/tmp/proj"));
    assert_eq!(
        entry["created_at"].as_str(),
        Some("2026-06-19T00:00:00Z")
    );
}

/// The workspace with a `last_run_at` should sort before the one without.
#[tokio::test]
async fn list_projects_sorts_by_last_run_at_descending() {
    let tmp = tempfile::tempdir().unwrap();
    let ws_dir = tmp.path().join("workspaces");

    // ws_no_run has no last_run_at
    seed_workspace_toml(
        &ws_dir,
        "ws_no_run",
        "/tmp/no-run-proj",
        "2026-06-19T00:00:00Z",
        None,
    );

    // ws_has_run has a recent last_run_at
    seed_workspace_toml(
        &ws_dir,
        "ws_has_run",
        "/tmp/has-run-proj",
        "2026-06-18T00:00:00Z",
        Some("2026-06-19T12:00:00Z"),
    );

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/projects"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert_eq!(arr.len(), 2, "expected two projects; got {}", arr.len());

    // The one with last_run_at should come first.
    assert_eq!(
        arr[0]["ws_id"].as_str(),
        Some("ws_has_run"),
        "workspace with last_run_at should sort first; got {:?}",
        arr[0]["ws_id"]
    );
    assert_eq!(
        arr[1]["ws_id"].as_str(),
        Some("ws_no_run"),
        "workspace without last_run_at should sort last; got {:?}",
        arr[1]["ws_id"]
    );
}

/// When the workspaces directory doesn't exist the endpoint returns an empty array.
#[tokio::test]
async fn list_projects_returns_empty_when_no_registry_dir() {
    let tmp = tempfile::tempdir().unwrap();
    // Intentionally do NOT create the workspaces/ subdirectory.

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/projects"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert!(
        arr.is_empty(),
        "missing workspaces dir should yield empty array; got {arr:?}"
    );
}
