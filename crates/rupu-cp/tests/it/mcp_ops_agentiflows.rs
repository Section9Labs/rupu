//! The agentiflows-area operator tools: parity with plain HTTP and the
//! steer attribution.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use super::mcp::{call_tool, rpc, spawn_operator_with, text_of};
use rupu_agentiflow::{agentiflow_dir, AgentiflowRecord};
use rupu_fleet::{Bus, LogLine, Member};
use serde_json::{json, Value};
use std::net::SocketAddr;

const FLOW: &str = "af_01J9ZQ3K4M5N6P7Q8R9S0T1MCP";

/// A running agentiflow whose coordinator is this process, its lead joined.
fn seed_live_flow(global: &std::path::Path) {
    let dir = agentiflow_dir(global).join(FLOW);
    AgentiflowRecord {
        id: FLOW.into(),
        name: "itest".into(),
        engagement_profiles: vec![],
        trigger: rupu_runtime::RunTriggerSource::Agentiflow,
        status: "running".into(),
        stop_reason: None,
        rounds: 0,
        goals: vec![],
        started_at: "2026-10-07T00:00:00Z".parse().unwrap(),
        ended_at: None,
        codename: None,
        spent_usd: None,
        spent_tokens: 0,
        runner_pid: Some(std::process::id()),
    }
    .write(&dir)
    .unwrap();
    Bus::open(&dir, "lead")
        .join(Member {
            agent: "lead".into(),
            run_id: format!("{FLOW}_lead"),
            kind: "flow_lead".into(),
            ..Default::default()
        })
        .unwrap();
}

async fn http_get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let r = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

fn json_of(result: &Value) -> Value {
    result
        .get("structuredContent")
        .cloned()
        .unwrap_or_else(|| serde_json::from_str(&text_of(result)).unwrap_or(Value::Null))
}

#[tokio::test]
async fn agentiflows_tools_are_listed_with_their_required_arguments() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let list = rpc(addr, "tools/list", json!({})).await["result"]["tools"].clone();
    let table: &[(&str, &[&str])] = &[
        ("agentiflows.list", &[]),
        ("agentiflows.get", &["id"]),
        ("agentiflows.events", &["id"]),
        ("agentiflows.children", &["id"]),
        ("agentiflows.unit_output", &["id", "unit_id"]),
        ("agentiflows.steer", &["id", "message"]),
    ];
    for (name, required) in table {
        let tool = list
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == *name)
            .unwrap_or_else(|| panic!("{name} is not listed"));
        let mut got: Vec<String> = tool["inputSchema"]["required"]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
            .unwrap_or_default();
        got.sort();
        let mut want: Vec<String> = required.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(got, want, "{name}");
    }
}

#[tokio::test]
async fn agentiflows_reads_answer_what_the_api_answers() {
    let (addr, _tmp) = spawn_operator_with(seed_live_flow, None).await;
    let cases: &[(&str, Value, String)] = &[
        ("agentiflows.list", json!({}), "/api/agentiflows".into()),
        (
            "agentiflows.get",
            json!({"id": FLOW}),
            format!("/api/agentiflows/{FLOW}"),
        ),
        (
            "agentiflows.events",
            json!({"id": FLOW}),
            format!("/api/agentiflows/{FLOW}/events"),
        ),
        (
            "agentiflows.children",
            json!({"id": FLOW}),
            format!("/api/agentiflows/{FLOW}/children"),
        ),
    ];
    for (tool, args, path) in cases {
        let r = call_tool(addr, tool, args.clone()).await;
        let (status, want) = http_get(addr, path).await;
        assert_eq!(status, 200, "{tool}: {path}");
        assert_ne!(r["isError"], json!(true), "{tool}: {r}");
        assert_eq!(json_of(&r), want, "{tool}");
    }
    let list = json_of(&call_tool(addr, "agentiflows.list", json!({})).await);
    assert_eq!(list["rows"].as_array().map(|a| a.len()), Some(1), "{list}");
}

#[tokio::test]
async fn steer_through_mcp_records_client_mcp() {
    let (addr, tmp) = spawn_operator_with(seed_live_flow, None).await;
    let r = call_tool(
        addr,
        "agentiflows.steer",
        json!({"id": FLOW, "message": "go"}),
    )
    .await;
    assert_ne!(r["isError"], json!(true), "{r}");
    let space = agentiflow_dir(tmp.path()).join(FLOW);
    let lines = Bus::open(&space, rupu_fleet::address::OPERATOR)
        .read_log(None, usize::MAX)
        .unwrap();
    let msgs: Vec<_> = lines
        .lines
        .iter()
        .filter_map(|l| match l {
            LogLine::Msg(m) => Some(serde_json::to_value(m).unwrap()),
            _ => None,
        })
        .collect();
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert_eq!(msgs[0]["via"], json!("cp"), "{}", msgs[0]);
    assert_eq!(msgs[0]["client"], json!("mcp"), "{}", msgs[0]);
}
