//! The protocol core over a stub source (ported from the `CatalogServer`
//! tests).

use crate::stub::{next, request, serve_stub};
use rupu_mcp::{McpServer, ServeError, ToolFilter, Transport};
use serde_json::{json, Value};
use std::sync::Arc;

#[tokio::test]
async fn protocol_errors_and_notifications() {
    let (client, handle) = serve_stub();
    let init = request(&client, 1, "initialize", Value::Null).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "rupu");
    // A notification gets no reply: the next reply is the next request's.
    client
        .send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await
        .unwrap();
    let r = request(
        &client,
        2,
        "tools/call",
        json!({"name": "stub.typo", "arguments": {}}),
    )
    .await;
    assert_eq!(r["result"]["isError"], true);
    assert!(r["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("unknown tool: stub.typo"));
    let resp = request(&client, 4, "bogus/method", Value::Null).await;
    assert_eq!(resp["error"]["code"], -32601);
    drop(client);
    let _ = handle.await;
}

#[tokio::test]
async fn ping_answers_an_empty_result() {
    let (client, handle) = serve_stub();
    let resp = request(&client, 3, "ping", Value::Null).await;
    assert_eq!(resp, json!({ "jsonrpc": "2.0", "id": 3, "result": {} }));
    drop(client);
    let _ = handle.await;
}

#[tokio::test]
async fn a_malformed_line_is_answered_and_the_server_keeps_serving() {
    let (client, handle) = serve_stub();
    client
        .send_raw("{\"jsonrpc\": \"2.0\", \"id\": 1,")
        .unwrap();
    let resp = next(&client).await;
    assert_eq!(resp["id"], Value::Null);
    assert_eq!(resp["error"]["code"], -32700, "{resp}");
    // Blank lines are skipped, not answered.
    client.send_raw("").unwrap();
    client.send_raw("   ").unwrap();
    let list = request(&client, 1, "tools/list", Value::Null).await;
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 9);
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
    let (client, handle) = serve_stub();
    client
        .send(json!([
            { "jsonrpc": "2.0", "id": "c", "method": "tools/call",
              "params": { "name": "stub.echo", "arguments": { "x": "alpha" } } },
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
    let (client, handle) = serve_stub();
    let latest = rupu_mcp::PROTOCOL_VERSIONS[0];
    let cases: [(Value, &str); 6] = [
        (json!({ "protocolVersion": "2024-11-05" }), "2024-11-05"),
        (json!({ "protocolVersion": "2025-03-26" }), "2025-03-26"),
        (json!({ "protocolVersion": "2025-06-18" }), "2025-06-18"),
        (json!({ "protocolVersion": "2025-11-25" }), "2025-11-25"),
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
    let (client, handle) = serve_stub();
    let sleep = |id: &str, ms: u64| {
        json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": { "name": "stub.sleep", "arguments": { "ms": ms } } })
    };
    // A slow call does not hold up a ping or a second call.
    client.send(sleep("slow", 60_000)).await.unwrap();
    client
        .send(json!({ "jsonrpc": "2.0", "id": "p", "method": "ping" }))
        .await
        .unwrap();
    assert_eq!(next(&client).await["id"], "p");
    client.send(sleep("fast", 1)).await.unwrap();
    assert_eq!(next(&client).await["id"], "fast");
    // Cancelling the slow call aborts it: it is never answered.
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
    client.send(sleep("after", 1)).await.unwrap();
    assert_eq!(next(&client).await["id"], "after");
    drop(client);
    tokio::time::timeout(std::time::Duration::from_secs(10), handle)
        .await
        .expect("the cancelled call no longer runs")
        .unwrap()
        .unwrap();
}

#[test]
fn a_tool_name_served_twice_is_refused() {
    let r = McpServer::new(
        vec![
            Arc::new(crate::stub::StubSource),
            Arc::new(crate::stub::StubSource),
        ],
        &ToolFilter::all(),
    );
    assert!(matches!(r, Err(ServeError::Duplicate(n)) if n == "stub.echo"));
}
