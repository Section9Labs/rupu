use std::path::Path;

async fn spawn_server(dir: &Path) -> std::net::SocketAddr {
    let state = rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn seed_workspace_toml(dir: &Path, id: &str, path: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let toml = format!("id = \"{id}\"\npath = \"{path}\"\ncreated_at = \"2026-06-19T00:00:00Z\"\n");
    std::fs::write(dir.join(format!("{id}.toml")), toml).unwrap();
}

/// Seed a coverage target dir with concerns.jsonl + findings.jsonl under
/// `<proj>/.rupu/coverage/<target>/`.
fn seed_coverage_target(proj: &Path, target: &str) {
    let dir = proj.join(".rupu").join("coverage").join(target);
    std::fs::create_dir_all(&dir).unwrap();

    let concerns = "\
{\"concern_id\":\"ssrf\",\"file_path\":\"src/a.rs\",\"status\":\"clean\"}\n\
{\"concern_id\":\"xss\",\"file_path\":\"src/b.rs\",\"status\":\"clean\"}\n";
    std::fs::write(dir.join("concerns.jsonl"), concerns).unwrap();

    let finding = "{\"id\":\"f1\",\"file_path\":\"src/a.rs\",\"line_range\":[1,5],\
\"scope\":\"line\",\"summary\":\"thing\",\"severity\":\"high\",\
\"concern_id\":\"ssrf\",\
\"evidence\":{\"rationale\":\"because\"},\
\"declared_by\":{\"run_id\":\"run_x\",\"model\":\"claude\",\"surface\":\"workflow\"},\
\"declared_at\":\"2026-06-19T00:00:00Z\"}\n";
    std::fs::write(dir.join("findings.jsonl"), finding).unwrap();

    // Per-file ledger: a Read + an Edit on src/a.rs → one FileView, strongest=edit.
    let files = "\
{\"kind\":\"read\",\"path\":\"src/a.rs\",\"line_range\":[1,40],\"tool\":\"read_file\",\
\"run_id\":\"run_x\",\"model\":\"claude\",\"surface\":\"workflow\",\"at\":\"2026-06-19T00:00:00Z\"}\n\
{\"kind\":\"edit\",\"path\":\"src/a.rs\",\"line_range\":[10,12],\"lines_changed\":3,\"tool\":\"edit_file\",\
\"run_id\":\"run_x\",\"model\":\"claude\",\"surface\":\"workflow\",\"at\":\"2026-06-19T00:01:00Z\"}\n";
    std::fs::write(dir.join("files.jsonl"), files).unwrap();
}

/// GET /api/coverage aggregates targets across ALL registered workspaces,
/// each row carrying the right ws_id/project.
#[tokio::test]
async fn list_coverage_aggregates_all_workspaces() {
    let tmp = tempfile::tempdir().unwrap();
    let proj_a = tmp.path().join("proj_a");
    let proj_b = tmp.path().join("proj_b");
    std::fs::create_dir_all(&proj_a).unwrap();
    std::fs::create_dir_all(&proj_b).unwrap();

    let ws_dir = tmp.path().join("workspaces");
    seed_workspace_toml(&ws_dir, "ws_a", proj_a.to_str().unwrap());
    seed_workspace_toml(&ws_dir, "ws_b", proj_b.to_str().unwrap());

    seed_coverage_target(&proj_a, "tgt");
    seed_coverage_target(&proj_b, "tgt");

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/coverage"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");

    assert!(
        arr.len() >= 2,
        "expected targets from both projects (>=2); got {arr:?}"
    );

    // Each ws should be represented exactly once (one target each).
    let ws_ids: Vec<&str> = arr.iter().filter_map(|r| r["ws_id"].as_str()).collect();
    assert!(ws_ids.contains(&"ws_a"), "ws_a missing; got {ws_ids:?}");
    assert!(ws_ids.contains(&"ws_b"), "ws_b missing; got {ws_ids:?}");

    // project basename is attributed per row.
    let row_a = arr
        .iter()
        .find(|r| r["ws_id"].as_str() == Some("ws_a"))
        .expect("ws_a row");
    assert_eq!(row_a["project"].as_str(), Some("proj_a"));
    assert_eq!(row_a["target_id"].as_str(), Some("tgt"));
    assert_eq!(row_a["findings"].as_u64(), Some(1));

    let row_b = arr
        .iter()
        .find(|r| r["ws_id"].as_str() == Some("ws_b"))
        .expect("ws_b row");
    assert_eq!(row_b["project"].as_str(), Some("proj_b"));
}

/// GET /api/coverage with no registry → empty array (not 500).
#[tokio::test]
async fn list_coverage_empty_when_no_registry() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/coverage"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.as_array().map(|a| a.is_empty()).unwrap_or(false));
}

/// GET /api/coverage/:target?ws_id=… resolves the target under that workspace.
#[tokio::test]
async fn get_coverage_resolves_via_ws_id() {
    let tmp = tempfile::tempdir().unwrap();
    let proj_a = tmp.path().join("proj_a");
    let proj_b = tmp.path().join("proj_b");
    std::fs::create_dir_all(&proj_a).unwrap();
    std::fs::create_dir_all(&proj_b).unwrap();

    let ws_dir = tmp.path().join("workspaces");
    seed_workspace_toml(&ws_dir, "ws_a", proj_a.to_str().unwrap());
    seed_workspace_toml(&ws_dir, "ws_b", proj_b.to_str().unwrap());

    // Only proj_b has the target.
    seed_coverage_target(&proj_b, "tgt");

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/coverage/tgt?ws_id=ws_b"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "ws_b/tgt should resolve");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["target_id"].as_str(), Some("tgt"));
    assert_eq!(
        body["findings"].as_array().map(|a| a.len()),
        Some(1),
        "expected one finding in the detail"
    );

    // ws_a has no such target → 404.
    let resp = reqwest::get(format!("http://{addr}/api/coverage/tgt?ws_id=ws_a"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "ws_a/tgt should not resolve");
}

/// GET /api/coverage/:target?ws_id=… returns the per-file heatmap (`files`)
/// alongside the full finding records (severity/summary preserved).
#[tokio::test]
async fn get_coverage_returns_files_and_findings() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();

    let ws_dir = tmp.path().join("workspaces");
    seed_workspace_toml(&ws_dir, "ws_a", proj.to_str().unwrap());
    seed_coverage_target(&proj, "tgt");

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/coverage/tgt?ws_id=ws_a"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();

    assert_eq!(body["ws_id"].as_str(), Some("ws_a"));
    assert_eq!(body["project"].as_str(), Some("proj"));

    // Per-file heatmap: one FileView for src/a.rs, strongest=edit, 1 edit.
    let files = body["files"].as_array().expect("files should be an array");
    assert_eq!(files.len(), 1, "expected one file view; got {files:?}");
    let f = &files[0];
    assert_eq!(f["path"].as_str(), Some("src/a.rs"));
    assert_eq!(f["strongest"].as_str(), Some("edit"));
    assert_eq!(f["edits"].as_u64(), Some(1));

    // Full finding records (not just a count) carry severity + summary.
    let findings = body["findings"].as_array().expect("findings array");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["severity"].as_str(), Some("high"));
    assert_eq!(findings[0]["summary"].as_str(), Some("thing"));
}
