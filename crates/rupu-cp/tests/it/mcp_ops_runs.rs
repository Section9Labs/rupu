//! The runs-area operator tools: listing, parity with plain HTTP, run
//! control attribution and the `GET /api/events` query keys.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use super::mcp::{
    assert_required_args, call_tool, json_of, rpc, spawn_operator_with, text_of, OP_RUN,
};
use rupu_orchestrator::runs::{AttemptRecord, RunStatus, RunStore};
use serde_json::{json, Value};
use std::net::SocketAddr;

const PAUSED: &str = "run_paused_for_mcp";

fn seed_paused(dir: &std::path::Path) {
    let rec = super::sse::seed_run(PAUSED, RunStatus::Paused);
    RunStore::new(dir.join("runs"))
        .create(rec, "name: test\nsteps: []\n")
        .unwrap();
}

/// Three failed-step events in `OP_RUN`'s `events.jsonl`.
fn seed_events(dir: &std::path::Path) {
    let lines: Vec<String> = (0..3)
        .map(|i| {
            serde_json::to_string(&rupu_orchestrator::executor::Event::StepFailed {
                run_id: OP_RUN.into(),
                step_id: format!("s{i}"),
                error: "boom".into(),
                cause: None,
            })
            .unwrap()
        })
        .collect();
    std::fs::write(
        dir.join("runs").join(OP_RUN).join("events.jsonl"),
        lines.join("\n") + "\n",
    )
    .unwrap();
}

async fn http_get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let r = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn runs_tools_are_listed_with_their_required_arguments() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let table: &[(&str, &[&str])] = &[
        ("runs.list_all", &[]),
        ("runs.archived", &[]),
        ("runs.attempts", &["id"]),
        ("runs.delete", &["id"]),
        ("runs.archive", &["id"]),
        ("runs.restore", &["id"]),
        ("runs.pause", &["id"]),
        ("runs.resume", &["id"]),
        ("runs.graph", &["id"]),
        ("runs.children", &["id"]),
        ("runs.usage", &["id"]),
        ("runs.usage_timeline", &["id"]),
        ("runs.source", &["id", "path"]),
        ("runs.ast", &["id", "path"]),
        ("runs.autoflow", &["id"]),
        ("agent_runs.list", &[]),
        ("transcripts.get", &["path"]),
        ("transcripts.page", &["path"]),
        ("transcripts.archive", &["id"]),
        ("transcripts.delete", &["id"]),
        ("events.recent", &[]),
        ("autoflow_runs.list", &[]),
        ("autoflow_runs.problems", &[]),
        ("autoflow_runs.cycles", &[]),
        ("autoflow_runs.events", &[]),
    ];
    assert_required_args(addr, table).await;
    let list = rpc(addr, "tools/list", json!({})).await["result"]["tools"].clone();
    let attempts = list
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "runs.attempts")
        .unwrap();
    let properties = attempts["inputSchema"]["properties"].as_object().unwrap();
    let keys: std::collections::BTreeSet<_> = properties.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "id",
            "host",
            "step_id",
            "unit_index",
            "sub_id",
            "limit",
            "cursor"
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(attempts["annotations"]["readOnlyHint"], true);
    assert_eq!(attempts["annotations"]["idempotentHint"], true);
    assert_eq!(attempts["annotations"]["destructiveHint"], false);
    assert_eq!(attempts["annotations"]["openWorldHint"], false);
}

#[tokio::test]
async fn attempts_mcp_preserves_filters_and_immutable_cursor_pages() {
    let (addr, tmp) = spawn_operator_with(|_| {}, None).await;
    let store = RunStore::new(tmp.path().join("runs"));
    let row = |id: &str, unit: usize, sub: &str| AttemptRecord {
        v: 1,
        step_id: "review + evidence".into(),
        unit_index: Some(unit),
        sub_id: Some(sub.into()),
        agent_run_id: id.into(),
        transcript_path: format!("/nonexistent/{id}.jsonl").into(),
        host: Some("worker-a".into()),
        continued_from: None,
        started_at: "2026-11-06T10:00:01Z".parse().unwrap(),
    };
    for record in [
        row("run_first", 2, "check / headers"),
        row("run_wrong_unit", 3, "check / headers"),
        row("run_wrong_sub", 2, "other"),
        row("run_second", 2, "check / headers"),
    ] {
        store.append_attempt(OP_RUN, &record).unwrap();
    }
    let mut args = json!({
        "id": OP_RUN, "host":"local", "step_id":"review + evidence",
        "unit_index":2, "sub_id":"check / headers", "limit":1
    });
    let first_result = call_tool(addr, "runs.attempts", args.clone()).await;
    assert_ne!(first_result["isError"], true, "{first_result}");
    let first = json_of(&first_result);
    assert_eq!(first["rows"].as_array().unwrap().len(), 1);
    assert_eq!(first["rows"][0]["agent_run_id"], "run_first");
    assert_eq!(first["rows"][0]["host"], "worker-a");
    assert_eq!(first["recorded_matching_total"], 2);
    let cursor = first["next_cursor"].as_str().unwrap();
    args["cursor"] = cursor.into();
    // A later append cannot change an existing snapshot's pages.
    store
        .append_attempt(OP_RUN, &row("run_later", 2, "check / headers"))
        .unwrap();
    let next_result = call_tool(addr, "runs.attempts", args.clone()).await;
    assert_ne!(next_result["isError"], true, "{next_result}");
    let next = json_of(&next_result);
    assert_eq!(next["snapshot"], first["snapshot"]);
    assert_eq!(next["rows"].as_array().unwrap().len(), 1);
    assert_eq!(next["rows"][0]["agent_run_id"], "run_second");
    assert!(next["next_cursor"].is_null());
    let path = format!(
        "/api/runs/{OP_RUN}/attempts?host=local&step_id=review%20%2B%20evidence&unit_index=2&sub_id=check%20%2F%20headers&limit=1&cursor={cursor}"
    );
    let (status, same_page) = http_get(addr, &path).await;
    assert_eq!(status, 200);
    assert_eq!(next, same_page);
    args["unit_index"] = 3.into();
    let mismatched = call_tool(addr, "runs.attempts", args).await;
    assert_eq!(mismatched["isError"], true, "{mismatched}");
    assert!(text_of(&mismatched).starts_with("HTTP 400"));
}

#[tokio::test]
async fn attempts_mcp_preserves_missing_evidence_and_refuses_remote_owners() {
    let (addr, tmp) = spawn_operator_with(|_| {}, None).await;
    std::fs::remove_file(tmp.path().join("runs").join(OP_RUN).join("attempts.jsonl")).unwrap();
    let result = call_tool(addr, "runs.attempts", json!({"id":OP_RUN})).await;
    assert_ne!(result["isError"], true, "{result}");
    let page = json_of(&result);
    assert_eq!(page["evidence"]["ledger"], "missing");
    assert!(page["recorded_matching_total"].is_null());
    assert!(page["rows"].as_array().unwrap().is_empty());
    let remote = call_tool(addr, "runs.attempts", json!({"id":OP_RUN,"host":"remote"})).await;
    assert_eq!(remote["isError"], true, "{remote}");
    assert!(text_of(&remote).starts_with("HTTP 501"));
}

#[tokio::test]
async fn transcript_pages_mcp_matches_http_and_preserves_stale_snapshot_errors() {
    let (addr, tmp) = spawn_operator_with(|_| {}, None).await;
    let path = tmp.path().join("paged transcript + scope.jsonl");
    std::fs::write(&path, "{\"type\":\"assistant_message\",\"data\":{\"content\":\"one\"}}\ninvalid\n{\"type\":\"assistant_message\",\"data\":{\"content\":\"three\"}}\n").unwrap();
    let mut args = json!({"path":path,"host":"local","run":"run_page_owner","limit":1});
    let first_result = call_tool(addr, "transcripts.page", args.clone()).await;
    assert_ne!(first_result["isError"], true, "{first_result}");
    let first = json_of(&first_result);
    assert_eq!(first["version"], 1);
    assert_eq!(first["records"][0]["position"], 1);
    args["cursor"] = first["next_cursor"].clone();
    let next_result = call_tool(addr, "transcripts.page", args.clone()).await;
    assert_ne!(next_result["isError"], true, "{next_result}");
    let next = json_of(&next_result);
    assert_eq!(next["records"][0]["position"], 2);
    assert_eq!(next["records"][0]["status"], "malformed");
    let http = reqwest::Client::new()
        .get(format!("http://{addr}/api/transcript/page"))
        .query(&[
            ("path", path.to_str().unwrap()),
            ("host", "local"),
            ("run", "run_page_owner"),
            ("limit", "1"),
            ("cursor", args["cursor"].as_str().unwrap()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(http.status().as_u16(), 200);
    assert_eq!(http.json::<Value>().await.unwrap(), next);
    std::fs::write(&path, "changed\n").unwrap();
    let stale = call_tool(addr, "transcripts.page", args.clone()).await;
    assert_eq!(stale["isError"], true);
    assert!(text_of(&stale).starts_with("HTTP 409"));
    args["host"] = "remote".into();
    args.as_object_mut().unwrap().remove("cursor");
    let remote = call_tool(addr, "transcripts.page", args).await;
    assert!(text_of(&remote).starts_with("HTTP 501"));
}

#[tokio::test]
async fn runs_reads_answer_what_the_api_answers() {
    let (addr, _tmp) = spawn_operator_with(seed_paused, None).await;
    let cases: &[(&str, Value, String)] = &[
        ("runs.list_all", json!({}), "/api/runs".into()),
        ("runs.archived", json!({}), "/api/runs/archived".into()),
        (
            "runs.usage",
            json!({"id": OP_RUN}),
            format!("/api/runs/{OP_RUN}/usage"),
        ),
        (
            "runs.usage_timeline",
            json!({"id": OP_RUN}),
            format!("/api/runs/{OP_RUN}/usage-timeline"),
        ),
        (
            "runs.children",
            json!({"id": OP_RUN}),
            format!("/api/runs/{OP_RUN}/children"),
        ),
        ("agent_runs.list", json!({}), "/api/runs/agents".into()),
        (
            "autoflow_runs.list",
            json!({}),
            "/api/runs/autoflows/runs".into(),
        ),
        (
            "autoflow_runs.cycles",
            json!({}),
            "/api/runs/autoflows".into(),
        ),
        (
            "autoflow_runs.problems",
            json!({}),
            "/api/runs/autoflows/problems".into(),
        ),
    ];
    for (tool, args, path) in cases {
        let r = call_tool(addr, tool, args.clone()).await;
        let (status, want) = http_get(addr, path).await;
        assert_eq!(status, 200, "{tool}: {path}");
        assert_ne!(r["isError"], json!(true), "{tool}: {r}");
        assert_eq!(json_of(&r), want, "{tool}");
    }
    // The seeded runs show up: the list is not trivially equal-and-empty.
    let all = json_of(&call_tool(addr, "runs.list_all", json!({})).await);
    assert!(all.as_array().unwrap().len() >= 2, "{all}");
}

#[tokio::test]
async fn events_recent_reads_the_same_keys_as_before() {
    let (addr, tmp) = spawn_operator_with(|_| {}, None).await;
    seed_events(tmp.path());
    let ids = |v: &Value| -> Vec<(String, u64)> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["step_id"].as_str().unwrap().to_string(),
                    r["pos"].as_u64().unwrap(),
                )
            })
            .collect()
    };
    let (s, all) = http_get(addr, "/api/events").await;
    assert_eq!(s, 200);
    assert_eq!(ids(&all).len(), 3, "{all}");

    // `limit` and `q`.
    let (s, two) = http_get(addr, "/api/events?limit=2&q=type:step_failed").await;
    assert_eq!(s, 200);
    assert_eq!(ids(&two).len(), 2, "{two}");
    let (_, none) = http_get(addr, "/api/events?q=type:run_completed").await;
    assert!(none.as_array().unwrap().is_empty(), "{none}");

    // A number that doesn't parse, or a zero limit, is ignored, not a 400.
    for junk in [
        "limit=abc",
        "limit=0",
        "limit=-3",
        "before_ts=soon",
        "before_pos=x&before_run=r",
    ] {
        let (s, rows) = http_get(addr, &format!("/api/events?{junk}")).await;
        assert_eq!(s, 200, "{junk}");
        assert_eq!(ids(&rows), ids(&all), "{junk}");
    }

    // The full cursor resumes exactly after the row it names; unknown keys
    // are ignored; a bad query line is the 400 it always was.
    let first = &all.as_array().unwrap()[0];
    let cursor = format!(
        "/api/events?before_ts={}&before_run={}&before_pos={}&extra=1",
        first["ts"],
        first["run_id"].as_str().unwrap(),
        first["pos"]
    );
    let (s, rest) = http_get(addr, &cursor).await;
    assert_eq!(s, 200);
    assert_eq!(ids(&rest), ids(&all)[1..].to_vec(), "{rest}");
    let (s, err) = http_get(addr, "/api/events?q=nosuchfield:1").await;
    assert_eq!(s, 400, "{err}");

    // The tool answers what the route answers for the same keys.
    let r = call_tool(
        addr,
        "events.recent",
        json!({"limit": 2, "q": "type:step_failed"}),
    )
    .await;
    assert_ne!(r["isError"], json!(true), "{r}");
    assert_eq!(json_of(&r), two);
}

#[tokio::test]
async fn runs_pause_resume_record_through_mcp() {
    let (addr, tmp) = spawn_operator_with(seed_paused, None).await;
    let r = call_tool(addr, "runs.resume", json!({"id": PAUSED})).await;
    assert_ne!(r["isError"], json!(true), "{r}");
    // The route answers the run's detail with the marker already on it.
    assert_eq!(r["structuredContent"]["run"]["id"], json!(PAUSED), "{r}");
    let rec = RunStore::new(tmp.path().join("runs")).load(PAUSED).unwrap();
    assert_eq!(rec.resume_approver.as_deref(), Some("mcp"));
    assert!(rec.resume_requested_at.is_some());
    // A run that is not paused is refused with its status, not swallowed.
    let r = call_tool(addr, "runs.resume", json!({"id": OP_RUN})).await;
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(text_of(&r).contains("409"), "{r}");
    // Pausing a run that is not running is refused, too.
    let r = call_tool(addr, "runs.pause", json!({"id": OP_RUN})).await;
    assert_eq!(r["isError"], json!(true), "{r}");
}

#[tokio::test]
async fn runs_delete_is_marked_destructive() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let list = rpc(addr, "tools/list", json!({})).await["result"]["tools"].clone();
    let hint = |name: &str, key: &str| -> Value {
        list.as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap()["annotations"][key]
            .clone()
    };
    for name in ["runs.delete", "transcripts.delete"] {
        assert_eq!(hint(name, "destructiveHint"), json!(true), "{name}");
    }
    assert_eq!(hint("runs.resume", "openWorldHint"), json!(true));
    assert_eq!(hint("runs.list_all", "readOnlyHint"), json!(true));
    assert_ne!(hint("runs.archive", "destructiveHint"), json!(true));
}
