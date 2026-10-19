//! Session-keyed cancel, annotations and structured results.

use crate::stub::stub_server;
use serde_json::{json, Value};
use std::time::Duration;

fn sleep_call(ms: u64) -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "stub.sleep", "arguments": { "ms": ms } } })
}

#[tokio::test]
async fn a_cancel_only_reaches_its_own_session() {
    let server = stub_server();
    let a = tokio::spawn({
        let s = server.clone();
        async move { s.handle(sleep_call(2000), Some("a")).await }
    });
    let b = tokio::spawn({
        let s = server.clone();
        async move { s.handle(sleep_call(300), Some("b")).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let cancel = json!({ "jsonrpc": "2.0", "method": "notifications/cancelled",
                         "params": { "requestId": 1 } });
    assert!(server.handle(cancel, Some("a")).await.is_none());
    let b = b.await.unwrap().expect("b's call is answered");
    assert_eq!(b["id"], 1);
    assert_eq!(b["result"]["content"][0]["text"], "slept");
    let a = tokio::time::timeout(Duration::from_secs(5), a)
        .await
        .expect("a's call ended")
        .unwrap();
    assert!(a.is_none(), "a cancelled call gets no reply: {a:?}");
}

#[tokio::test]
async fn tools_list_carries_annotations() {
    let server = stub_server();
    let resp = server
        .handle(
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
            None,
        )
        .await
        .unwrap();
    let tools = resp["result"]["tools"].as_array().unwrap();
    let ro = tools.iter().find(|t| t["name"] == "stub.ro").unwrap();
    assert_eq!(ro["annotations"]["readOnlyHint"], true);
    assert_eq!(ro["annotations"]["idempotentHint"], true);
    let echo = tools.iter().find(|t| t["name"] == "stub.echo").unwrap();
    assert_eq!(echo["annotations"]["readOnlyHint"], false);
    assert_eq!(echo["inputSchema"], json!({"type": "object"}));
}

#[tokio::test]
async fn json_results_carry_structured_content() {
    let server = stub_server();
    let resp = server
        .handle(
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": { "name": "stub.echo", "arguments": { "x": 1 } } }),
            None,
        )
        .await
        .unwrap();
    assert_eq!(resp["result"]["structuredContent"], json!({"x": 1}));
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(text).unwrap(),
        json!({"x": 1})
    );
}
