//! The operator's messaging endpoints (spec F1b §3.5, §3.6):
//! `GET|POST /api/{runs,sessions,agentiflows}/:id/messages`, its SSE
//! stream, the `/steer` alias, and the same calls routed to a remote host.

// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

use std::net::SocketAddr;
use std::path::Path;

use futures_util::TryStreamExt as _;
use rupu_agentiflow::{agentiflow_dir, AgentiflowRecord};
use rupu_fleet::{
    address, Address, Board, BoardPost, Bus, BusCaps, LogLine, LogMessage, Member, OperatorMessage,
    OperatorVia, PostKind,
};
use rupu_orchestrator::runs::{RunStatus, RunStore};
use rupu_runtime::services::operator::MessageRoot;
use serde_json::{json, Value};
use tokio::io::AsyncBufReadExt as _;

const RUN: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0T1MSG";
const DONE: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0T1END";
const SESSION: &str = "ses_01J9ZQ3K4M5N6P7Q8R9S0T1MSG";
const FLOW: &str = "af_01J9ZQ3K4M5N6P7Q8R9S0T1MSG";
const LEGACY_FLOW: &str = "af_01J9ZQ3K4M5N6P7Q8R9S0T1OLD";

/// Any launcher: the CP's "this is `cp serve`" signal, which writes need.
struct NoopLauncher;

#[async_trait::async_trait]
impl rupu_cp::launcher::RunLauncher for NoopLauncher {
    async fn launch(
        &self,
        _req: rupu_cp::launcher::LaunchRequest,
    ) -> Result<String, rupu_cp::launcher::LaunchError> {
        Ok("run_unused".into())
    }
}

fn state(global: &Path, writable: bool) -> rupu_cp::state::AppState {
    let state = rupu_cp::state::AppState::new(global.into(), rupu_config::PricingConfig::default());
    if writable {
        state.with_launcher(Some(std::sync::Arc::new(NoopLauncher)))
    } else {
        state
    }
}

async fn serve(state: rupu_cp::state::AppState) -> SocketAddr {
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn spawn(global: &Path, writable: bool) -> SocketAddr {
    serve(state(global, writable)).await
}

async fn get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let resp = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

async fn post(addr: SocketAddr, path: &str, body: Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

/// A workflow run recorded `status`, whose runner is this process, with
/// `lynx#1` joined in its space.
fn workflow_run(global: &Path, id: &str, status: RunStatus) -> Bus {
    let store = RunStore::new(global.join("runs"));
    let mut rec = super::sse::seed_run(id, status);
    rec.runner_pid = Some(std::process::id());
    store.create(rec, "name: test\nsteps: []\n").unwrap();
    let bus = Bus::open(&global.join("runs").join(id).join("messages"), "lynx#1");
    bus.join(Member {
        agent: "recon".into(),
        run_id: format!("{id}_a"),
        step_id: Some("scan".into()),
        kind: "workflow_step".into(),
        ..Default::default()
    })
    .unwrap();
    bus
}

/// A running agentiflow whose coordinator is this process, its lead joined.
fn live_flow(global: &Path, id: &str) -> std::path::PathBuf {
    let dir = agentiflow_dir(global).join(id);
    let mut rec = flow_record(id, "running");
    rec.runner_pid = Some(std::process::id());
    rec.write(&dir).unwrap();
    Bus::open(&dir, "lead")
        .join(Member {
            agent: "lead".into(),
            run_id: format!("{id}_lead"),
            kind: "flow_lead".into(),
            ..Default::default()
        })
        .unwrap();
    dir
}

fn flow_record(id: &str, status: &str) -> AgentiflowRecord {
    AgentiflowRecord {
        id: id.into(),
        name: "itest".into(),
        engagement_profiles: vec![],
        trigger: rupu_runtime::RunTriggerSource::Agentiflow,
        status: status.into(),
        stop_reason: None,
        rounds: 0,
        goals: vec![],
        started_at: "2026-10-07T00:00:00Z".parse().unwrap(),
        ended_at: None,
        codename: None,
        spent_usd: None,
        spent_tokens: 0,
        runner_pid: None,
    }
}

fn log_lines(space: &Path) -> Vec<LogLine> {
    Bus::open(space, address::OPERATOR)
        .read_log(None, usize::MAX)
        .unwrap()
        .lines
}

fn only_message(space: &Path) -> LogMessage {
    match log_lines(space).as_slice() {
        [LogLine::Msg(m)] => m.clone(),
        other => panic!("{other:?}"),
    }
}

/// `m` as JSON without the fields every send mints afresh.
fn minted_away(m: &LogMessage, also: &[&str]) -> Value {
    let mut v = serde_json::to_value(m).unwrap();
    for key in ["id", "ts"].iter().chain(also) {
        v.as_object_mut().unwrap().remove(*key);
    }
    v
}

#[tokio::test]
async fn messages_view_for_runs_sessions_and_agentiflows() {
    let tmp = tempfile::tempdir().unwrap();
    let g = tmp.path();
    workflow_run(g, RUN, RunStatus::Running)
        .post(PostKind::Note, None, "scan done")
        .unwrap();
    let session = g.join("sessions").join(SESSION);
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(session.join("session.json"), r#"{"status":"active"}"#).unwrap();
    let flow = live_flow(g, FLOW);
    Bus::open(&flow, "lead")
        .send(&Address::Broadcast, "all hands")
        .unwrap();
    let legacy = agentiflow_dir(g).join(LEGACY_FLOW);
    flow_record(LEGACY_FLOW, "completed")
        .write(&legacy)
        .unwrap();
    Board::new(&legacy)
        .post(&BoardPost {
            id: None,
            author: "recon#1".into(),
            ts: "2026-10-07T00:00:01Z".into(),
            kind: PostKind::Observation,
            body: "port 22 open".into(),
            addressed_to: None,
        })
        .unwrap();
    let addr = spawn(g, false).await;

    let (status, run) = get(addr, &format!("/api/runs/{RUN}/messages")).await;
    assert_eq!(status, 200, "{run}");
    assert_eq!(run["space"], "present");
    assert_eq!(run["legacy"], false);
    assert_eq!(run["participants"][0]["address"], "lynx#1");
    assert_eq!(run["participants"][0]["step_id"], "scan");
    assert_eq!(run["messages"][0]["kind"], "post");
    assert_eq!(run["messages"][0]["from"], "lynx#1");
    assert_eq!(run["messages"][0]["body"], "scan done");
    assert_eq!(run["messages"][0]["seen"], json!([]));
    assert_eq!(run["next"], Value::Null);

    let (status, filtered) = get(addr, &format!("/api/runs/{RUN}/messages?kind=operator")).await;
    assert_eq!(status, 200, "{filtered}");
    assert_eq!(filtered["messages"], json!([]));
    let (status, bad) = get(addr, &format!("/api/runs/{RUN}/messages?kind=shout")).await;
    assert_eq!(status, 400, "{bad}");
    assert!(bad["error"].as_str().unwrap().contains("shout"), "{bad}");

    let (status, ses) = get(addr, &format!("/api/sessions/{SESSION}/messages")).await;
    assert_eq!(status, 200, "{ses}");
    assert_eq!(ses["space"], "absent");
    assert_eq!(ses["messages"], json!([]));

    let (status, af) = get(addr, &format!("/api/agentiflows/{FLOW}/messages")).await;
    assert_eq!(status, 200, "{af}");
    assert_eq!(af["legacy"], false);
    assert_eq!(af["messages"][0]["kind"], "broadcast");
    assert_eq!(af["messages"][0]["from"], "lead");

    let (status, old) = get(addr, &format!("/api/agentiflows/{LEGACY_FLOW}/messages")).await;
    assert_eq!(status, 200, "{old}");
    assert_eq!(old["legacy"], true);
    assert_eq!(old["messages"][0]["kind"], "post");
    assert_eq!(old["messages"][0]["from"], "recon#1");
    assert_eq!(old["messages"][0]["post_kind"], "observation");
    assert_eq!(old["messages"][0]["body"], "port 22 open");

    // An id is served by the route its prefix names, whatever it resolves to.
    for path in [
        format!("/api/runs/{SESSION}/messages"),
        format!("/api/sessions/{RUN}/messages"),
        format!("/api/agentiflows/{RUN}/messages"),
        "/api/runs/run_01NOSUCHRUN/messages".to_string(),
    ] {
        let (status, body) = get(addr, &path).await;
        assert_eq!(status, 404, "{path}: {body}");
    }
}

#[tokio::test]
async fn post_is_202_with_its_delivery_and_501_without_cp_serve() {
    let tmp = tempfile::tempdir().unwrap();
    let g = tmp.path();
    workflow_run(g, RUN, RunStatus::Running);
    let space = g.join("runs").join(RUN).join("messages");
    let path = format!("/api/runs/{RUN}/messages");

    let read_only = spawn(g, false).await;
    let (status, body) = post(read_only, &path, json!({ "body": "hold on" })).await;
    assert_eq!(status, 501, "{body}");
    assert!(body["error"].as_str().unwrap().contains("rupu cp serve"));
    assert!(
        log_lines(&space).is_empty(),
        "a refused write writes nothing"
    );

    let addr = spawn(g, true).await;
    let (status, reply) = post(addr, &path, json!({ "body": "hold on", "urgent": true })).await;
    assert_eq!(status, 202, "{reply}");
    assert!(reply["id"].as_str().unwrap().starts_with("msg_"), "{reply}");
    assert_eq!(reply["delivery"]["resolved"], json!(["lynx#1"]));
    let sent = only_message(&space);
    assert_eq!(sent.id, reply["id"]);
    assert_eq!(sent.from, address::OPERATOR);
    assert_eq!(sent.to.as_deref(), Some("broadcast"));
    assert!(sent.urgent);
    assert_eq!(sent.via, Some(OperatorVia::Cp));
    assert_eq!(sent.viewer, None);

    let (status, body) = post(addr, &path, json!({ "to": "two words", "body": "x" })).await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(addr, &path, json!({ "body": "  " })).await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(
        addr,
        &format!("/api/sessions/{RUN}/messages"),
        json!({"body": "x"}),
    )
    .await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(log_lines(&space).len(), 1);
}

#[tokio::test]
async fn send_to_finished_run_409() {
    let tmp = tempfile::tempdir().unwrap();
    let g = tmp.path();
    workflow_run(g, DONE, RunStatus::Completed);
    let space = g.join("runs").join(DONE).join("messages");
    let stopped = agentiflow_dir(g).join(FLOW);
    flow_record(FLOW, "stopped").write(&stopped).unwrap();
    let addr = spawn(g, true).await;

    let (status, body) = post(
        addr,
        &format!("/api/runs/{DONE}/messages"),
        json!({"body": "x"}),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("not running (completed)"),
        "{body}"
    );
    assert!(log_lines(&space).is_empty());

    let (status, body) = post(
        addr,
        &format!("/api/agentiflows/{FLOW}/messages"),
        json!({"body": "x"}),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("not running (stopped)"),
        "{body}"
    );
    assert!(!Bus::open(&stopped, address::OPERATOR).log_path().exists());
}

#[tokio::test]
async fn steer_alias_parity() {
    let tmp = tempfile::tempdir().unwrap();
    let g = tmp.path();
    let flow = live_flow(g, FLOW);
    let addr = spawn(g, true).await;
    let take = || {
        let m = only_message(&flow);
        std::fs::remove_file(Bus::open(&flow, address::OPERATOR).log_path()).unwrap();
        m
    };

    let (status, steered) = post(
        addr,
        &format!("/api/agentiflows/{FLOW}/steer"),
        json!({ "message": "focus on auth", "now": true }),
    )
    .await;
    assert_eq!(status, 202, "{steered}");
    let steer = take();
    assert_eq!(steer.id, steered["id"]);

    let (status, sent) = post(
        addr,
        &format!("/api/agentiflows/{FLOW}/messages"),
        json!({ "to": "lead", "body": "focus on auth", "urgent": true }),
    )
    .await;
    assert_eq!(status, 202, "{sent}");
    let message = take();
    assert_eq!(steered["delivery"], sent["delivery"]);

    // `rupu agentiflow send --now`'s call.
    MessageRoot::resolve(g, FLOW)
        .unwrap()
        .operator_send(
            g,
            OperatorMessage {
                to: Address::Lead,
                body: "focus on auth".into(),
                urgent: true,
                via: OperatorVia::Cli,
                viewer: None,
                client: None,
            },
            BusCaps::default(),
            &rupu_orchestrator::runs::pid_is_running,
        )
        .unwrap();
    let cli = take();

    assert_eq!(minted_away(&steer, &[]), minted_away(&message, &[]));
    assert_eq!(minted_away(&cli, &["via"]), minted_away(&steer, &["via"]));
    assert_eq!(steer.via, Some(OperatorVia::Cp));
    assert_eq!(cli.via, Some(OperatorVia::Cli));
    assert_eq!(steer.to.as_deref(), Some("lead"));
    assert!(steer.urgent);

    // `stop` stays a control-queue request, never a message.
    let (status, body) = post(
        addr,
        &format!("/api/agentiflows/{FLOW}/steer"),
        json!({ "message": "wrap up", "stop": true }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["queued"], true);
    assert!(log_lines(&flow).is_empty());
}

#[tokio::test]
async fn stream_emits_a_line_appended_after_subscribe() {
    let tmp = tempfile::tempdir().unwrap();
    let g = tmp.path();
    let bus = workflow_run(g, RUN, RunStatus::Running);
    let addr = spawn(g, true).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/{RUN}/messages/stream"))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let reader =
        tokio_util::io::StreamReader::new(resp.bytes_stream().map_err(std::io::Error::other));
    let mut lines = tokio::io::BufReader::new(reader).lines();

    let sent = bus.send(&Address::Broadcast, "after subscribe").unwrap();
    let (mut id, mut data) = (None, None);
    while data.is_none() {
        let line = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
            .await
            .expect("a frame within 5s")
            .unwrap()
            .expect("the stream stays open");
        if let Some(v) = line.strip_prefix("id:") {
            id = Some(v.trim().to_string());
        }
        if let Some(v) = line.strip_prefix("data:") {
            data = Some(serde_json::from_str::<Value>(v.trim()).unwrap());
        }
    }
    let data = data.unwrap();
    assert_eq!(id.as_deref(), Some("1"));
    assert_eq!(data["t"], "msg");
    assert_eq!(data["id"], sent.id());
    assert_eq!(data["body"], "after subscribe");
    assert_eq!(data["seen"], json!([]));

    let (status, body) = get(addr, &format!("/api/sessions/{RUN}/messages/stream")).await;
    assert_eq!(status, 404, "{body}");
}

#[tokio::test]
async fn remote_unsupported_is_501() {
    let tmp = tempfile::tempdir().unwrap();
    let g = tmp.path();
    let store = rupu_workspace::HostStore {
        root: g.join("hosts"),
    };
    let (tunnel, _token) = rupu_workspace::enroll_node(&store, "node-a").unwrap();
    let bucket_dir = tempfile::tempdir().unwrap();
    let bucket = rupu_workspace::add_bucket_host(
        &store,
        "drop",
        &format!("file://{}", bucket_dir.path().display()),
        None,
    )
    .unwrap();
    let addr = spawn(g, true).await;

    for (transport, host) in [("tunnel", &tunnel.id), ("bucket", &bucket.id)] {
        let (status, body) = get(addr, &format!("/api/runs/{RUN}/messages?host={host}")).await;
        assert_eq!(status, 501, "{transport} GET: {body}");
        let reason = body["error"].as_str().unwrap();
        assert!(
            reason.contains(&format!("a {transport} host has no messaging transport")),
            "{reason}"
        );

        let (status, body) = post(
            addr,
            &format!("/api/runs/{RUN}/messages?host={host}"),
            json!({ "body": "x" }),
        )
        .await;
        assert_eq!(status, 501, "{transport} POST: {body}");
        assert!(body["error"]
            .as_str()
            .unwrap()
            .contains("no messaging transport"));

        let (status, body) = get(
            addr,
            &format!("/api/runs/{RUN}/messages/stream?host={host}"),
        )
        .await;
        assert_eq!(status, 501, "{transport} stream: {body}");
        assert!(
            body["error"].as_str().unwrap().contains("poll GET"),
            "{body}"
        );
    }
}

/// An HTTP host that advertises `run.messages` (a real peer CP) serves its
/// runs' messages through the coordinator: read, sent, refused alike.
#[tokio::test]
async fn http_host_proxies_messages_when_it_advertises_them() {
    let peer_dir = tempfile::tempdir().unwrap();
    workflow_run(peer_dir.path(), RUN, RunStatus::Running)
        .post(PostKind::Note, None, "on the peer")
        .unwrap();
    workflow_run(peer_dir.path(), DONE, RunStatus::Completed);
    let peer = spawn(peer_dir.path(), true).await;

    let tmp = tempfile::tempdir().unwrap();
    let coordinator = state(tmp.path(), true);
    let host = coordinator
        .hosts
        .add_host("peer", &format!("http://{peer}"), None)
        .unwrap()
        .id;
    let addr = serve(coordinator).await;

    let (status, direct) = get(peer, &format!("/api/runs/{RUN}/messages")).await;
    assert_eq!(status, 200);
    let (status, proxied) = get(addr, &format!("/api/runs/{RUN}/messages?host={host}")).await;
    assert_eq!(status, 200, "{proxied}");
    assert_eq!(proxied, direct);
    let (status, page) = get(
        addr,
        &format!("/api/runs/{RUN}/messages?host={host}&kind=operator&limit=5"),
    )
    .await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["messages"], json!([]));

    let (status, reply) = post(
        addr,
        &format!("/api/runs/{RUN}/messages?host={host}"),
        json!({ "to": "lynx#1", "body": "from the coordinator" }),
    )
    .await;
    assert_eq!(status, 202, "{reply}");
    let peer_space = peer_dir.path().join("runs").join(RUN).join("messages");
    let sent = match log_lines(&peer_space).as_slice() {
        [_, LogLine::Msg(m)] => m.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(sent.id, reply["id"]);
    assert_eq!(sent.body, "from the coordinator");
    assert_eq!(sent.via, Some(OperatorVia::Cp));

    let (status, body) = post(
        addr,
        &format!("/api/runs/{DONE}/messages?host={host}"),
        json!({ "body": "x" }),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("not running"),
        "{body}"
    );
    let (status, body) = post(
        addr,
        &format!("/api/runs/{RUN}/messages?host={host}"),
        json!({ "to": "two words", "body": "x" }),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = get(
        addr,
        &format!("/api/runs/run_01NOSUCHRUN/messages?host={host}"),
    )
    .await;
    assert_eq!(status, 404, "{body}");
}

/// A peer whose `/api/host/info` does not list `run.messages` is never asked
/// for messages: the coordinator answers 501 with the reason.
#[tokio::test]
async fn http_host_without_run_messages_is_501() {
    let mock = httpmock::MockServer::start_async().await;
    let info = mock
        .mock_async(|when, then| {
            when.method("GET").path("/api/host/info");
            then.status(200)
                .json_body(json!({ "version": "0.80.0", "features": ["run.list"] }));
        })
        .await;
    let messages = mock
        .mock_async(|when, then| {
            when.path(format!("/api/runs/{RUN}/messages"));
            then.status(200).json_body(json!({}));
        })
        .await;
    let tmp = tempfile::tempdir().unwrap();
    let coordinator = state(tmp.path(), true);
    let host = coordinator
        .hosts
        .add_host("old-peer", &mock.base_url(), None)
        .unwrap()
        .id;
    let addr = serve(coordinator).await;

    let (status, body) = get(addr, &format!("/api/runs/{RUN}/messages?host={host}")).await;
    assert_eq!(status, 501, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("upgrade rupu there"),
        "{body}"
    );
    let (status, body) = post(
        addr,
        &format!("/api/runs/{RUN}/messages?host={host}"),
        json!({ "body": "x" }),
    )
    .await;
    assert_eq!(status, 501, "{body}");
    assert_eq!(messages.hits_async().await, 0);
    assert_eq!(
        info.hits_async().await,
        1,
        "the features it read are remembered"
    );
}
