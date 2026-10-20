//! The sessions-area operator tools: listing and parity with plain HTTP.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use super::mcp::{assert_required_args, call_tool, json_of, rpc, spawn_operator_with, OP_SESSION};
use serde_json::{json, Value};
use std::net::SocketAddr;

const ARCHIVED: &str = "ses_01J9ZQ3K4M5N6P7Q8R9S0T1ARC";

fn seed_archived(dir: &std::path::Path) {
    let d = dir.join("sessions-archive").join(ARCHIVED);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("session.json"),
        json!({"session_id": ARCHIVED, "status": "active"}).to_string(),
    )
    .unwrap();
}

async fn http_get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let r = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn sessions_tools_are_listed_with_their_required_arguments() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let list = rpc(addr, "tools/list", json!({})).await["result"]["tools"].clone();
    let table: &[(&str, &[&str])] = &[
        ("sessions.list", &[]),
        ("sessions.get", &["id"]),
        ("sessions.turns", &["id"]),
        ("sessions.usage_timeline", &["id"]),
        ("sessions.archive", &["id"]),
        ("sessions.restore", &["id"]),
        ("sessions.delete", &["id"]),
    ];
    assert_required_args(addr, table).await;
    let hint = |name: &str, key: &str| -> Value {
        list.as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap()["annotations"][key]
            .clone()
    };
    assert_eq!(hint("sessions.delete", "destructiveHint"), json!(true));
    assert_eq!(hint("sessions.list", "readOnlyHint"), json!(true));
}

#[tokio::test]
async fn sessions_reads_answer_what_the_api_answers() {
    let (addr, _tmp) = spawn_operator_with(seed_archived, None).await;
    let cases: &[(&str, Value, String)] = &[
        ("sessions.list", json!({}), "/api/sessions".into()),
        (
            "sessions.get",
            json!({"id": OP_SESSION}),
            format!("/api/sessions/{OP_SESSION}"),
        ),
        (
            "sessions.turns",
            json!({"id": OP_SESSION}),
            format!("/api/sessions/{OP_SESSION}/runs"),
        ),
        (
            "sessions.usage_timeline",
            json!({"id": OP_SESSION}),
            format!("/api/sessions/{OP_SESSION}/usage-timeline"),
        ),
    ];
    for (tool, args, path) in cases {
        let r = call_tool(addr, tool, args.clone()).await;
        let (status, want) = http_get(addr, path).await;
        assert_eq!(status, 200, "{tool}: {path}");
        assert_ne!(r["isError"], json!(true), "{tool}: {r}");
        assert_eq!(json_of(&r), want, "{tool}");
    }
}

#[tokio::test]
async fn sessions_list_with_a_query_filters_like_the_api() {
    let (addr, _tmp) = spawn_operator_with(seed_archived, None).await;
    let ids = |v: &Value| -> Vec<String> {
        let mut ids: Vec<String> = v
            .as_array()
            .unwrap_or_else(|| panic!("not an array: {v}"))
            .iter()
            .map(|r| r["session_id"].as_str().unwrap().to_string())
            .collect();
        ids.sort();
        ids
    };
    let all = json_of(&call_tool(addr, "sessions.list", json!({"q": "is:archived"})).await);
    let (s, want) = http_get(addr, "/api/sessions?q=is:archived").await;
    assert_eq!(s, 200);
    assert_eq!(ids(&all), ids(&want));
    assert_eq!(ids(&all), vec![ARCHIVED.to_string()]);
    let active = json_of(&call_tool(addr, "sessions.list", json!({"q": "is:active"})).await);
    let (_, want) = http_get(addr, "/api/sessions?q=is:active").await;
    assert_eq!(ids(&active), ids(&want));
    // The seeded `OP_SESSION` record carries no `session_id`, so count rows.
    assert_eq!(active.as_array().unwrap().len(), 1, "{active}");
    assert!(!ids(&active).contains(&ARCHIVED.to_string()), "{active}");
}
