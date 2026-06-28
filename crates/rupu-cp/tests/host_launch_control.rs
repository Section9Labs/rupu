//! Integration tests for Task 9: host-aware launch + run control.
//!
//! Each test:
//! 1. Starts an `httpmock::MockServer` that acts as the "remote" CP.
//! 2. Registers the remote in the local CP's `HostRegistry` via `add_host`.
//! 3. Spins up the local CP server.
//! 4. Issues an HTTP request to the local CP with `host:<remote-id>`.
//! 5. Asserts the mock was called and the response contains `{host_id, run_id}`.

use std::sync::Arc;

use reqwest::StatusCode;
use rupu_cp::{
    agent_launcher::{AgentLaunchError, AgentLaunchRequest, AgentLauncher},
    launcher::{LaunchError, LaunchRequest, RunLauncher},
    session_sender::{SendError, SendMessageRequest, SessionSender},
    session_starter::{SessionStartError, SessionStartRequest, SessionStarter},
    state::AppState,
};

// ── Mock launcher / sender / starter ─────────────────────────────────────────

struct MockLauncher;

#[async_trait::async_trait]
impl RunLauncher for MockLauncher {
    async fn launch(&self, _req: LaunchRequest) -> Result<String, LaunchError> {
        Ok("local_run_id".into())
    }
}

struct MockAgentLauncher;

#[async_trait::async_trait]
impl AgentLauncher for MockAgentLauncher {
    async fn launch(&self, _req: AgentLaunchRequest) -> Result<String, AgentLaunchError> {
        Ok("local_agent_run_id".into())
    }
}

struct MockSessionStarter;

#[async_trait::async_trait]
impl SessionStarter for MockSessionStarter {
    async fn start(&self, _req: SessionStartRequest) -> Result<String, SessionStartError> {
        Ok("local_session_id".into())
    }
}

struct MockSessionSender;

#[async_trait::async_trait]
impl SessionSender for MockSessionSender {
    async fn send(&self, _req: SendMessageRequest) -> Result<String, SendError> {
        Ok("local_send_run_id".into())
    }
}

// ── Server helpers ────────────────────────────────────────────────────────────

/// Spin up a local CP server with all launchers installed.
/// Returns (addr, host_id_of_remote) where the remote is the given httpmock
/// server registered in the local HostRegistry.
async fn spawn_with_remote(
    dir: &std::path::Path,
    remote_url: &str,
) -> (std::net::SocketAddr, String) {
    let state = AppState::new(dir.into(), rupu_config::PricingConfig::default())
        .with_launcher(Some(Arc::new(MockLauncher)))
        .with_agent_launcher(Some(Arc::new(MockAgentLauncher)))
        .with_session_starter(Some(Arc::new(MockSessionStarter)))
        .with_session_sender(Some(Arc::new(MockSessionSender)));

    // Register the remote mock server as a host.
    let host = state
        .hosts
        .add_host("test-remote", remote_url, None)
        .expect("add_host should succeed");
    let host_id = host.id.clone();

    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, host_id)
}

/// Spin up a read-only local CP (no launchers, no remote registered).
async fn spawn_readonly(dir: &std::path::Path) -> std::net::SocketAddr {
    let state = AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

// ── Task 9 tests ─────────────────────────────────────────────────────────────

// ── Launch workflow via remote host ──────────────────────────────────────────

#[tokio::test]
async fn launch_workflow_with_remote_host_proxies_and_returns_host_id() {
    let remote = httpmock::MockServer::start_async().await;
    let m = remote.mock(|when, then| {
        when.method("POST").path("/api/workflows/my-wf/run");
        then.status(200)
            .json_body(serde_json::json!({ "run_id": "remote_run_1", "host_id": "local" }));
    });

    let tmp = tempfile::tempdir().unwrap();
    let (addr, host_id) = spawn_with_remote(tmp.path(), &remote.base_url()).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/workflows/my-wf/run"))
        .json(&serde_json::json!({ "host": host_id }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["run_id"], "remote_run_1");
    assert_eq!(body["host_id"], host_id);
    m.assert();
}

// ── Launch agent via remote host ──────────────────────────────────────────────

#[tokio::test]
async fn launch_agent_with_remote_host_proxies_and_returns_host_id() {
    let remote = httpmock::MockServer::start_async().await;
    let m = remote.mock(|when, then| {
        when.method("POST").path("/api/agents/triage/run");
        then.status(200)
            .json_body(serde_json::json!({ "run_id": "remote_agent_run_1", "host_id": "local" }));
    });

    let tmp = tempfile::tempdir().unwrap();
    let (addr, host_id) = spawn_with_remote(tmp.path(), &remote.base_url()).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/agents/triage/run"))
        .json(&serde_json::json!({ "prompt": "do it", "host": host_id }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["run_id"], "remote_agent_run_1");
    assert_eq!(body["host_id"], host_id);
    m.assert();
}

// ── Start session via remote host ─────────────────────────────────────────────

#[tokio::test]
async fn start_session_with_remote_host_proxies_and_returns_host_id() {
    let remote = httpmock::MockServer::start_async().await;
    let m = remote.mock(|when, then| {
        when.method("POST").path("/api/agents/coder/session");
        then.status(200)
            .json_body(serde_json::json!({ "session_id": "remote_ses_1", "host_id": "local" }));
    });

    let tmp = tempfile::tempdir().unwrap();
    let (addr, host_id) = spawn_with_remote(tmp.path(), &remote.base_url()).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/agents/coder/session"))
        .json(&serde_json::json!({ "mode": "bypass", "host": host_id }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["session_id"], "remote_ses_1");
    assert_eq!(body["host_id"], host_id);
    m.assert();
}

// ── Session send via remote host ──────────────────────────────────────────────

#[tokio::test]
async fn send_session_with_remote_host_proxies_and_returns_host_id() {
    let remote = httpmock::MockServer::start_async().await;
    let m = remote.mock(|when, then| {
        when.method("POST").path("/api/sessions/ses_abc/send");
        then.status(200)
            .json_body(serde_json::json!({ "run_id": "remote_send_run_1", "host_id": "local" }));
    });

    let tmp = tempfile::tempdir().unwrap();
    let (addr, host_id) = spawn_with_remote(tmp.path(), &remote.base_url()).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/sessions/ses_abc/send?host={host_id}"
        ))
        .json(&serde_json::json!({ "prompt": "hello" }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["run_id"], "remote_send_run_1");
    assert_eq!(body["host_id"], host_id);
    m.assert();
}

// ── Cancel run via remote host ────────────────────────────────────────────────

#[tokio::test]
async fn cancel_run_with_remote_host_proxies_and_returns_host_id() {
    let remote = httpmock::MockServer::start_async().await;
    let m = remote.mock(|when, then| {
        when.method("POST").path("/api/runs/run_xyz/cancel");
        then.status(200)
            .json_body(serde_json::json!({ "run": { "id": "run_xyz", "status": "cancelled" } }));
    });

    let tmp = tempfile::tempdir().unwrap();
    let (addr, host_id) = spawn_with_remote(tmp.path(), &remote.base_url()).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/runs/run_xyz/cancel?host={host_id}"
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["host_id"], host_id);
    m.assert();
}

// ── Approve run via remote host ───────────────────────────────────────────────

#[tokio::test]
async fn approve_run_with_remote_host_proxies_and_returns_host_id() {
    let remote = httpmock::MockServer::start_async().await;
    let m = remote.mock(|when, then| {
        when.method("POST").path("/api/runs/run_gate/approve");
        then.status(200).json_body(
            serde_json::json!({ "run": { "id": "run_gate", "status": "awaiting_approval" } }),
        );
    });

    let tmp = tempfile::tempdir().unwrap();
    let (addr, host_id) = spawn_with_remote(tmp.path(), &remote.base_url()).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/runs/run_gate/approve?host={host_id}"
        ))
        .json(&serde_json::json!({ "mode": "bypass" }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["host_id"], host_id);
    m.assert();
}

// ── Reject run via remote host ────────────────────────────────────────────────

#[tokio::test]
async fn reject_run_with_remote_host_proxies_and_returns_host_id() {
    let remote = httpmock::MockServer::start_async().await;
    let m = remote.mock(|when, then| {
        when.method("POST").path("/api/runs/run_gate/reject");
        then.status(200)
            .json_body(serde_json::json!({ "run": { "id": "run_gate", "status": "rejected" } }));
    });

    let tmp = tempfile::tempdir().unwrap();
    let (addr, host_id) = spawn_with_remote(tmp.path(), &remote.base_url()).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/runs/run_gate/reject?host={host_id}"
        ))
        .json(&serde_json::json!({ "reason": "not safe" }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["host_id"], host_id);
    m.assert();
}

// ── Local launch still returns host_id:"local" ────────────────────────────────

#[tokio::test]
async fn local_launch_workflow_returns_run_id_and_local_host_id() {
    let tmp = tempfile::tempdir().unwrap();
    let state = AppState::new(tmp.path().into(), rupu_config::PricingConfig::default())
        .with_launcher(Some(Arc::new(MockLauncher)));
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/workflows/my-wf/run"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["run_id"], "local_run_id");
    assert_eq!(body["host_id"], "local");
}

// ── Read-only 501 local path is unchanged ─────────────────────────────────────

#[tokio::test]
async fn readonly_local_launch_workflow_still_returns_501() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_readonly(tmp.path()).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/workflows/my-wf/run"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn readonly_local_launch_agent_still_returns_501() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_readonly(tmp.path()).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/agents/triage/run"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
}

// ── Unknown host → 404 ───────────────────────────────────────────────────────

#[tokio::test]
async fn unknown_host_in_launch_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let state = AppState::new(tmp.path().into(), rupu_config::PricingConfig::default())
        .with_launcher(Some(Arc::new(MockLauncher)));
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/workflows/my-wf/run"))
        .json(&serde_json::json!({ "host": "host_nonexistent" }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_host_in_cancel_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_readonly(tmp.path()).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/runs/run_x/cancel?host=host_nonexistent"
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
