//! Integration tests for the hosts API:
//! - `GET /api/host/info` (host info — from Task 6)
//! - `GET /api/hosts`     (list all hosts with health)
//! - `POST /api/hosts`    (register a remote host)
//! - `DELETE /api/hosts/:id` (remove a host)

use reqwest::StatusCode;
use rupu_cp::launcher::{LaunchError, LaunchRequest, RunLauncher};
use std::sync::Arc;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Minimal mock launcher — just enough to satisfy the read-only guard check.
struct MockLauncher;

#[async_trait::async_trait]
impl RunLauncher for MockLauncher {
    async fn launch(&self, _req: LaunchRequest) -> Result<String, LaunchError> {
        Ok("mock_run_id".into())
    }
}

/// Spawn a read-only CP server (no launcher). Backs tests that expect 501.
async fn spawn_server(dir: &std::path::Path) -> std::net::SocketAddr {
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

/// Spawn a CP server that has a mock launcher installed (`cp serve` mode).
/// Backs tests that exercise write paths (POST /api/hosts).
async fn spawn_server_serve(dir: &std::path::Path) -> std::net::SocketAddr {
    let state =
        rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default())
            .with_launcher(Some(Arc::new(MockLauncher)));
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

// ── Task 6 — host info ────────────────────────────────────────────────────────

/// `GET /api/host/info` returns 200 with version and capabilities.
#[tokio::test]
async fn host_info_returns_version_and_capabilities() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/host/info"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value = resp.json().await.unwrap();

    let version = body
        .get("version")
        .and_then(|v| v.as_str())
        .expect("version field should be a string");
    assert_eq!(version, env!("CARGO_PKG_VERSION"));

    let capabilities = body
        .get("capabilities")
        .expect("capabilities field should exist");

    assert!(
        capabilities.get("backends").is_some(),
        "capabilities should have backends field"
    );
    assert!(
        capabilities.get("scm_hosts").is_some(),
        "capabilities should have scm_hosts field"
    );
    assert!(
        capabilities.get("permission_modes").is_some(),
        "capabilities should have permission_modes field"
    );
}

// ── Task 7 — hosts CRUD + health ──────────────────────────────────────────────

/// `GET /api/hosts` on a fresh server returns exactly one host: `local`, online.
#[tokio::test]
async fn get_hosts_returns_local_online() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/hosts"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let hosts: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert_eq!(hosts.len(), 1, "fresh server should have exactly one host");

    let local = &hosts[0];
    assert_eq!(local["id"], "local");
    assert_eq!(local["status"], "online");
    assert_eq!(local["transport_kind"], "local");
    assert_eq!(local["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(local["active_run_count"], 0);
}

/// `POST /api/hosts` without a launcher (read-only deploy) → 501 Not Implemented.
#[tokio::test]
async fn post_hosts_without_launcher_returns_501() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await; // no launcher

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/api/hosts"))
        .json(&serde_json::json!({
            "name": "prod",
            "base_url": "http://127.0.0.1:1"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
}

/// `POST /api/hosts` with a launcher → 200 with the new host; the host then
/// appears in `GET /api/hosts` as `"offline"` (no remote is running).
#[tokio::test]
async fn post_hosts_with_launcher_adds_host_and_appears_in_list() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server_serve(tmp.path()).await;

    let client = reqwest::Client::new();

    // POST — add the host (no token so keychain is not touched in tests).
    let post_resp = client
        .post(format!("http://{addr}/api/hosts"))
        .json(&serde_json::json!({
            "name": "staging",
            // Port 1 on localhost refuses immediately — safe for tests.
            "base_url": "http://127.0.0.1:1"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(
        post_resp.status(),
        StatusCode::OK,
        "POST /api/hosts should return 200"
    );

    let added: serde_json::Value = post_resp.json().await.unwrap();
    assert_eq!(added["name"], "staging");
    assert_eq!(added["transport_kind"], "http_cp");
    assert_eq!(added["base_url"], "http://127.0.0.1:1");

    let added_id = added["id"].as_str().expect("id should be a string").to_string();
    assert!(
        added_id.starts_with("host_"),
        "id should be a host_ ULID, got {added_id:?}"
    );

    // GET — the host must appear in the list; status is offline (no server).
    let get_resp = client
        .get(format!("http://{addr}/api/hosts"))
        .send()
        .await
        .unwrap();

    assert_eq!(get_resp.status(), StatusCode::OK);

    let hosts: Vec<serde_json::Value> = get_resp.json().await.unwrap();
    assert_eq!(hosts.len(), 2, "list should have local + the new host");

    let new_host = hosts
        .iter()
        .find(|h| h["id"] == added_id)
        .expect("added host should be in the list");

    assert_eq!(new_host["name"], "staging");
    assert_eq!(
        new_host["status"], "offline",
        "unreachable host should be offline, not fail the list"
    );
    assert_eq!(new_host["active_run_count"], 0);
}

/// `DELETE /api/hosts/local` → 400 Bad Request.
#[tokio::test]
async fn delete_local_host_returns_400() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let client = reqwest::Client::new();
    let resp = client
        .delete(format!("http://{addr}/api/hosts/local"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// `DELETE /api/hosts/:id` for an added host → 204; host gone from list after.
#[tokio::test]
async fn delete_added_host_returns_204_and_removes_it() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server_serve(tmp.path()).await;

    let client = reqwest::Client::new();

    // Add a host first.
    let post_resp = client
        .post(format!("http://{addr}/api/hosts"))
        .json(&serde_json::json!({
            "name": "staging",
            "base_url": "http://127.0.0.1:1"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(post_resp.status(), StatusCode::OK);
    let added: serde_json::Value = post_resp.json().await.unwrap();
    let added_id = added["id"].as_str().unwrap().to_string();

    // DELETE the host.
    let del_resp = client
        .delete(format!("http://{addr}/api/hosts/{added_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(del_resp.status(), StatusCode::NO_CONTENT);

    // Confirm it's gone from the list.
    let get_resp = client
        .get(format!("http://{addr}/api/hosts"))
        .send()
        .await
        .unwrap();
    assert_eq!(get_resp.status(), StatusCode::OK);
    let hosts: Vec<serde_json::Value> = get_resp.json().await.unwrap();
    assert_eq!(hosts.len(), 1, "only local should remain after delete");
    assert_eq!(hosts[0]["id"], "local");
}

// ── Task 9 — tunnel node enrollment ──────────────────────────────────────────

/// `POST /api/hosts/node` without a launcher → 501.
#[tokio::test]
async fn post_hosts_node_without_launcher_returns_501() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await; // no launcher

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/api/hosts/node"))
        .json(&serde_json::json!({ "name": "my-node" }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
}

/// `POST /api/hosts/node` with a launcher → 200; response has a non-empty
/// `token`, a `command` that contains `--token`, and the host is a Tunnel
/// host visible in `GET /api/hosts`.
#[tokio::test]
async fn post_hosts_node_enrolls_and_appears_in_list() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server_serve(tmp.path()).await;

    let client = reqwest::Client::new();

    // Enroll the node.
    let post_resp = client
        .post(format!("http://{addr}/api/hosts/node"))
        .json(&serde_json::json!({ "name": "my-test-node" }))
        .send()
        .await
        .unwrap();

    assert_eq!(
        post_resp.status(),
        StatusCode::OK,
        "POST /api/hosts/node should return 200"
    );

    let body: serde_json::Value = post_resp.json().await.unwrap();

    // Token must be non-empty.
    let token = body["token"].as_str().expect("token must be a string");
    assert!(!token.is_empty(), "token must not be empty");

    // Command must contain --token.
    let command = body["command"].as_str().expect("command must be a string");
    assert!(
        command.contains("--token"),
        "command must contain --token, got {command:?}"
    );

    // Host must be a Tunnel transport.
    let host = &body["host"];
    assert_eq!(host["name"], "my-test-node");
    assert_eq!(
        host["transport_kind"], "tunnel",
        "enrolled host must be a tunnel transport"
    );
    assert_eq!(
        host["status"], "offline",
        "freshly enrolled node should be offline"
    );

    let enrolled_id = host["id"].as_str().expect("host.id must be a string").to_string();
    assert!(
        enrolled_id.starts_with("node_"),
        "tunnel host id must start with node_, got {enrolled_id:?}"
    );

    // The enrolled host must appear in GET /api/hosts.
    let list_resp = client
        .get(format!("http://{addr}/api/hosts"))
        .send()
        .await
        .unwrap();

    assert_eq!(list_resp.status(), StatusCode::OK);

    let hosts: Vec<serde_json::Value> = list_resp.json().await.unwrap();
    assert_eq!(hosts.len(), 2, "list should have local + the enrolled node");

    let node_host = hosts
        .iter()
        .find(|h| h["id"] == enrolled_id)
        .expect("enrolled node should appear in GET /api/hosts");

    assert_eq!(node_host["transport_kind"], "tunnel");
    assert_eq!(node_host["name"], "my-test-node");
}
