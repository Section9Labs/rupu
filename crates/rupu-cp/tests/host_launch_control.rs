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

// ── Archive / restore / delete run via remote host ────────────────────────────

#[tokio::test]
async fn archive_run_with_remote_host_proxies_and_returns_host_id() {
    let remote = httpmock::MockServer::start_async().await;
    let m = remote.mock(|when, then| {
        when.method("POST").path("/api/runs/run_arch/archive");
        then.status(200)
            .json_body(serde_json::json!({ "ok": true, "id": "run_arch", "archived": true }));
    });

    let tmp = tempfile::tempdir().unwrap();
    let (addr, host_id) = spawn_with_remote(tmp.path(), &remote.base_url()).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/runs/run_arch/archive?host={host_id}"
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

#[tokio::test]
async fn restore_run_with_remote_host_proxies_and_returns_host_id() {
    let remote = httpmock::MockServer::start_async().await;
    let m = remote.mock(|when, then| {
        when.method("POST").path("/api/runs/run_arch/restore");
        then.status(200)
            .json_body(serde_json::json!({ "ok": true, "id": "run_arch", "archived": false }));
    });

    let tmp = tempfile::tempdir().unwrap();
    let (addr, host_id) = spawn_with_remote(tmp.path(), &remote.base_url()).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/runs/run_arch/restore?host={host_id}"
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

#[tokio::test]
async fn delete_run_with_remote_host_proxies_and_returns_host_id() {
    let remote = httpmock::MockServer::start_async().await;
    let m = remote.mock(|when, then| {
        when.method("DELETE").path("/api/runs/run_del");
        then.status(200)
            .json_body(serde_json::json!({ "ok": true, "id": "run_del", "deleted": true }));
    });

    let tmp = tempfile::tempdir().unwrap();
    let (addr, host_id) = spawn_with_remote(tmp.path(), &remote.base_url()).await;

    let resp = reqwest::Client::new()
        .delete(format!("http://{addr}/api/runs/run_del?host={host_id}"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["host_id"], host_id);
    m.assert();
}

/// A remote-host archive/restore/delete request must NOT touch the local run
/// store at all — proven by creating a LOCAL run with the same id the remote
/// mock uses and asserting it is untouched (still present, still active)
/// after the proxied call.
#[tokio::test]
async fn archive_run_with_remote_host_does_not_touch_local_run_store() {
    let remote = httpmock::MockServer::start_async().await;
    let m = remote.mock(|when, then| {
        when.method("POST").path("/api/runs/run_shared_id/archive");
        then.status(200)
            .json_body(serde_json::json!({ "ok": true, "id": "run_shared_id", "archived": true }));
    });

    let tmp = tempfile::tempdir().unwrap();
    let state = AppState::new(tmp.path().into(), rupu_config::PricingConfig::default())
        .with_launcher(Some(Arc::new(MockLauncher)));
    let local_store = state.run_store.clone();
    let host = state
        .hosts
        .add_host("test-remote", &remote.base_url(), None)
        .expect("add_host should succeed");
    let host_id = host.id.clone();

    // Seed a LOCAL run under the same id the remote mock uses.
    local_store
        .create(
            local_terminal_record("run_shared_id"),
            "name: x\n",
        )
        .unwrap();

    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/runs/run_shared_id/archive?host={host_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    m.assert();

    // The LOCAL run must be completely untouched — still active, not archived.
    assert_eq!(local_store.list().unwrap().len(), 1, "local run must still be active");
    assert_eq!(local_store.list_archived().unwrap().len(), 0, "local archive scope must stay empty");
}

fn local_terminal_record(id: &str) -> rupu_orchestrator::RunRecord {
    rupu_orchestrator::RunRecord {
        id: id.into(),
        workflow_name: "wf".into(),
        status: rupu_orchestrator::RunStatus::Completed,
        inputs: Default::default(),
        event: None,
        workspace_id: "ws_1".into(),
        workspace_path: std::path::PathBuf::from("/tmp/proj"),
        transcript_dir: std::path::PathBuf::from("/tmp/proj/.rupu/transcripts"),
        started_at: chrono::Utc::now(),
        finished_at: Some(chrono::Utc::now()),
        error_message: None,
        awaiting: Vec::new(),
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
        resume_requested_at: None,
        resume_claimed_at: None,
        resume_claimed_by: None,
        resume_mode: None,
        resume_gate_id: None,
        resume_approver: None,
        reject_cleanup_pending: None,
        permission_mode: None,
        final_output: None,
        loop_progress: Default::default(),
    }
}

// ── Archive / restore / delete session via remote host ────────────────────────

struct StubSessionMutator;

#[async_trait::async_trait]
impl rupu_cp::session_mutator::SessionMutator for StubSessionMutator {
    async fn mutate(
        &self,
        _id: &str,
        _action: rupu_cp::session_mutator::SessionAction,
    ) -> Result<(), rupu_cp::session_mutator::SessionMutateError> {
        Ok(())
    }
}

async fn spawn_with_remote_and_session_mutator(
    dir: &std::path::Path,
    remote_url: &str,
) -> (std::net::SocketAddr, String) {
    let state = AppState::new(dir.into(), rupu_config::PricingConfig::default())
        .with_launcher(Some(Arc::new(MockLauncher)))
        .with_session_mutator(Some(Arc::new(StubSessionMutator)));
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

#[tokio::test]
async fn archive_session_with_remote_host_proxies_and_returns_host_id() {
    let remote = httpmock::MockServer::start_async().await;
    let m = remote.mock(|when, then| {
        when.method("POST").path("/api/sessions/sess_arch/archive");
        then.status(200)
            .json_body(serde_json::json!({ "ok": true, "id": "sess_arch" }));
    });

    let tmp = tempfile::tempdir().unwrap();
    let (addr, host_id) = spawn_with_remote_and_session_mutator(tmp.path(), &remote.base_url()).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/sessions/sess_arch/archive?host={host_id}"
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

#[tokio::test]
async fn delete_session_with_remote_host_proxies_and_returns_host_id() {
    let remote = httpmock::MockServer::start_async().await;
    let m = remote.mock(|when, then| {
        when.method("DELETE").path("/api/sessions/sess_del");
        then.status(200)
            .json_body(serde_json::json!({ "ok": true, "id": "sess_del" }));
    });

    let tmp = tempfile::tempdir().unwrap();
    let (addr, host_id) = spawn_with_remote_and_session_mutator(tmp.path(), &remote.base_url()).await;

    let resp = reqwest::Client::new()
        .delete(format!("http://{addr}/api/sessions/sess_del?host={host_id}"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["host_id"], host_id);
    m.assert();
}

/// Local (absent `?host=`) archive/delete-session still dispatches through
/// the local `SessionMutator` port, completely bypassing the remote — proof
/// the back-compat local path is untouched by the new host-threading.
#[tokio::test]
async fn archive_session_absent_host_still_uses_local_mutator_not_remote() {
    let remote = httpmock::MockServer::start_async().await;
    // No mock registered — if the local path accidentally proxied, the
    // remote's default httpmock behavior (connection accepted, 404-ish
    // response) would surface as an error instead of `ok: true`.
    let tmp = tempfile::tempdir().unwrap();
    let (addr, _host_id) = spawn_with_remote_and_session_mutator(tmp.path(), &remote.base_url()).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/sessions/sess_local/archive"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    // No host_id key on the local branch — unlike the remote branch above.
    assert!(body.get("host_id").is_none());
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

#[tokio::test]
async fn unknown_host_in_archive_run_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_readonly(tmp.path()).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/runs/run_x/archive?host=host_nonexistent"
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_host_in_delete_run_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_readonly(tmp.path()).await;

    let resp = reqwest::Client::new()
        .delete(format!("http://{addr}/api/runs/run_x?host=host_nonexistent"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_host_in_archive_session_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_readonly(tmp.path()).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/sessions/sess_x/archive?host=host_nonexistent"
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
