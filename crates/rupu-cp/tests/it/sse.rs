//! Integration tests for the SSE event-stream endpoints:
//! - `GET /api/runs/:id/log`
//! - `GET /api/events/stream`

// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

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
        customer: None,
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
        awaiting: Vec::new(),
        awaiting_step_id: None,
        approval_prompt: None,
        awaiting_since: None,
        expires_at: None,
        resume_requested_at: None,
        resume_claimed_at: None,
        resume_claimed_by: None,
        resume_mode: None,
        resume_gate_id: None,
        resume_approver: None,
        resume_rerequested_at: None,
        reject_cleanup_pending: None,
        engagement_profiles: Vec::new(),
        permission_mode: None,
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
        loop_progress: Default::default(),
        gate_decisions: Vec::new(),
        codename: None,
        cause: None,
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
            host: None,
            codename: None,
        },
    ]
}

async fn spawn_server(dir: &std::path::Path) -> std::net::SocketAddr {
    let state = rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
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
        .create(
            seed_run(run_id, RunStatus::Running),
            "name: test\nsteps: []\n",
        )
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
    let async_reader = tokio_util::io::StreamReader::new(stream.map_err(std::io::Error::other));
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

    let data = first_data_line.expect("no data: line received within timeout");
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
        .create(
            seed_run(run_id, RunStatus::Running),
            "name: test\nsteps: []\n",
        )
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

/// `/api/events/stream` with no `?run` param multiplexes EVERY active run:
/// events from two concurrent runs both arrive on the single firehose. This is
/// the behaviour the single-run Phase-1 tail could not provide.
#[tokio::test]
async fn events_stream_multiplexes_active_runs() {
    use futures_util::TryStreamExt as _;

    let tmp = tempfile::tempdir().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));

    for run_id in ["mux_run_a", "mux_run_b"] {
        store
            .create(
                seed_run(run_id, RunStatus::Running),
                "name: test\nsteps: []\n",
            )
            .unwrap();
        let events = vec![Event::RunStarted {
            event_version: 1,
            run_id: run_id.into(),
            workflow_path: PathBuf::from("/tmp/wf.yaml"),
            started_at: Utc::now(),
        }];
        write_events_jsonl(&store, run_id, &events);
    }

    let addr = spawn_server(tmp.path()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/events/stream"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let async_reader =
        tokio_util::io::StreamReader::new(resp.bytes_stream().map_err(std::io::Error::other));
    let mut lines = tokio::io::BufReader::new(async_reader).lines();

    // Collect run_ids from data lines until we've seen BOTH runs (or time out).
    let mut seen = std::collections::HashSet::new();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(rid) = v["run_id"].as_str() {
                        seen.insert(rid.to_string());
                    }
                }
            }
            if seen.contains("mux_run_a") && seen.contains("mux_run_b") {
                return;
            }
        }
    })
    .await
    .expect("timed out before both runs' events arrived on the firehose");

    assert!(
        seen.contains("mux_run_a") && seen.contains("mux_run_b"),
        "firehose should carry events from both runs, saw {seen:?}"
    );
}

/// `/api/events/stream` with a seeded run and no `?run` param → auto-selects
/// the run and returns 200 `text/event-stream`.
#[tokio::test]
async fn events_stream_auto_selects_run() {
    let tmp = tempfile::tempdir().unwrap();

    let run_id = "sse_auto_run";
    let store = RunStore::new(tmp.path().join("runs"));
    store
        .create(
            seed_run(run_id, RunStatus::Running),
            "name: test\nsteps: []\n",
        )
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

// ── Resumable, finite one-run streams ────────────────────────────────────────

/// Read a whole SSE body; panics if the server doesn't end it within 5 s.
async fn read_to_end(req: reqwest::RequestBuilder) -> String {
    let resp = req.send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    tokio::time::timeout(std::time::Duration::from_secs(5), resp.text())
        .await
        .expect("a finished run's stream must end on its own")
        .unwrap()
}

/// The `id:` values of every frame, in order.
fn ids(body: &str) -> Vec<u64> {
    body.lines()
        .filter_map(|l| l.strip_prefix("id: ").or_else(|| l.strip_prefix("id:")))
        .map(|v| v.trim().parse().unwrap())
        .collect()
}

fn finished_run(tmp: &std::path::Path, run_id: &str) {
    let store = RunStore::new(tmp.join("runs"));
    store
        .create(
            seed_run(run_id, RunStatus::Completed),
            "name: test\nsteps: []\n",
        )
        .unwrap();
    let mut events = make_events();
    events.push(Event::RunCompleted {
        run_id: run_id.into(),
        status: RunStatus::Completed,
        finished_at: Utc::now(),
    });
    write_events_jsonl(&store, run_id, &events);
}

/// Each event carries its position in `events.jsonl` as its id, and a
/// finished run's stream ends with an `end` event instead of tailing forever
/// — on both one-run endpoints.
#[tokio::test]
async fn a_finished_runs_stream_is_numbered_and_ends() {
    let tmp = tempfile::tempdir().unwrap();
    finished_run(tmp.path(), "run_done");
    let addr = spawn_server(tmp.path()).await;
    let client = reqwest::Client::new();
    for url in [
        format!("http://{addr}/api/runs/run_done/log"),
        format!("http://{addr}/api/events/stream?run=run_done"),
    ] {
        let body = read_to_end(client.get(&url)).await;
        assert_eq!(ids(&body), vec![1, 2, 3], "{url}: {body}");
        // `retry:` with or without the optional space (both are SSE).
        let tail = body.trim_end().replace("retry: ", "retry:");
        assert!(
            tail.ends_with("event: end\nretry:15000\ndata: {}"),
            "{url}: {body}"
        );
    }
}

/// `Last-Event-ID` resumes after that event instead of replaying the run; a
/// resume point at or past the end gets just the `end`.
#[tokio::test]
async fn last_event_id_resumes_after_it() {
    let tmp = tempfile::tempdir().unwrap();
    finished_run(tmp.path(), "run_done");
    let addr = spawn_server(tmp.path()).await;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/api/runs/run_done/log");

    let body = read_to_end(client.get(&url).header("Last-Event-ID", "2")).await;
    assert_eq!(ids(&body), vec![3], "{body}");
    assert!(body.contains("run_completed"), "{body}");
    assert!(!body.contains("run_started"), "{body}");

    for past in ["3", "99"] {
        let body = read_to_end(client.get(&url).header("Last-Event-ID", past)).await;
        assert!(ids(&body).is_empty(), "{body}");
        assert!(body.contains("event: end"), "{body}");
    }
}

/// A run that is not over stays open: a `run_completed` whose record is not
/// terminal (a gate park, a resumed retry) does not end the stream.
#[tokio::test]
async fn a_live_run_stream_stays_open() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));
    store
        .create(
            seed_run("run_live", RunStatus::AwaitingApproval),
            "name: test\nsteps: []\n",
        )
        .unwrap();
    let mut events = make_events();
    events.push(Event::RunCompleted {
        run_id: "run_live".into(),
        status: RunStatus::AwaitingApproval,
        finished_at: Utc::now(),
    });
    write_events_jsonl(&store, "run_live", &events);
    let addr = spawn_server(tmp.path()).await;
    let resp = reqwest::get(format!("http://{addr}/api/runs/run_live/log"))
        .await
        .unwrap();
    // Well past the 2 s quiet period: still open, the run isn't settled.
    let ended = tokio::time::timeout(std::time::Duration::from_secs(4), resp.text()).await;
    assert!(ended.is_err(), "a parked run's stream must stay open");
}

/// The transcript stream numbers its events the same way, resumes after
/// `Last-Event-ID`, and ends after `run_complete`.
#[tokio::test]
async fn a_finished_transcript_stream_is_numbered_resumable_and_ends() {
    use rupu_transcript::{Event as T, RunMode, RunStatus as TStatus};
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("transcripts").join("run_t.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let lines = [
        T::RunStart {
            run_id: "run_t".into(),
            workspace_id: "ws_1".into(),
            agent: "a".into(),
            provider: "anthropic".into(),
            model: "m".into(),
            started_at: Utc::now(),
            mode: RunMode::Ask,
            schema: None,
            system_prompt: None,
            codename: None,
            customer: None,
        },
        T::AssistantMessage {
            content: "hi".into(),
            thinking: None,
        },
        T::RunComplete {
            run_id: "run_t".into(),
            status: TStatus::Ok,
            total_tokens: 1,
            duration_ms: 1,
            error: None,
            outcome: None,
        },
    ]
    .iter()
    .map(|e| serde_json::to_string(e).unwrap())
    .collect::<Vec<_>>()
    .join("\n");
    std::fs::write(&path, lines + "\n").unwrap();
    let canon = std::fs::canonicalize(&path).unwrap();
    let addr = spawn_server(tmp.path()).await;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/api/transcript/stream");
    let req = || client.get(&url).query(&[("path", canon.to_str().unwrap())]);

    let body = read_to_end(req()).await;
    assert_eq!(ids(&body), vec![1, 2, 3], "{body}");
    assert!(body.contains("event: end"), "{body}");

    let body = read_to_end(req().header("Last-Event-ID", "1")).await;
    assert_eq!(ids(&body), vec![2, 3], "{body}");
}

/// An earlier terminal event — a failed attempt the operator retried — is
/// followed by more events, so it doesn't end the stream; and a line this
/// rupu doesn't recognise still counts toward the ids (they are line
/// numbers), so they don't shift between versions.
#[tokio::test]
async fn a_retried_run_streams_past_its_first_failure_and_ids_are_line_numbers() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));
    let run_id = "run_retried";
    store
        .create(
            seed_run(run_id, RunStatus::Completed),
            "name: test\nsteps: []\n",
        )
        .unwrap();
    let line = |e: &Event| serde_json::to_string(e).unwrap();
    let events = make_events();
    let body = [
        line(&events[0]),
        line(&Event::RunFailed {
            run_id: run_id.into(),
            error: "first attempt".into(),
            finished_at: Utc::now(),
        }),
        r#"{"type":"some_future_event","run_id":"run_retried"}"#.to_string(),
        line(&events[1]),
        line(&Event::RunCompleted {
            run_id: run_id.into(),
            status: RunStatus::Completed,
            finished_at: Utc::now(),
        }),
    ]
    .join("\n")
        + "\n";
    std::fs::write(store.events_path(run_id), body).unwrap();
    let addr = spawn_server(tmp.path()).await;
    let body =
        read_to_end(reqwest::Client::new().get(format!("http://{addr}/api/runs/{run_id}/log")))
            .await;
    // Line 3 (unknown) is counted but not sent.
    assert_eq!(ids(&body), vec![1, 2, 4, 5], "{body}");
    assert!(body.contains("run_completed"), "{body}");
    assert!(body.contains("event: end"), "{body}");
}
