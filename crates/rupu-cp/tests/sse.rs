//! Integration tests for the SSE event-stream endpoints:
//! - `GET /api/runs/:id/log`
//! - `GET /api/events/stream`

use chrono::Utc;
use reqwest::StatusCode;
use rupu_orchestrator::{
    executor::Event,
    runs::{RunRecord, RunStatus, RunStore, StepKind},
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::io::AsyncBufReadExt as _;

// ── helpers ──────────────────────────────────────────────────────────────────

fn seed_run(id: &str, status: RunStatus) -> RunRecord {
    RunRecord {
        id: id.into(),
        workflow_name: "test-workflow".into(),
        status,
        inputs: BTreeMap::from([("prompt".into(), "hello".into())]),
        event: None,
        workspace_id: "ws_test".into(),
        workspace_path: PathBuf::from("/tmp/test-proj"),
        transcript_dir: PathBuf::from("/tmp/test-proj/.rupu/transcripts"),
        started_at: Utc::now(),
        finished_at: None,
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

fn make_events() -> Vec<Event> {
    vec![
        Event::RunStarted {
            event_version: 1,
            run_id: "sse_test_run".into(),
            workflow_path: PathBuf::from("/tmp/wf.yaml"),
            started_at: Utc::now(),
        },
        Event::StepStarted {
            run_id: "sse_test_run".into(),
            step_id: "step_a".into(),
            kind: StepKind::Linear,
            agent: Some("rupu-agent".into()),
        },
    ]
}

async fn spawn_server(dir: &std::path::Path) -> std::net::SocketAddr {
    let state =
        rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// Write serialised events to the run's `events.jsonl` path.
fn write_events_jsonl(store: &RunStore, run_id: &str, events: &[Event]) {
    let path = store.events_path(run_id);
    let lines: Vec<String> = events
        .iter()
        .map(|e| serde_json::to_string(e).expect("serialize event"))
        .collect();
    std::fs::write(&path, lines.join("\n") + "\n").expect("write events.jsonl");
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// `/api/runs/:id/log` for an unknown run → 404.
#[tokio::test]
async fn run_log_unknown_id_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/unknown-id/log"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// `/api/runs/:id/log` for a known run → 200 `text/event-stream` + real data.
#[tokio::test]
async fn run_log_known_run_streams_events() {
    let tmp = tempfile::tempdir().unwrap();

    let run_id = "sse_test_run";
    let store = RunStore::new(tmp.path().join("runs"));
    store
        .create(seed_run(run_id, RunStatus::Running), "name: test\nsteps: []\n")
        .unwrap();
    // Pre-populate events.jsonl with two events.
    write_events_jsonl(&store, run_id, &make_events());

    let addr = spawn_server(tmp.path()).await;
    let url = format!("http://{addr}/api/runs/{run_id}/log");

    // --- assert content-type ---
    let client = reqwest::Client::new();
    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("text/event-stream"),
        "expected text/event-stream, got {ct:?}"
    );

    // --- read the first SSE data: line with a timeout ---
    let resp2 = client.get(&url).send().await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);

    let stream = resp2.bytes_stream();
    // Collect bytes line-by-line via a small async reader
    use futures_util::TryStreamExt as _;
    let async_reader = tokio_util::io::StreamReader::new(
        stream.map_err(std::io::Error::other),
    );
    let mut lines = tokio::io::BufReader::new(async_reader).lines();

    let first_data_line = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(data) = line.strip_prefix("data: ") {
                return Some(data.to_string());
            }
        }
        None
    })
    .await
    .expect("timed out waiting for first SSE data line");

    let data =
        first_data_line.expect("no data: line received within timeout");
    // Parse back to a JSON value and confirm it contains the expected type.
    let v: serde_json::Value = serde_json::from_str(&data).expect("data line is JSON");
    assert_eq!(
        v["type"].as_str(),
        Some("run_started"),
        "first event should be run_started, got {v}"
    );
}

/// `/api/events/stream` with no runs → 200 `text/event-stream` (idle stream,
/// not an immediate close).
#[tokio::test]
async fn events_stream_no_runs_stays_open() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/events/stream"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("text/event-stream"),
        "expected text/event-stream for idle stream, got {ct:?}"
    );
}

/// `/api/events/stream?run=<id>` with a valid run → streams its events.
#[tokio::test]
async fn events_stream_explicit_run_streams_events() {
    let tmp = tempfile::tempdir().unwrap();

    let run_id = "sse_global_run";
    let store = RunStore::new(tmp.path().join("runs"));
    store
        .create(seed_run(run_id, RunStatus::Running), "name: test\nsteps: []\n")
        .unwrap();

    // Build events with the correct run_id.
    let events = vec![Event::RunStarted {
        event_version: 1,
        run_id: run_id.into(),
        workflow_path: PathBuf::from("/tmp/wf.yaml"),
        started_at: Utc::now(),
    }];
    write_events_jsonl(&store, run_id, &events);

    let addr = spawn_server(tmp.path()).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/events/stream?run={run_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("text/event-stream"),
        "expected text/event-stream, got {ct:?}"
    );
}

/// `/api/events/stream?run=unknown` → 404.
#[tokio::test]
async fn events_stream_explicit_run_unknown_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/events/stream?run=no-such-run"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// `/api/events/stream` with a seeded run and no `?run` param → auto-selects
/// the run and returns 200 `text/event-stream`.
#[tokio::test]
async fn events_stream_auto_selects_run() {
    let tmp = tempfile::tempdir().unwrap();

    let run_id = "sse_auto_run";
    let store = RunStore::new(tmp.path().join("runs"));
    store
        .create(seed_run(run_id, RunStatus::Running), "name: test\nsteps: []\n")
        .unwrap();
    let events = vec![Event::RunStarted {
        event_version: 1,
        run_id: run_id.into(),
        workflow_path: PathBuf::from("/tmp/wf.yaml"),
        started_at: Utc::now(),
    }];
    write_events_jsonl(&store, run_id, &events);

    let addr = spawn_server(tmp.path()).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/events/stream"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("text/event-stream"),
        "expected text/event-stream, got {ct:?}"
    );
}
