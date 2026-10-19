//! A test-only [`ToolSource`] and helpers for driving an [`McpServer`].

use async_trait::async_trait;
use rupu_mcp::{
    CallCx, InProcessTransport, McpError, McpServer, ToolDef, ToolFilter, ToolHints, ToolResult,
    ToolSource, Transport,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::task::JoinHandle;

/// Calls of `stub.touch` that ran to completion.
pub static TOUCHED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `stub.echo` returns its args; `stub.sleep` sleeps `args.ms`; `stub.ro` is
/// a read-only no-op.
pub struct StubSource;

#[async_trait]
impl ToolSource for StubSource {
    fn tools(&self) -> Vec<ToolDef> {
        let def = |name: &str, hints: ToolHints| ToolDef {
            name: name.into(),
            description: format!("{name} (stub)"),
            input_schema: json!({"type": "object"}),
            hints,
        };
        vec![
            def("stub.echo", ToolHints::default()),
            def("stub.sleep", ToolHints::default()),
            def("stub.panic", ToolHints::default()),
            def("stub.touch", ToolHints::default()),
            def(
                "stub.ro",
                ToolHints {
                    read_only: true,
                    idempotent: true,
                    ..ToolHints::default()
                },
            ),
        ]
    }

    async fn call(&self, name: &str, args: Value, _cx: &CallCx) -> Option<ToolResult> {
        match name {
            "stub.echo" => Some(ToolResult::Json(args)),
            "stub.sleep" => {
                let ms = args["ms"].as_u64().unwrap_or(0);
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                Some(ToolResult::Text("slept".into()))
            }
            "stub.panic" => panic!("stub tool panics"),
            "stub.touch" => {
                let ms = args["ms"].as_u64().unwrap_or(0);
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                TOUCHED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Some(ToolResult::Text("touched".into()))
            }
            "stub.ro" => Some(ToolResult::Text("ro".into())),
            _ => None,
        }
    }
}

pub fn stub_server() -> Arc<McpServer> {
    Arc::new(McpServer::new(vec![Arc::new(StubSource)], &ToolFilter::all()).unwrap())
}

pub fn serve_stub() -> (InProcessTransport, JoinHandle<Result<(), McpError>>) {
    let (client, server_t) = InProcessTransport::pair();
    (client, tokio::spawn(stub_server().run(server_t)))
}

pub async fn request(client: &InProcessTransport, id: u64, method: &str, params: Value) -> Value {
    client
        .send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
        .await
        .unwrap();
    let resp = client.recv().await.unwrap().unwrap();
    assert_eq!(resp["id"], id);
    resp
}

/// The next message, or a panic after 10 s.
pub async fn next(client: &InProcessTransport) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(10), client.recv())
        .await
        .expect("a reply within 10 s")
        .unwrap()
        .unwrap()
}
