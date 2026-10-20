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

#[tokio::test]
async fn a_non_object_json_result_has_no_structured_content() {
    let server = stub_server();
    for value in [json!([1, 2, 3]), json!("s"), json!(7)] {
        let resp = server
            .handle(
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                        "params": { "name": "stub.echo", "arguments": value.clone() } }),
                None,
            )
            .await
            .unwrap();
        let result = &resp["result"];
        assert!(result.get("structuredContent").is_none(), "{resp}");
        assert_ne!(result["isError"], json!(true), "{resp}");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert_eq!(serde_json::from_str::<Value>(text).unwrap(), value);
    }
}

fn call(id: u64, name: &str, args: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": args } })
}

fn cancel(id: u64) -> Value {
    json!({ "jsonrpc": "2.0", "method": "notifications/cancelled",
            "params": { "requestId": id } })
}

#[tokio::test]
async fn a_finished_call_does_not_unregister_a_newer_call_with_the_same_id() {
    let server = stub_server();
    let older = tokio::spawn({
        let s = server.clone();
        async move {
            s.handle(call(1, "stub.sleep", json!({"ms": 100})), Some("a"))
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let newer = tokio::spawn({
        let s = server.clone();
        async move {
            s.handle(call(1, "stub.sleep", json!({"ms": 5000})), Some("a"))
                .await
        }
    });
    // The older call finishes while the newer one still runs.
    assert!(older.await.unwrap().is_some());
    assert!(server.handle(cancel(1), Some("a")).await.is_none());
    let newer = tokio::time::timeout(Duration::from_secs(2), newer)
        .await
        .expect("the cancel reached the newer call")
        .unwrap();
    assert!(newer.is_none());
}

#[tokio::test]
async fn a_cancel_inside_a_batch_drops_only_that_reply() {
    let server = stub_server();
    let batch = tokio::spawn({
        let s = server.clone();
        async move {
            s.handle(
                json!([
                    call(1, "stub.sleep", json!({"ms": 5000})),
                    call(2, "stub.echo", json!({"x": 2})),
                ]),
                None,
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(server.handle(cancel(1), None).await.is_none());
    let reply = tokio::time::timeout(Duration::from_secs(2), batch)
        .await
        .expect("the batch finished")
        .unwrap()
        .unwrap();
    let replies = reply.as_array().unwrap();
    assert_eq!(replies.len(), 1, "{reply}");
    assert_eq!(replies[0]["id"], 2);
}

#[tokio::test]
async fn a_panicking_tool_gets_an_error_reply() {
    let server = stub_server();
    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        server.handle(call(9, "stub.panic", json!({})), None),
    )
    .await
    .expect("no hang")
    .expect("a reply");
    assert_eq!(reply["id"], 9);
    assert_eq!(reply["result"]["isError"], true);
    assert!(reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("panicked"));
}

#[tokio::test]
async fn a_dropped_handle_future_does_not_stop_the_call() {
    use crate::stub::TOUCHED;
    use std::sync::atomic::Ordering;
    let server = stub_server();
    let before = TOUCHED.load(Ordering::SeqCst);
    let fut = server.handle(call(1, "stub.touch", json!({"ms": 200})), None);
    // Poll briefly, then drop (a client disconnect).
    let _ = tokio::time::timeout(Duration::from_millis(50), fut).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(TOUCHED.load(Ordering::SeqCst), before + 1);
}
