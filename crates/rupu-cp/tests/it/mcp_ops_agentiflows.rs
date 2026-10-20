//! The agentiflows-area operator tools: parity with plain HTTP, the steer
//! attribution, and the launch, stop and definition tools.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use super::agentiflow_api::{self as api, FakeLauncher};
use super::mcp::{assert_required_args, call_tool, json_of, rpc, spawn_operator_with};
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

#[tokio::test]
async fn agentiflows_tools_are_listed_with_their_required_arguments() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let table: &[(&str, &[&str])] = &[
        ("agentiflows.list", &[]),
        ("agentiflows.get", &["id"]),
        ("agentiflows.events", &["id"]),
        ("agentiflows.children", &["id"]),
        ("agentiflows.unit_output", &["id", "unit_id"]),
        ("agentiflows.steer", &["id", "message"]),
        ("agentiflows.run", &["definition"]),
        ("agentiflows.stop", &["id"]),
        ("agentiflow_defs.list", &[]),
        ("agentiflow_defs.get", &["name"]),
        ("agentiflow_defs.create", &["raw"]),
        ("agentiflow_defs.save", &["name", "raw"]),
        ("agentiflow_defs.delete", &["name"]),
        ("agentiflow_defs.validate", &["raw"]),
    ];
    assert_required_args(addr, table).await;
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

#[tokio::test]
async fn steer_with_stop_queues_a_stop() {
    let (addr, _tmp) = spawn_operator_with(seed_live_flow, None).await;
    let r = call_tool(
        addr,
        "agentiflows.steer",
        json!({"id": FLOW, "message": "wind down", "stop": true}),
    )
    .await;
    assert_ne!(r["isError"], json!(true), "{r}");
    assert_eq!(json_of(&r), json!({"queued": true}), "{r}");
}

/// A CP over `state`, serving `/mcp`.
async fn spawn_state(state: rupu_cp::state::AppState) -> SocketAddr {
    let server = rupu_cp::mcp::build_server(
        &state,
        &rupu_cp::mcp::McpOpts {
            catalog: None,
            tools: vec![],
        },
    )
    .unwrap();
    let app = rupu_cp::server::router_with(
        state,
        None,
        rupu_cp::server::RouterOpts {
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

/// A temp dir holding `home/` (the global dir, with the `itest` definition)
/// and the CP over it, writable, with `launcher`.
async fn spawn_home(launcher: FakeLauncher) -> (SocketAddr, tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("home");
    let defs = agentiflow_dir(&global);
    std::fs::create_dir_all(&defs).unwrap();
    std::fs::write(defs.join("itest.yaml"), api::DEF).unwrap();
    let addr = spawn_state(api::state(&global, true, Some(launcher))).await;
    (addr, tmp, global)
}

fn is_error(r: &Value) -> bool {
    r["isError"] == json!(true)
}

#[tokio::test]
async fn agentiflow_defs_reads_answer_what_the_api_answers() {
    let (addr, _tmp, _global) = spawn_home(FakeLauncher::started()).await;
    let cases: &[(&str, Value, &str)] = &[
        ("agentiflow_defs.list", json!({}), "/api/agentiflow-defs"),
        (
            "agentiflow_defs.get",
            json!({"name": "itest"}),
            "/api/agentiflow-defs/itest",
        ),
        (
            "agentiflow_defs.get",
            json!({"name": "itest", "scope_kind": "global"}),
            "/api/agentiflow-defs/itest?scope_kind=global",
        ),
    ];
    for (tool, args, path) in cases {
        let r = call_tool(addr, tool, args.clone()).await;
        let (status, want) = http_get(addr, path).await;
        assert_eq!(status, 200, "{tool}: {path}");
        assert!(!is_error(&r), "{tool}: {r}");
        assert_eq!(json_of(&r), want, "{tool}");
    }
    let validated = call_tool(addr, "agentiflow_defs.validate", json!({"raw": api::DEF})).await;
    assert_eq!(json_of(&validated)["ok"], json!(true), "{validated}");
}

#[tokio::test]
async fn agentiflows_run_launches_through_the_port() {
    let launcher = FakeLauncher::started();
    let (addr, _tmp, global) = spawn_home(launcher.clone()).await;
    let r = call_tool(addr, "agentiflows.run", json!({"definition": "itest"})).await;
    assert!(!is_error(&r), "{r}");
    let body = json_of(&r);
    assert_eq!(body["status"], "started", "{body}");
    let seen = launcher.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].definition, "itest");
    assert_eq!(seen[0].run_id, body["id"].as_str().unwrap());
    assert_eq!(seen[0].working_dir, api::workspace_dir(&global));

    let r = call_tool(
        addr,
        "agentiflows.run",
        json!({"definition": "itest", "host": "peer"}),
    )
    .await;
    assert!(is_error(&r) && r.to_string().contains("501"), "{r}");
    assert_eq!(launcher.requests().len(), 1);
}

#[tokio::test]
async fn agentiflows_stop_queues_a_graceful_stop() {
    let (addr, _tmp, global) = spawn_home(FakeLauncher::started()).await;
    api::seed_flow(&global, FLOW, "running", Some(std::process::id()));
    let r = call_tool(addr, "agentiflows.stop", json!({"id": FLOW})).await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(json_of(&r), json!({"queued": true}));
    assert!(api::stop_queued(&global, FLOW));
}

#[tokio::test]
async fn agentiflow_defs_writes_record_through_mcp() {
    let (addr, _tmp, global) = spawn_home(FakeLauncher::started()).await;
    let raw = api::DEF.replace("name: itest", "name: fresh");
    let r = call_tool(addr, "agentiflow_defs.create", json!({"raw": raw})).await;
    assert!(!is_error(&r), "{r}");
    let file = agentiflow_dir(&global).join("fresh.yaml");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), raw);

    let edited = raw.replace("A fixture flow.", "Edited.");
    let r = call_tool(
        addr,
        "agentiflow_defs.save",
        json!({"name": "fresh", "raw": edited, "scope_kind": "global"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), edited);

    let r = call_tool(addr, "agentiflow_defs.delete", json!({"name": "fresh"})).await;
    assert!(!is_error(&r), "{r}");
    assert!(!file.exists());
}

/// Review Focus 1: `validate` is a literal sibling of `:name`; as a name it is
/// refused before any handler runs, and never reaches the validate route.
#[tokio::test]
async fn a_name_equal_to_the_validate_segment_is_refused() {
    let (addr, _tmp, global) = spawn_home(FakeLauncher::started()).await;
    let file = agentiflow_dir(&global).join("validate.yaml");
    std::fs::write(&file, api::DEF.replace("name: itest", "name: validate")).unwrap();
    for (tool, args) in [
        ("agentiflow_defs.get", json!({"name": "validate"})),
        ("agentiflow_defs.delete", json!({"name": "validate"})),
        (
            "agentiflow_defs.save",
            json!({"name": "validate", "raw": api::DEF}),
        ),
    ] {
        let r = call_tool(addr, tool, args).await;
        assert!(is_error(&r), "{tool}: {r}");
    }
    assert!(file.exists());
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        api::DEF.replace("name: itest", "name: validate")
    );
}

#[tokio::test]
async fn agentiflow_tool_hints() {
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
    assert_eq!(hint("agentiflows.run", "openWorldHint"), json!(true));
    assert_eq!(hint("agentiflows.stop", "destructiveHint"), json!(true));
    assert_eq!(
        hint("agentiflow_defs.delete", "destructiveHint"),
        json!(true)
    );
    assert_eq!(
        hint("agentiflow_defs.validate", "readOnlyHint"),
        json!(true)
    );
    assert_eq!(hint("agentiflow_defs.list", "readOnlyHint"), json!(true));
    assert_ne!(hint("agentiflow_defs.save", "destructiveHint"), json!(true));
}
