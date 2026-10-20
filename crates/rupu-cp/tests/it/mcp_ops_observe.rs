//! The observe-area operator tools: netflow, usage and the dashboard.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use super::mcp::{
    assert_required_args, call_tool, json_of, rpc, spawn_operator_with, text_of, OP_RUN,
};
use serde_json::{json, Value};
use std::net::SocketAddr;

async fn http_get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let r = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

fn is_error(r: &Value) -> bool {
    r["isError"] == json!(true)
}

/// A mirrored worker run that recorded no customer: its customer can't be
/// known here, so a customer filter leaves its worker host out.
fn seed_mirrored_run(global: &std::path::Path) {
    let store = rupu_orchestrator::runs::RunStore::new(global.join("runs"));
    let mut rec = super::sse::seed_run(
        "run_01J9ZQ3K4M5N6P7Q8R9S0T1MIR",
        rupu_orchestrator::runs::RunStatus::Completed,
    );
    rec.worker_id = Some("node_1".into());
    store.create(rec, "name: test\nsteps: []\n").unwrap();
}

#[tokio::test]
async fn observe_tools_are_listed_with_their_required_arguments() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let table: &[(&str, &[&str])] = &[
        ("netflow.run", &["id"]),
        ("netflow.agentiflow", &["id"]),
        ("netflow.project", &["id"]),
        ("netflow.global", &[]),
        ("netflow.graph", &[]),
        ("netflow.explorer", &[]),
        ("netflow.index", &[]),
        ("netflow.latest", &[]),
        ("usage.get", &[]),
        ("usage.timeline", &[]),
        ("usage.runs", &[]),
        ("usage.outliers", &[]),
        ("dashboard.get", &[]),
    ];
    assert_required_args(addr, table).await;
    // The netflow `host` is a flow filter, and the tools say so.
    let list = rpc(addr, "tools/list", json!({})).await["result"]["tools"].clone();
    let tool = |name: &str| -> Value {
        list.as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap()
            .clone()
    };
    for name in ["netflow.run", "netflow.global", "netflow.explorer"] {
        let t = tool(name);
        assert!(t["description"].as_str().unwrap().contains("host:port"));
        assert!(t["inputSchema"]["properties"]["not_host"].is_object());
        assert_eq!(t["annotations"]["readOnlyHint"], json!(true));
    }
}

#[tokio::test]
async fn observe_reads_answer_what_the_api_answers() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let run_path = format!("/api/runs/{OP_RUN}/netflow");
    let cases: &[(&str, Value, &str)] = &[
        ("netflow.run", json!({"id": OP_RUN}), &run_path),
        ("netflow.global", json!({}), "/api/netflow"),
        (
            "netflow.global",
            json!({"from": "2026-01-01T00:00:00Z", "not_host": "a.example:443,b.example:80"}),
            "/api/netflow?from=2026-01-01T00%3A00%3A00Z&not_host=a.example%3A443%2Cb.example%3A80",
        ),
        ("netflow.graph", json!({"from": "2026-01-01T00:00:00Z"}), "/api/netflow/graph?from=2026-01-01T00%3A00%3A00Z"),
        ("netflow.explorer", json!({}), "/api/netflow/explorer"),
        ("netflow.index", json!({}), "/api/netflow/index"),
        ("netflow.latest", json!({}), "/api/netflow/latest"),
        (
            "usage.get",
            json!({"since": "2026-01-01T00:00:00Z", "until": "2026-01-02T00:00:00Z", "group_by": "model"}),
            "/api/usage?since=2026-01-01T00%3A00%3A00Z&until=2026-01-02T00%3A00%3A00Z&group_by=model",
        ),
        (
            "usage.timeline",
            json!({"since": "2026-01-01T00:00:00Z", "until": "2026-01-03T00:00:00Z", "bucket": "week"}),
            "/api/usage/timeline?since=2026-01-01T00%3A00%3A00Z&until=2026-01-03T00%3A00%3A00Z&bucket=week",
        ),
        (
            "usage.runs",
            json!({"since": "2026-01-01T00:00:00Z", "until": "2026-01-03T00:00:00Z"}),
            "/api/usage/runs?since=2026-01-01T00%3A00%3A00Z&until=2026-01-03T00%3A00%3A00Z",
        ),
        (
            "usage.outliers",
            json!({"since": "2026-01-01T00:00:00Z", "until": "2026-01-03T00:00:00Z"}),
            "/api/usage/outliers?since=2026-01-01T00%3A00%3A00Z&until=2026-01-03T00%3A00%3A00Z",
        ),
        ("dashboard.get", json!({"range": "7d", "host": "local"}), "/api/dashboard?range=7d&host=local"),
    ];
    for (tool, args, path) in cases {
        let r = call_tool(addr, tool, args.clone()).await;
        let (status, want) = http_get(addr, path).await;
        assert_eq!(status, 200, "{tool}: {path}");
        assert!(!is_error(&r), "{tool}: {r}");
        let mut got = json_of(&r);
        let mut want = want;
        // The aggregates stamp the time they read each host.
        if matches!(*tool, "dashboard.get" | "usage.get") {
            for v in [&mut got, &mut want] {
                v.as_object_mut().unwrap().remove("captured_at");
                for h in v["hosts"].as_array_mut().unwrap() {
                    h.as_object_mut().unwrap().remove("captured_at");
                }
            }
        }
        assert_eq!(got, want, "{tool}");
    }
}

#[tokio::test]
async fn netflow_for_an_unknown_run_is_a_404() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let r = call_tool(addr, "netflow.run", json!({"id": "run_nope"})).await;
    assert!(is_error(&r), "{r}");
    assert!(text_of(&r).starts_with("HTTP 404"), "{r}");
}

#[tokio::test]
async fn usage_runs_carries_the_without_customer_note() {
    let (addr, _tmp) = spawn_operator_with(seed_mirrored_run, None).await;
    let r = call_tool(addr, "usage.runs", json!({"customer": "acme"})).await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(
        r["content"][1]["text"],
        "x-rupu-hosts-without-customer: node_1 — these hosts were left out: they can't report a customer"
    );
    assert_eq!(
        r["_meta"]["rupu/headers"]["x-rupu-hosts-without-customer"],
        "node_1"
    );
    // Without the filter there is no note.
    let r = call_tool(addr, "usage.runs", json!({})).await;
    assert!(r["_meta"]["rupu/headers"]
        .get("x-rupu-hosts-without-customer")
        .is_none());
}
