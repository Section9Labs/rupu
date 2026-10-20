//! The `wait` MCP tool: a run settling, a gate parking, new run events and
//! new messages, resumed from a cursor of the SSE streams' line ids.

#![allow(clippy::disallowed_methods)]

use super::mcp::{call_tool, text_of};
use chrono::Utc;
use rupu_cp::mcp::McpOpts;
use rupu_cp::server::{router_with, RouterOpts};
use rupu_fleet::{Address, OperatorMessage, OperatorVia};
use rupu_orchestrator::executor::Event;
use rupu_orchestrator::runs::{AwaitingGate, RunStatus, RunStore, StepKind};
use serde_json::{json, Value};
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

const RUN: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0T1WAT";

/// A CP over `global` serving `/mcp` (no catalog).
async fn spawn(global: &Path) -> SocketAddr {
    let state =
        rupu_cp::state::AppState::new(global.to_path_buf(), rupu_config::PricingConfig::default());
    let server = rupu_cp::mcp::build_server(
        &state,
        &McpOpts {
            catalog: None,
            tools: vec![],
        },
    )
    .unwrap();
    let app = router_with(
        state,
        None,
        RouterOpts {
            web: true,
            mcp: Some(server),
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn store(global: &Path) -> RunStore {
    RunStore::new(global.join("runs"))
}

fn seed(global: &Path, status: RunStatus) -> rupu_orchestrator::runs::RunRecord {
    let rec = super::sse::seed_run(RUN, status);
    store(global)
        .create(rec.clone(), "name: test\nsteps: []\n")
        .unwrap();
    rec
}

fn append_step(global: &Path, step_id: &str) {
    let ev = Event::StepStarted {
        run_id: RUN.into(),
        step_id: step_id.into(),
        kind: StepKind::Linear,
        agent: None,
        host: None,
        codename: None,
    };
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(store(global).events_path(RUN))
        .unwrap();
    writeln!(f, "{}", serde_json::to_string(&ev).unwrap()).unwrap();
}

async fn wait(addr: SocketAddr, args: Value) -> Value {
    call_tool(addr, "wait", args).await
}

fn ids(r: &Value) -> Vec<u64> {
    r["structuredContent"]["events"]
        .as_array()
        .unwrap_or_else(|| panic!("no events: {r}"))
        .iter()
        .map(|e| e["id"].as_u64().unwrap())
        .collect()
}

#[tokio::test]
async fn a_settled_run_fires_at_once() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path(), RunStatus::Completed);
    let addr = spawn(tmp.path()).await;
    let t = Instant::now();
    let r = wait(
        addr,
        json!({"watch": [{"kind": "run_settled", "run": RUN}]}),
    )
    .await;
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    assert_ne!(r["isError"], json!(true), "{r}");
    let s = &r["structuredContent"];
    assert_eq!(s["fired"][0]["kind"], "run_settled", "{r}");
    assert_eq!(s["fired"][0]["index"], 0, "{r}");
    assert_eq!(s["timed_out"], false, "{r}");
}

#[tokio::test]
async fn a_parked_gate_fires_at_once_with_its_gates() {
    let tmp = tempfile::tempdir().unwrap();
    let mut rec = super::sse::seed_run(RUN, RunStatus::AwaitingApproval);
    rec.awaiting = vec![AwaitingGate {
        step_id: "review".into(),
        prompt: Some("ship it?".into()),
        since: Utc::now(),
        expires_at: None,
    }];
    store(tmp.path())
        .create(rec, "name: test\nsteps: []\n")
        .unwrap();
    let addr = spawn(tmp.path()).await;
    let t = Instant::now();
    let r = wait(addr, json!({"watch": [{"kind": "run_gate", "run": RUN}]})).await;
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    let fired = &r["structuredContent"]["fired"][0];
    assert_eq!(fired["kind"], "run_gate", "{r}");
    assert_eq!(fired["detail"]["gates"][0]["step_id"], "review", "{r}");
    assert_eq!(fired["detail"]["gates"][0]["prompt"], "ship it?", "{r}");
}

#[tokio::test]
async fn a_legacy_record_s_gate_fires_too() {
    let tmp = tempfile::tempdir().unwrap();
    let mut rec = super::sse::seed_run(RUN, RunStatus::AwaitingApproval);
    // A record from before `awaiting`: its one gate is the legacy fields.
    rec.awaiting = vec![];
    rec.awaiting_step_id = Some("review".into());
    rec.approval_prompt = Some("ship it?".into());
    store(tmp.path())
        .create(rec, "name: test\nsteps: []\n")
        .unwrap();
    let addr = spawn(tmp.path()).await;
    let t = Instant::now();
    let r = wait(
        addr,
        json!({"watch": [{"kind": "run_gate", "run": RUN}], "timeout_ms": 5000}),
    )
    .await;
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    let fired = &r["structuredContent"]["fired"][0];
    assert_eq!(fired["kind"], "run_gate", "{r}");
    assert_eq!(fired["detail"]["gates"][0]["step_id"], "review", "{r}");
    assert_eq!(fired["detail"]["gates"][0]["prompt"], "ship it?", "{r}");
}

#[tokio::test]
async fn new_events_are_returned_with_their_line_ids_and_the_cursor_resumes() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path(), RunStatus::Running);
    append_step(tmp.path(), "one");
    append_step(tmp.path(), "two");
    let addr = spawn(tmp.path()).await;
    let watch = json!([{"kind": "run_events", "run": RUN}]);
    let r = wait(addr, json!({"watch": watch, "timeout_ms": 5000})).await;
    assert_eq!(ids(&r), vec![1, 2], "{r}");
    let s = &r["structuredContent"];
    let source = format!("run:{RUN}");
    assert_eq!(s["events"][0]["source"], source.as_str(), "{r}");
    assert_eq!(s["events"][1]["event"]["type"], "step_started", "{r}");
    assert_eq!(s["events"][1]["event"]["step_id"], "two", "{r}");
    assert_eq!(s["cursor"], json!({ &source: 2 }), "{r}");
    assert_eq!(s["fired"][0]["kind"], "run_events", "{r}");
    assert_eq!(s["timed_out"], false, "{r}");

    append_step(tmp.path(), "three");
    let r = wait(
        addr,
        json!({"watch": watch, "cursor": s["cursor"], "timeout_ms": 5000}),
    )
    .await;
    assert_eq!(ids(&r), vec![3], "{r}");
    assert_eq!(
        r["structuredContent"]["cursor"],
        json!({ &source: 3 }),
        "{r}"
    );
}

#[tokio::test]
async fn a_types_filter_skips_other_events_but_moves_the_cursor() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path(), RunStatus::Running);
    append_step(tmp.path(), "one");
    let addr = spawn(tmp.path()).await;
    let r = wait(
        addr,
        json!({"watch": [{"kind": "run_events", "run": RUN, "types": ["run_completed"]}],
               "timeout_ms": 600}),
    )
    .await;
    let s = &r["structuredContent"];
    assert_eq!(s["timed_out"], true, "{r}");
    assert_eq!(ids(&r), Vec::<u64>::new(), "{r}");
    assert_eq!(s["cursor"], json!({ format!("run:{RUN}"): 1 }), "{r}");
}

#[tokio::test]
async fn messages_wait_returns_new_operator_lines_and_skips_seen() {
    let tmp = tempfile::tempdir().unwrap();
    let g = tmp.path();
    let bus = super::messages::workflow_run(g, RUN, RunStatus::Running);
    // Line 1: a broadcast; line 2: an operator message; line 3: its receipt.
    bus.send(&Address::Broadcast, "status").unwrap();
    let op = bus
        .operator_send(OperatorMessage {
            to: Address::Broadcast,
            body: "focus".into(),
            urgent: false,
            via: OperatorVia::Cli,
            viewer: None,
            client: None,
        })
        .unwrap();
    bus.mark_seen(&[op.id().to_string()], "lynx#1", 1).unwrap();
    let addr = spawn(g).await;
    let source = format!("messages:{RUN}");

    let r = wait(
        addr,
        json!({"watch": [{"kind": "messages", "root": RUN, "kinds": ["operator"]}],
               "timeout_ms": 5000}),
    )
    .await;
    assert_eq!(ids(&r), vec![2], "{r}");
    let s = &r["structuredContent"];
    assert_eq!(s["events"][0]["source"], source.as_str(), "{r}");
    assert_eq!(s["events"][0]["event"]["id"], op.id(), "{r}");
    assert_eq!(s["events"][0]["event"]["body"], "focus", "{r}");
    assert_eq!(s["fired"][0]["kind"], "messages", "{r}");

    // After the operator line only its receipt follows: nothing to return.
    let r = wait(
        addr,
        json!({"watch": [{"kind": "messages", "root": RUN}],
               "cursor": { &source: 2 }, "timeout_ms": 600}),
    )
    .await;
    let s = &r["structuredContent"];
    assert_eq!(s["timed_out"], true, "{r}");
    assert_eq!(ids(&r), Vec::<u64>::new(), "{r}");
    assert_eq!(s["cursor"], json!({ &source: 3 }), "{r}");
}

/// A run that wrote its last events and settled between two calls: the
/// call that sees it settled still returns those events.
#[tokio::test]
async fn a_settled_run_still_returns_its_unread_events() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path(), RunStatus::Completed);
    for step in ["one", "two", "three"] {
        append_step(tmp.path(), step);
    }
    let addr = spawn(tmp.path()).await;
    let t = Instant::now();
    let r = wait(
        addr,
        json!({"watch": [{"kind": "run_events", "run": RUN},
                         {"kind": "run_settled", "run": RUN}]}),
    )
    .await;
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    let s = &r["structuredContent"];
    let kinds: Vec<&str> = s["fired"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"run_settled"), "{r}");
    assert_eq!(ids(&r), vec![1, 2, 3], "{r}");
    assert_eq!(s["cursor"], json!({ format!("run:{RUN}"): 3 }), "{r}");
    assert_eq!(s["timed_out"], false, "{r}");
}

/// A parked gate fires on every call, but never hides the messages watched
/// beside it: they come back with it and the cursor moves past them.
#[tokio::test]
async fn a_parked_gate_does_not_starve_a_message_watch() {
    let tmp = tempfile::tempdir().unwrap();
    let g = tmp.path();
    let bus = super::messages::workflow_run(g, RUN, RunStatus::AwaitingApproval);
    let mut rec = store(g).load(RUN).unwrap();
    rec.awaiting = vec![AwaitingGate {
        step_id: "review".into(),
        prompt: None,
        since: Utc::now(),
        expires_at: None,
    }];
    store(g).update(&rec).unwrap();
    let op = bus
        .operator_send(OperatorMessage {
            to: Address::Broadcast,
            body: "while you wait".into(),
            urgent: false,
            via: OperatorVia::Cli,
            viewer: None,
            client: None,
        })
        .unwrap();
    let addr = spawn(g).await;
    let source = format!("messages:{RUN}");
    let watch = json!([{"kind": "messages", "root": RUN},
                       {"kind": "run_gate", "run": RUN}]);

    let r = wait(addr, json!({"watch": watch})).await;
    let s = &r["structuredContent"];
    assert_eq!(s["fired"][0]["kind"], "messages", "{r}");
    assert_eq!(s["fired"][1]["kind"], "run_gate", "{r}");
    assert_eq!(ids(&r), vec![1], "{r}");
    assert_eq!(s["events"][0]["event"]["id"], op.id(), "{r}");
    assert_eq!(s["cursor"], json!({ &source: 1 }), "{r}");

    let t = Instant::now();
    let r = wait(addr, json!({"watch": watch, "cursor": s["cursor"]})).await;
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    let s = &r["structuredContent"];
    assert_eq!(s["fired"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(s["fired"][0]["kind"], "run_gate", "{r}");
    assert_eq!(s["events"], json!([]), "{r}");
    assert_eq!(s["cursor"], json!({ &source: 1 }), "{r}");
}

#[tokio::test]
async fn times_out_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path(), RunStatus::Running);
    let addr = spawn(tmp.path()).await;
    let t = Instant::now();
    let r = wait(
        addr,
        json!({"watch": [{"kind": "run_settled", "run": RUN},
                         {"kind": "run_events", "run": RUN}],
               "timeout_ms": 300}),
    )
    .await;
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    assert_ne!(r["isError"], json!(true), "{r}");
    let s = &r["structuredContent"];
    assert_eq!(s["timed_out"], true, "{r}");
    assert_eq!(s["fired"], json!([]), "{r}");
    assert_eq!(s["events"], json!([]), "{r}");
}

#[tokio::test]
async fn an_unknown_run_is_an_error_not_a_hang() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn(tmp.path()).await;
    for watch in [
        json!({"kind": "run_settled", "run": "run_nope"}),
        json!({"kind": "run_events", "run": "run_nope"}),
        json!({"kind": "messages", "root": "run_nope"}),
    ] {
        let t = Instant::now();
        let r = wait(addr, json!({"watch": [watch], "timeout_ms": 5000})).await;
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
        assert_eq!(r["isError"], json!(true), "{r}");
        assert!(text_of(&r).contains("run_nope"), "{r}");
    }
}

#[tokio::test]
async fn a_project_local_run_is_found() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    rupu_workspace::upsert(&ws_store, project.path()).unwrap();
    RunStore::new(project.path().join(".rupu").join("runs"))
        .create(
            super::sse::seed_run(RUN, RunStatus::Completed),
            "name: test\nsteps: []\n",
        )
        .unwrap();
    let addr = spawn(global.path()).await;
    let r = wait(
        addr,
        json!({"watch": [{"kind": "run_settled", "run": RUN}], "timeout_ms": 5000}),
    )
    .await;
    assert_ne!(r["isError"], json!(true), "{r}");
    let s = &r["structuredContent"];
    assert_eq!(s["fired"][0]["kind"], "run_settled", "{r}");
    assert_eq!(s["timed_out"], false, "{r}");
}

#[tokio::test]
async fn a_string_cursor_on_a_local_source_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path(), RunStatus::Running);
    let addr = spawn(tmp.path()).await;
    let key = format!("run:{RUN}");
    let r = wait(
        addr,
        json!({"watch": [{"kind": "run_events", "run": RUN}],
               "cursor": {key.clone(): "msg_01"}, "timeout_ms": 0}),
    )
    .await;
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(text_of(&r).contains(&format!("cursor.{key}")), "{r}");
}

#[tokio::test]
async fn bad_arguments_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path(), RunStatus::Running);
    let addr = spawn(tmp.path()).await;
    let cases = [
        (
            json!({"watch": [{"kind": "run_settled", "run": RUN, "host": "mini"}]}),
            "watch[0] (`run_settled`): wait on a remote host is not supported yet",
        ),
        (
            json!({"watch": [{"kind": "run_settled", "run": RUN}], "follow": true}),
            "unknown argument `follow`",
        ),
        (
            json!({"watch": [{"kind": "run_settled", "run": RUN, "types": ["x"]}]}),
            "`types`",
        ),
        (json!({"watch": []}), "`watch`"),
        (
            json!({"watch": [{"kind": "run_events", "run": RUN, "types": ["step_started", "step_warped"]}]}),
            "unknown event type `step_warped`",
        ),
        (
            json!({"watch": [{"kind": "messages", "root": RUN, "kinds": ["shout"]}]}),
            "shout",
        ),
        (
            json!({"watch": [{"kind": "run_settled", "run": "../config"}]}),
            "../config",
        ),
    ];
    for (args, want) in cases {
        let r = wait(addr, args.clone()).await;
        assert_eq!(r["isError"], json!(true), "{args} -> {r}");
        assert!(text_of(&r).contains(want), "{args} -> {r}");
    }
    // `host: local` is this host.
    let r = wait(
        addr,
        json!({"watch": [{"kind": "run_settled", "run": RUN, "host": "local"}],
               "timeout_ms": 0}),
    )
    .await;
    assert_ne!(r["isError"], json!(true), "{r}");
}

#[test]
fn timeout_is_capped_at_55s() {
    use rupu_cp::mcp::wait::clamp_timeout;
    assert_eq!(clamp_timeout(None), Duration::from_millis(30_000));
    assert_eq!(clamp_timeout(Some(1_000)), Duration::from_millis(1_000));
    assert_eq!(clamp_timeout(Some(55_000)), Duration::from_millis(55_000));
    assert_eq!(clamp_timeout(Some(600_000)), Duration::from_millis(55_000));
}

/// Cancelling an in-flight `wait` stops it: no reply (202), long before its
/// timeout. The cancel is re-sent until the call ends, since it can arrive
/// before the call is registered.
#[tokio::test]
async fn cancelling_a_wait_stops_it() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path(), RunStatus::Running);
    let addr = spawn(tmp.path()).await;
    let c = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");
    let call = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {"name": "wait", "arguments": {
            "watch": [{"kind": "run_settled", "run": RUN}], "timeout_ms": 20_000}}});
    let t = Instant::now();
    let pending = {
        let (c, url) = (c.clone(), url.clone());
        tokio::spawn(async move {
            c.post(&url)
                .header("Mcp-Session-Id", "mcp_W")
                .json(&call)
                .send()
                .await
                .unwrap()
        })
    };
    let cancel = json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": {"requestId": 7}});
    while !pending.is_finished() {
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "the wait was never cancelled"
        );
        let r = c
            .post(&url)
            .header("Mcp-Session-Id", "mcp_W")
            .json(&cancel)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 202);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let resp = pending.await.unwrap();
    assert_eq!(resp.status(), 202);
    assert!(resp.text().await.unwrap().is_empty());
    assert!(t.elapsed() < Duration::from_secs(10));
}

// ---- agentiflow, agent_run_end, session_turn ------------------------------

const FLOW: &str = "af_01J9ZQ3K4M5N6P7Q8R9S0T1WAT";

fn transcript(global: &Path, run: &str, answer: &str, status: &str) -> std::path::PathBuf {
    let dir = global.join("transcripts");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{run}.jsonl"));
    let a = json!({"type": "assistant_message", "data": {"content": answer}});
    let c = json!({"type": "run_complete", "data": {"run_id": run, "status": status,
        "total_tokens": 12, "duration_ms": 5}});
    std::fs::write(&path, format!("{a}\n{c}\n")).unwrap();
    path
}

fn session(global: &Path, id: &str, status: &str, active: Option<&str>, runs: Value) {
    let dir = global.join("sessions").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let v = json!({"session_id": id, "agent_name": "triage", "status": status,
        "active_run_id": active, "runs": runs});
    std::fs::write(dir.join("session.json"), serde_json::to_vec(&v).unwrap()).unwrap();
}

#[tokio::test]
async fn agentiflow_lines_are_returned_and_filtered_by_kind() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = rupu_agentiflow::agentiflow_dir(tmp.path()).join(FLOW);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("events.jsonl"),
        "{\"kind\":\"round\",\"n\":1}\n{\"kind\":\"run_stopped\",\"why\":\"done\"}\n",
    )
    .unwrap();
    let addr = spawn(tmp.path()).await;
    let r = wait(
        addr,
        json!({"watch": [{"kind": "agentiflow", "id": FLOW, "types": ["run_stopped"]}]}),
    )
    .await;
    assert_ne!(r["isError"], json!(true), "{r}");
    let s = &r["structuredContent"];
    assert_eq!(s["events"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(s["events"][0]["event"]["why"], "done", "{r}");
    assert_eq!(s["fired"][0]["detail"]["count"], 1, "{r}");
    assert_eq!(
        s["cursor"],
        json!({ format!("agentiflow:{FLOW}"): 2 }),
        "{r}"
    );
}

#[tokio::test]
async fn an_ended_agent_run_fires_with_its_final_answer() {
    let tmp = tempfile::tempdir().unwrap();
    transcript(tmp.path(), "run_a", "done.", "ok");
    let addr = spawn(tmp.path()).await;
    let r = wait(
        addr,
        json!({"watch": [{"kind": "agent_run_end", "run": "run_a"}]}),
    )
    .await;
    assert_ne!(r["isError"], json!(true), "{r}");
    let d = &r["structuredContent"]["fired"][0]["detail"];
    assert_eq!(d["final_answer"], "done.", "{r}");
    assert_eq!(d["total_tokens"], 12, "{r}");
    assert_eq!(d["status"], "ok", "{r}");
}

#[tokio::test]
async fn an_agent_run_that_appears_late_still_fires() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn(tmp.path()).await;
    let g = tmp.path().to_path_buf();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        transcript(&g, "run_late", "late.", "ok");
    });
    let r = wait(
        addr,
        json!({"watch": [{"kind": "agent_run_end", "run": "run_late"}], "timeout_ms": 20000}),
    )
    .await;
    assert_ne!(r["isError"], json!(true), "{r}");
    assert_eq!(
        r["structuredContent"]["fired"][0]["detail"]["final_answer"], "late.",
        "{r}"
    );
}

#[tokio::test]
async fn an_agent_run_that_never_appears_is_an_error_after_the_grace() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn(tmp.path()).await;
    let t = Instant::now();
    let r = wait(
        addr,
        json!({"watch": [{"kind": "agent_run_end", "run": "run_none"}], "timeout_ms": 30000}),
    )
    .await;
    assert!(t.elapsed() < Duration::from_secs(12), "{:?}", t.elapsed());
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(text_of(&r).contains("run_none"), "{r}");
}

#[tokio::test]
async fn a_session_turn_fires_with_the_turns_answer() {
    let tmp = tempfile::tempdir().unwrap();
    let path = transcript(tmp.path(), "run_t", "the answer.", "ok");
    session(
        tmp.path(),
        "ses_A",
        "running",
        Some("run_t"),
        json!([{"run_id": "run_t", "transcript_path": path.to_str().unwrap()}]),
    );
    let addr = spawn(tmp.path()).await;
    let r = wait(
        addr,
        json!({"watch": [{"kind": "session_turn", "session": "ses_A"}]}),
    )
    .await;
    assert_ne!(r["isError"], json!(true), "{r}");
    let d = &r["structuredContent"]["fired"][0]["detail"];
    assert_eq!(d["run_id"], "run_t", "{r}");
    assert_eq!(d["final_answer"], "the answer.", "{r}");
}

#[tokio::test]
async fn a_session_that_stopped_fires_without_a_turn() {
    let tmp = tempfile::tempdir().unwrap();
    session(tmp.path(), "ses_B", "stopped", None, json!([]));
    let addr = spawn(tmp.path()).await;
    let r = wait(
        addr,
        json!({"watch": [{"kind": "session_turn", "session": "ses_B"}]}),
    )
    .await;
    assert_ne!(r["isError"], json!(true), "{r}");
    let d = &r["structuredContent"]["fired"][0]["detail"];
    assert_eq!(d["run_id"], Value::Null, "{r}");
    assert_eq!(d["final_answer"], Value::Null, "{r}");
    assert_eq!(d["status"], "stopped", "{r}");
}

#[tokio::test]
async fn a_host_on_these_conditions_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn(tmp.path()).await;
    for c in [
        json!({"kind": "agentiflow", "id": FLOW, "host": "local"}),
        json!({"kind": "agent_run_end", "run": "run_a", "host": "local"}),
        json!({"kind": "session_turn", "session": "ses_A", "host": "other"}),
    ] {
        let r = wait(addr, json!({ "watch": [c] })).await;
        assert_eq!(r["isError"], json!(true), "{r}");
        assert!(text_of(&r).contains("host"), "{r}");
    }
}
