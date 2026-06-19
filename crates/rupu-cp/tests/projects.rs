use std::path::Path;

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
