//! Federation end-to-end test: proves a central CP proxies a real remote host
//! over HTTP using two in-process axum servers in a single test binary.
//!
//! Two independent [`axum`] servers are spun up on ephemeral `127.0.0.1:0` ports:
//! - **Remote** — a plain `AppState`-backed server with one seeded `Running` run.
//! - **Central** — its `HostRegistry` is pre-populated (via `add_host`) to point
//!   at the remote before the server starts. All assertions drive via the central
//!   server's HTTP API using [`reqwest`].

#![deny(clippy::all)]

use chrono::Utc;
use reqwest::StatusCode;
use rupu_orchestrator::runs::{RunRecord, RunStatus, RunStore};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Seed a minimal `session.json` under `<global>/sessions/<id>/`.
fn seed_session(global: &Path, id: &str) {
    let dir = global.join("sessions").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let payload = serde_json::json!({
        "session_id": id,
        "agent_name": "test-agent",
        "model": "claude-sonnet-4-6",
        "provider_name": "anthropic",
        "status": "active",
        "total_turns": 1,
        "total_tokens_in": 0,
        "total_tokens_out": 0,
        "total_tokens_cached": 0,
        "created_at": "2026-06-27T10:00:00Z",
        "updated_at": "2026-06-27T10:00:00Z",
        "workspace_id": "ws_fed"
    });
    std::fs::write(
        dir.join("session.json"),
        serde_json::to_string(&payload).unwrap(),
    )
    .unwrap();
}

/// Build a minimal `RunRecord` with the given `id` and `status`.
fn seed_run(id: &str, status: RunStatus) -> RunRecord {
    RunRecord {
        id: id.into(),
        workflow_name: "fed-workflow".into(),
        status,
        inputs: BTreeMap::from([("prompt".into(), "hello".into())]),
        event: None,
        workspace_id: "ws_fed".into(),
        workspace_path: PathBuf::from("/tmp/fed-proj"),
        transcript_dir: PathBuf::from("/tmp/fed-proj/.rupu/transcripts"),
        started_at: Utc::now(),
        finished_at: None,
        error_message: None,
        awaiting_step_id: None,
        approval_prompt: None,
        awaiting_since: None,
        expires_at: None,
        resume_requested_at: None,
        resume_claimed_at: None,
        resume_claimed_by: None,
        resume_mode: None,
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
        final_output: None,
    }
}

/// Spawn a read-only CP server (no launcher) on an ephemeral port.
/// The provided directory becomes the `global_dir` for the `AppState`.
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

/// Spawn a CP server from an already-built `AppState` on an ephemeral port.
/// Used for the central server so hosts can be pre-registered before the first
/// request arrives.
async fn spawn_server_from_state(state: rupu_cp::state::AppState) -> std::net::SocketAddr {
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

// ── Test ──────────────────────────────────────────────────────────────────────

/// Full federation end-to-end.
///
/// 1. Spin a **remote** CP (in-process axum) with a seeded `Running` workflow run.
/// 2. Register the remote on a **central** CP via `HostRegistry::add_host`
///    (token `None` → no keychain write required; the remote requires no auth).
/// 3. Assert `GET central /api/hosts` lists the remote with `status: "online"`.
/// 4. Assert `GET central /api/runs?host=<remote_id>` returns the seeded run
///    tagged with the correct `host_id`.
/// 5. Assert `POST central /api/runs/<run_id>/cancel?host=<remote_id>` returns
///    `{ "ok": true }`, then verify the cancel reached the remote by reading its
///    `RunStore` directly and asserting `status == Cancelled`.
#[tokio::test]
async fn central_proxies_remote_host() {
    // ── Remote setup ─────────────────────────────────────────────────────────
    let remote_tmp = tempfile::tempdir().unwrap();
    let run_id = "fed_e2e_run_01";

    // Seed a Running run into the remote's RunStore **before** the server starts.
    let remote_store = RunStore::new(remote_tmp.path().join("runs"));
    remote_store
        .create(
            seed_run(run_id, RunStatus::Running),
            "name: fed-wf\nsteps: []\n",
        )
        .unwrap();

    // Spawn the remote server on an ephemeral port.
    let remote_addr = spawn_server(remote_tmp.path()).await;
    let remote_base_url = format!("http://{remote_addr}");

    // ── Central setup ────────────────────────────────────────────────────────
    let central_tmp = tempfile::tempdir().unwrap();
    let central_state = rupu_cp::state::AppState::new(
        central_tmp.path().into(),
        rupu_config::PricingConfig::default(),
    );

    // Register the remote host with no token (avoids keychain interaction in CI).
    // `add_host` writes a `HostStore` record under `central_tmp/hosts/`; the
    // returned `Host.id` is the ulid we use in subsequent `?host=` queries.
    let added = central_state
        .hosts
        .add_host("remote", &remote_base_url, None)
        .expect("add_host should succeed");
    let remote_host_id = added.id.clone();

    // Spawn the central server. The `AppState` was moved in, but since
    // `hosts` is an `Arc<HostRegistry>`, the pre-registered remote host is
    // already visible inside the server — no restart required.
    let central_addr = spawn_server_from_state(central_state).await;
    let client = reqwest::Client::new();

    // ── Assertion 1: GET /api/hosts → remote is online ────────────────────────
    let resp = client
        .get(format!("http://{central_addr}/api/hosts"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let hosts: Vec<serde_json::Value> = resp.json().await.unwrap();

    // Both local and remote must appear.
    assert!(
        hosts.len() >= 2,
        "expected at least local + remote in hosts list, got {hosts:?}"
    );

    let remote_view = hosts
        .iter()
        .find(|h| h["id"].as_str() == Some(remote_host_id.as_str()))
        .expect("remote host should appear in GET /api/hosts");

    assert_eq!(
        remote_view["status"].as_str(),
        Some("online"),
        "remote host should be online (in-process server is reachable); got {remote_view:?}"
    );
    assert_eq!(remote_view["transport_kind"], "http_cp");
    assert_eq!(remote_view["name"], "remote");

    // ── Assertion 2: GET /api/runs?host=<id> → seeded run appears ─────────────
    let resp = client
        .get(format!(
            "http://{central_addr}/api/runs?host={remote_host_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "scoped run list should return 200"
    );

    let runs: Vec<serde_json::Value> = resp.json().await.unwrap();
    let seeded = runs
        .iter()
        .find(|r| r["id"].as_str() == Some(run_id))
        .expect("seeded run should appear in the proxied run list");

    assert_eq!(
        seeded["host_id"].as_str(),
        Some(remote_host_id.as_str()),
        "proxied run rows must carry the remote host_id"
    );
    assert_eq!(
        seeded["status"].as_str(),
        Some("running"),
        "seeded run should be running before cancel"
    );

    // ── Assertion 3: cancel proxies to remote; remote store shows Cancelled ────
    let cancel_resp = client
        .post(format!(
            "http://{central_addr}/api/runs/{run_id}/cancel?host={remote_host_id}"
        ))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        cancel_resp.status(),
        StatusCode::OK,
        "proxied cancel should return 200"
    );

    let cancel_body: serde_json::Value = cancel_resp.json().await.unwrap();
    assert_eq!(
        cancel_body["ok"].as_bool(),
        Some(true),
        "proxied cancel response must include ok:true"
    );
    assert_eq!(
        cancel_body["host_id"].as_str(),
        Some(remote_host_id.as_str()),
        "proxied cancel response must echo the remote host_id"
    );

    // Read the remote's RunStore directly to confirm the cancel arrived.
    // Both `remote_store` and the remote server's internal `AppState.run_store`
    // point to the same directory — the cancel handler wrote the updated status
    // synchronously, so it is visible here without any polling.
    let cancelled = remote_store
        .load(run_id)
        .expect("run should still be loadable after cancel");
    assert_eq!(
        cancelled.status,
        RunStatus::Cancelled,
        "cancel should have reached the remote and marked the run Cancelled"
    );
}

/// Central proxies a remote session (list + detail) and a remote run graph.
///
/// 1. Seed a session on the **remote** (`<global>/sessions/<id>/session.json`).
/// 2. Assert `GET central /api/sessions?host=<remote_id>` lists it tagged
///    `host_id=<remote_id>`.
/// 3. Assert `GET central /api/sessions/<id>?host=<remote_id>` returns it
///    (proxied, 200).
/// 4. Seed a workflow run on the **remote** (via `RunStore::create`).
/// 5. Assert `GET central /api/runs/<run_id>/graph?host=<remote_id>` returns
///    the remote's graph JSON (200 + `run.id` field present).
#[tokio::test]
async fn central_proxies_remote_session_and_graph() {
    // ── Remote setup ──────────────────────────────────────────────────────────
    let remote_tmp = tempfile::tempdir().unwrap();
    let session_id = "fed_e2e_sess_01";
    let graph_run_id = "fed_e2e_graph_run_01";

    // Seed a session on the remote.
    seed_session(remote_tmp.path(), session_id);

    // Seed a run for the graph assertion.
    // Workflow::parse requires at least one step with agent + prompt.
    let remote_store = RunStore::new(remote_tmp.path().join("runs"));
    remote_store
        .create(
            seed_run(graph_run_id, RunStatus::Running),
            "name: fed-wf\nsteps:\n  - id: step1\n    agent: test-agent\n    prompt: hello\n",
        )
        .unwrap();

    // Spawn the remote server on an ephemeral port.
    let remote_addr = spawn_server(remote_tmp.path()).await;
    let remote_base_url = format!("http://{remote_addr}");

    // ── Central setup ─────────────────────────────────────────────────────────
    let central_tmp = tempfile::tempdir().unwrap();
    let central_state = rupu_cp::state::AppState::new(
        central_tmp.path().into(),
        rupu_config::PricingConfig::default(),
    );
    let added = central_state
        .hosts
        .add_host("remote", &remote_base_url, None)
        .expect("add_host should succeed");
    let remote_host_id = added.id.clone();
    let central_addr = spawn_server_from_state(central_state).await;
    let client = reqwest::Client::new();

    // ── Assertion 1: session list proxy ───────────────────────────────────────
    // GET /api/sessions?host=<remote_id> must include the seeded session tagged
    // with the correct host_id.
    let resp = client
        .get(format!(
            "http://{central_addr}/api/sessions?host={remote_host_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "proxied session list should return 200"
    );

    let sessions: Vec<serde_json::Value> = resp.json().await.unwrap();
    let found_session = sessions
        .iter()
        .find(|s| s["session_id"].as_str() == Some(session_id))
        .expect("seeded session should appear in the proxied session list");
    assert_eq!(
        found_session["host_id"].as_str(),
        Some(remote_host_id.as_str()),
        "proxied session rows must carry the remote host_id"
    );

    // ── Assertion 2: session detail proxy ─────────────────────────────────────
    // GET /api/sessions/<id>?host=<remote_id> must return the session object.
    let resp = client
        .get(format!(
            "http://{central_addr}/api/sessions/{session_id}?host={remote_host_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "proxied session detail should return 200"
    );

    let detail: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        detail["session_id"].as_str(),
        Some(session_id),
        "proxied session detail must include the correct session_id"
    );
    assert_eq!(
        detail["agent_name"].as_str(),
        Some("test-agent"),
        "proxied session detail must carry the agent_name"
    );

    // ── Assertion 3: run graph proxy ──────────────────────────────────────────
    // GET /api/runs/<run_id>/graph?host=<remote_id> must forward to the remote
    // and return a recognisable graph object.
    let resp = client
        .get(format!(
            "http://{central_addr}/api/runs/{graph_run_id}/graph?host={remote_host_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "proxied run graph should return 200"
    );

    let graph: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        graph["run"]["id"].as_str(),
        Some(graph_run_id),
        "proxied graph must include the seeded run id in run.id"
    );
    assert!(
        graph.get("workflow").is_some(),
        "proxied graph must include the workflow DAG field"
    );
}
