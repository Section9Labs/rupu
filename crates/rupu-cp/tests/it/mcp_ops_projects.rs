//! The projects-area operator tools.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use super::mcp::{assert_required_args, call_tool, json_of, spawn_operator_with};
use serde_json::{json, Value};
use std::net::SocketAddr;

const WS: &str = "ws_proj1";

/// A project root with a README and a source file, registered as `WS`.
fn seed_project(global: &std::path::Path) {
    let root = global.join("proj-root");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("README.md"), "# hi\nsecond line\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::create_dir_all(global.join("workspaces")).unwrap();
    std::fs::write(
        global.join("workspaces").join(format!("{WS}.toml")),
        format!(
            "id = \"{WS}\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
            root.display()
        ),
    )
    .unwrap();
}

async fn http_get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let r = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

fn is_error(r: &Value) -> bool {
    r["isError"] == json!(true)
}

#[tokio::test]
async fn projects_tools_are_listed_with_their_required_arguments() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let table: &[(&str, &[&str])] = &[
        ("projects.list", &[]),
        ("projects.get", &["ws_id"]),
        ("projects.runs", &["ws_id"]),
        ("projects.sessions", &["ws_id"]),
        ("projects.agents", &["ws_id"]),
        ("projects.workflows", &["ws_id"]),
        ("projects.autoflows", &["ws_id"]),
        ("projects.coverage", &["ws_id"]),
        ("projects.coverage_assessed", &["ws_id"]),
        ("projects.tree", &["ws_id"]),
        ("projects.source", &["ws_id"]),
        ("projects.files", &["ws_id"]),
    ];
    assert_required_args(addr, table).await;
}

#[tokio::test]
async fn projects_reads_answer_what_the_api_answers() {
    let (addr, _tmp) = spawn_operator_with(seed_project, None).await;
    let cases: &[(&str, Value, String)] = &[
        ("projects.list", json!({}), "/api/projects".into()),
        (
            "projects.get",
            json!({"ws_id": WS}),
            format!("/api/projects/{WS}"),
        ),
        (
            "projects.runs",
            json!({"ws_id": WS, "limit": 5}),
            format!("/api/projects/{WS}/runs?limit=5"),
        ),
        (
            "projects.sessions",
            json!({"ws_id": WS}),
            format!("/api/projects/{WS}/sessions"),
        ),
        (
            "projects.agents",
            json!({"ws_id": WS}),
            format!("/api/projects/{WS}/agents"),
        ),
        (
            "projects.coverage_assessed",
            json!({"ws_id": WS}),
            format!("/api/projects/{WS}/coverage/assessed"),
        ),
        (
            "projects.tree",
            json!({"ws_id": WS, "path": "src"}),
            format!("/api/projects/{WS}/tree?path=src"),
        ),
        (
            "projects.files",
            json!({"ws_id": WS}),
            format!("/api/projects/{WS}/files"),
        ),
    ];
    for (tool, args, path) in cases {
        let r = call_tool(addr, tool, args.clone()).await;
        let (status, want) = http_get(addr, path).await;
        assert_eq!(status, 200, "{tool}: {path}");
        assert!(!is_error(&r), "{tool}: {r}");
        assert_eq!(json_of(&r), want, "{tool}");
    }
    let r = call_tool(addr, "projects.list", json!({})).await;
    assert_eq!(json_of(&r)[0]["ws_id"], WS);
}

#[tokio::test]
async fn projects_source_returns_file_lines_as_json() {
    let (addr, _tmp) = spawn_operator_with(seed_project, None).await;
    let r = call_tool(
        addr,
        "projects.source",
        json!({"ws_id": WS, "path": "README.md"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let got = json_of(&r);
    assert_eq!(got["available"], true);
    assert_eq!(got["path"], "README.md");
    assert_eq!(got["totalLines"], 2);
    assert_eq!(got["lines"][0], json!({"n": 1, "text": "# hi"}));
    assert_eq!(got["lines"][1]["text"], "second line");
    let (_, want) = http_get(addr, &format!("/api/projects/{WS}/source?path=README.md")).await;
    assert_eq!(got, want);
}

#[tokio::test]
async fn an_unknown_project_is_an_error() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let r = call_tool(addr, "projects.get", json!({"ws_id": "ws_nope"})).await;
    assert!(is_error(&r), "{r}");
}
