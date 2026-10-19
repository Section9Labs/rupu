//! `mcp_serve_lists_grant` (W4 §6.3): `rupu mcp serve` lists exactly its
//! grant's tools and calls them through the catalog.

use rupu_mcp::{CatalogServer, InProcessTransport, Transport, MCP_DEFAULT_GRANT};
use rupu_tools::{PermissionMode, ToolCatalog, ToolContext, Unavailable};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tokio::task::JoinHandle;

/// A server over `workspace` with an (empty) SCM registry, serving `tools`.
pub fn server(
    workspace: &Path,
    tools: Option<&[&str]>,
    mode: PermissionMode,
) -> (
    InProcessTransport,
    JoinHandle<Result<(), rupu_mcp::McpError>>,
    Vec<Unavailable>,
) {
    let mut ctx = ToolContext::in_workspace(workspace);
    ctx.services.scm = Some(Arc::new(rupu_scm::Registry::default()));
    let id = ctx.identity_mut();
    id.run_id = "mcp_test".into();
    id.scope_name = Some("mcp".into());
    let tools: Option<Vec<String>> = tools.map(|t| t.iter().map(|s| s.to_string()).collect());
    let (client, server_t) = InProcessTransport::pair();
    let (server, unavailable) =
        CatalogServer::new(server_t, ctx, mode, tools.as_deref()).expect("grant resolves");
    (client, tokio::spawn(server.run()), unavailable)
}

async fn request(client: &InProcessTransport, id: u64, method: &str, params: Value) -> Value {
    client
        .send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
        .await
        .unwrap();
    let resp = client.recv().await.unwrap().unwrap();
    assert_eq!(resp["id"], id);
    resp
}

pub async fn list(client: &InProcessTransport) -> Vec<Value> {
    request(client, 1, "tools/list", Value::Null).await["result"]["tools"]
        .as_array()
        .unwrap()
        .clone()
}

fn names(tools: &[Value]) -> Vec<String> {
    tools
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

async fn call(client: &InProcessTransport, name: &str, args: Value) -> (bool, String) {
    let r = request(
        client,
        7,
        "tools/call",
        json!({ "name": name, "arguments": args }),
    )
    .await;
    (
        r["result"]["isError"] == true,
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string(),
    )
}

#[tokio::test]
async fn default_flags_list_exactly_the_default_grant() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (client, handle, unavailable) = server(tmp.path(), None, PermissionMode::Bypass);
    assert!(unavailable.is_empty(), "{unavailable:?}");
    let want: Vec<&str> = ToolCatalog::all()
        .iter()
        .map(|d| d.name)
        .filter(|n| {
            MCP_DEFAULT_GRANT
                .iter()
                .any(|g| n.starts_with(g.trim_end_matches('*')))
        })
        .collect();
    assert_eq!(names(&list(&client).await), want);
    assert!(want.contains(&"scm.prs.create") && want.contains(&"findings.report"));
    assert!(!want
        .iter()
        .any(|n| n.starts_with("coverage.") || *n == "bash"));
    drop(client);
    let _ = handle.await;
}

#[tokio::test]
async fn tools_read_file_serves_the_cwd() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("hello.txt"), "hi there\n").unwrap();
    let (client, handle, _) = server(tmp.path(), Some(&["read_file"]), PermissionMode::Bypass);
    assert_eq!(names(&list(&client).await), ["read_file"]);
    let (is_error, text) = call(&client, "read_file", json!({ "path": "hello.txt" })).await;
    assert!(!is_error, "{text}");
    assert!(text.contains("hi there"), "{text}");
    // A catalog tool outside the grant is not served.
    let (is_error, text) = call(&client, "findings.query", json!({})).await;
    assert!(is_error && text.contains("not served here"), "{text}");
    drop(client);
    let _ = handle.await;
}

#[tokio::test]
async fn a_tool_whose_service_is_missing_is_reported_and_not_listed() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (client, handle, unavailable) = server(
        tmp.path(),
        Some(&["dispatch", "read_file"]),
        PermissionMode::Bypass,
    );
    assert_eq!(unavailable.len(), 1);
    assert_eq!(unavailable[0].tool, "dispatch");
    assert!(
        unavailable[0].notice_message().contains("launcher"),
        "{}",
        unavailable[0].notice_message()
    );
    assert_eq!(names(&list(&client).await), ["read_file"]);
    drop(client);
    let _ = handle.await;
}

#[tokio::test]
async fn findings_record_alias_records_through_the_one_findings_tool() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (client, handle, _) = server(tmp.path(), None, PermissionMode::Readonly);
    // Record is allowed under readonly (D4); a legacy name still resolves.
    let report: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap();
    let (is_error, text) = call(
        &client,
        "findings.record",
        json!({ "scope": "repo", "report": report }),
    )
    .await;
    assert!(!is_error, "{text}");
    assert!(text.starts_with("finding_id: fnd_"), "{text}");
    let paths =
        rupu_coverage::CoveragePaths::new(tmp.path(), &rupu_coverage::target_id(tmp.path(), "mcp"));
    let recs = rupu_coverage::read_findings(&paths).unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].declared_by.run_id, "mcp_test");
    // External is denied under readonly, before any connector is reached.
    let (is_error, text) = call(
        &client,
        "scm.prs.create",
        json!({"owner": "o", "repo": "r", "title": "t", "body": "b", "head": "h", "base": "main"}),
    )
    .await;
    assert!(is_error, "{text}");
    assert!(
        text.contains("readonly mode blocks external tools"),
        "{text}"
    );
    drop(client);
    let _ = handle.await;
}

#[tokio::test]
async fn protocol_errors_and_notifications() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (client, handle, _) = server(tmp.path(), None, PermissionMode::Bypass);
    let init = request(&client, 1, "initialize", Value::Null).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "rupu");
    // A notification gets no reply: the next reply is the next request's.
    client
        .send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await
        .unwrap();
    let (is_error, text) = call(&client, "scm.repo.typo", json!({})).await;
    assert!(is_error && text.contains("unknown tool"), "{text}");
    let resp = request(&client, 4, "bogus/method", Value::Null).await;
    assert_eq!(resp["error"]["code"], -32601);
    drop(client);
    let _ = handle.await;
}

/// The next message, or a panic after 10 s.
async fn next(client: &InProcessTransport) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(10), client.recv())
        .await
        .expect("a reply within 10 s")
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn ping_answers_an_empty_result() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (client, handle, _) = server(tmp.path(), None, PermissionMode::Bypass);
    let resp = request(&client, 3, "ping", Value::Null).await;
    assert_eq!(resp, json!({ "jsonrpc": "2.0", "id": 3, "result": {} }));
    drop(client);
    let _ = handle.await;
}

#[tokio::test]
async fn a_malformed_line_is_answered_and_the_server_keeps_serving() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (client, handle, _) = server(tmp.path(), Some(&["read_file"]), PermissionMode::Bypass);
    client
        .send_raw("{\"jsonrpc\": \"2.0\", \"id\": 1,")
        .unwrap();
    let resp = next(&client).await;
    assert_eq!(resp["id"], Value::Null);
    assert_eq!(resp["error"]["code"], -32700, "{resp}");
    // Blank lines are skipped, not answered.
    client.send_raw("").unwrap();
    client.send_raw("   ").unwrap();
    assert_eq!(names(&list(&client).await), ["read_file"]);
    // JSON that is no JSON-RPC message is an invalid request.
    client.send(json!(42)).await.unwrap();
    assert_eq!(next(&client).await["error"]["code"], -32600);
    client.send(json!([])).await.unwrap();
    assert_eq!(next(&client).await["error"]["code"], -32600);
    assert!(!handle.is_finished());
    drop(client);
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_batch_is_answered_with_one_array_in_order() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "alpha\n").unwrap();
    let (client, handle, _) = server(tmp.path(), Some(&["read_file"]), PermissionMode::Bypass);
    client
        .send(json!([
            { "jsonrpc": "2.0", "id": "c", "method": "tools/call",
              "params": { "name": "read_file", "arguments": { "path": "a.txt" } } },
            { "jsonrpc": "2.0", "method": "notifications/initialized" },
            { "jsonrpc": "2.0", "id": "p", "method": "ping" },
        ]))
        .await
        .unwrap();
    let resp = next(&client).await;
    let replies = resp.as_array().expect("an array reply");
    assert_eq!(replies.len(), 2, "the notification gets no reply: {resp}");
    assert_eq!(replies[0]["id"], "c");
    assert!(
        replies[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("alpha"),
        "{resp}"
    );
    assert_eq!(
        replies[1],
        json!({ "jsonrpc": "2.0", "id": "p", "result": {} })
    );
    drop(client);
    let _ = handle.await;
}

#[tokio::test]
async fn initialize_negotiates_the_protocol_version() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (client, handle, _) = server(tmp.path(), None, PermissionMode::Bypass);
    let latest = rupu_mcp::PROTOCOL_VERSIONS[0];
    let cases: [(Value, &str); 6] = [
        (json!({ "protocolVersion": "2024-11-05" }), "2024-11-05"),
        (json!({ "protocolVersion": "2025-03-26" }), "2025-03-26"),
        (json!({ "protocolVersion": "2025-06-18" }), "2025-06-18"),
        (json!({ "protocolVersion": "2025-11-25" }), "2025-11-25"),
        // One we don't speak (the stateless 2026-07-28 drops `initialize`),
        // or none at all: our newest.
        (json!({ "protocolVersion": "2026-07-28" }), latest),
        (Value::Null, latest),
    ];
    for (i, (params, want)) in cases.into_iter().enumerate() {
        let resp = request(&client, i as u64, "initialize", params.clone()).await;
        assert_eq!(resp["result"]["protocolVersion"], want, "{params}");
        assert_eq!(resp["result"]["capabilities"], json!({ "tools": {} }));
    }
    assert_eq!(latest, "2025-11-25");
    drop(client);
    let _ = handle.await;
}

#[tokio::test]
async fn calls_run_concurrently_and_a_cancel_aborts_one() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (client, handle, _) = server(tmp.path(), Some(&["bash"]), PermissionMode::Bypass);
    let bash = |id: &str, command: &str| {
        json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": { "name": "bash", "arguments": { "command": command } } })
    };
    // A slow call does not hold up a ping or a second call. (`exec`, so the
    // abort's kill reaches the sleep itself.)
    client.send(bash("slow", "exec sleep 60")).await.unwrap();
    client
        .send(json!({ "jsonrpc": "2.0", "id": "p", "method": "ping" }))
        .await
        .unwrap();
    assert_eq!(next(&client).await["id"], "p");
    client.send(bash("fast", "echo quick")).await.unwrap();
    let fast = next(&client).await;
    assert_eq!(fast["id"], "fast");
    assert!(
        fast["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("quick"),
        "{fast}"
    );
    // Cancelling the slow call aborts it: it is never answered, and the
    // server goes on serving.
    client
        .send(
            json!({ "jsonrpc": "2.0", "method": "notifications/cancelled",
                      "params": { "requestId": "slow", "reason": "user" } }),
        )
        .await
        .unwrap();
    // A cancel for a request that isn't running is ignored.
    client
        .send(
            json!({ "jsonrpc": "2.0", "method": "notifications/cancelled",
                      "params": { "requestId": "fast" } }),
        )
        .await
        .unwrap();
    client.send(bash("after", "echo next")).await.unwrap();
    assert_eq!(next(&client).await["id"], "after");
    drop(client);
    // Nothing is left running to drain: the server ends right away rather
    // than after the 60 s sleep.
    tokio::time::timeout(std::time::Duration::from_secs(10), handle)
        .await
        .expect("the cancelled call no longer runs")
        .unwrap()
        .unwrap();
}
