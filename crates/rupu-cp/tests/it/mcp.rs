//! `/mcp` on `cp serve`: the HTTP endpoint, its auth and origin rules, and
//! the serve modes (`--no-web`, `--no-mcp`).

#![allow(clippy::disallowed_methods)]

use async_trait::async_trait;
use rupu_cp::mcp::McpOpts;
use rupu_cp::server::{router_with, RouterOpts};
use rupu_mcp::{CallCx, ToolDef, ToolHints, ToolResult, ToolSource};
use serde_json::{json, Value};
use std::sync::Arc;

fn init_body() -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "t", "version": "0"}}})
}

fn serve_opts(dir: &std::path::Path) -> rupu_cp::ServeOpts {
    rupu_cp::ServeOpts {
        bind: "127.0.0.1:0".parse().unwrap(),
        token: None,
        global_dir: dir.to_path_buf(),
        open_browser: false,
        launcher: None,
        session_sender: None,
        repos: None,
        model_catalog: None,
        agent_launcher: None,
        session_starter: None,
        generator: None,
        session_mutator: None,
        transcript_mutator: None,
        inventory: None,
        web: true,
        mcp: None,
    }
}

async fn spawn_with(token: Option<&str>, web: bool, mcp: Option<McpOpts>) -> std::net::SocketAddr {
    let dir = tempfile::tempdir().unwrap().keep();
    let state = rupu_cp::build_state(&serve_opts(&dir));
    let server = mcp.map(|o| rupu_cp::mcp::build_server(&state, &o).unwrap());
    let app = router_with(
        state,
        token.map(str::to_string),
        RouterOpts { web, mcp: server },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn plain_mcp() -> Option<McpOpts> {
    Some(McpOpts {
        catalog: None,
        tools: vec![],
    })
}

#[tokio::test]
async fn initialize_returns_a_session_header_and_tools_capability() {
    let addr = spawn_with(None, true, plain_mcp()).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .json(&init_body())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let sid = resp
        .headers()
        .get("mcp-session-id")
        .expect("session header")
        .to_str()
        .unwrap()
        .to_string();
    assert!(sid.starts_with("mcp_"), "{sid}");
    let v: Value = resp.json().await.unwrap();
    assert!(v["result"]["capabilities"]["tools"].is_object(), "{v}");
}

#[tokio::test]
async fn get_on_mcp_is_405() {
    let addr = spawn_with(None, true, plain_mcp()).await;
    let resp = reqwest::get(format!("http://{addr}/mcp")).await.unwrap();
    assert_eq!(resp.status(), 405);
}

#[tokio::test]
async fn notification_only_post_is_202() {
    let addr = spawn_with(None, true, plain_mcp()).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);
    assert!(resp.text().await.unwrap().is_empty());
}

#[tokio::test]
async fn unparseable_body_is_400_with_a_parse_error() {
    let addr = spawn_with(None, true, plain_mcp()).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .body("{nope")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["error"]["code"], -32700);
}

#[tokio::test]
async fn cross_origin_is_refused() {
    let addr = spawn_with(None, true, plain_mcp()).await;
    let c = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");
    let evil = c
        .post(&url)
        .header("Origin", "http://evil.example")
        .json(&init_body())
        .send()
        .await
        .unwrap();
    assert_eq!(evil.status(), 403);
    let own = c
        .post(&url)
        .header("Origin", format!("http://{addr}"))
        .json(&init_body())
        .send()
        .await
        .unwrap();
    assert_eq!(own.status(), 200);
    let none = c.post(&url).json(&init_body()).send().await.unwrap();
    assert_eq!(none.status(), 200);
}

#[tokio::test]
async fn bearer_required_when_token_set() {
    let addr = spawn_with(Some("t"), true, plain_mcp()).await;
    let c = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");
    let no = c.post(&url).json(&init_body()).send().await.unwrap();
    assert_eq!(no.status(), 401);
    let yes = c
        .post(&url)
        .header("Authorization", "Bearer t")
        .json(&init_body())
        .send()
        .await
        .unwrap();
    assert_eq!(yes.status(), 200);
}

/// A tool that signals when a call starts and then outlasts the test unless
/// cancelled.
struct Sleeper(Arc<tokio::sync::Semaphore>);

#[async_trait]
impl ToolSource for Sleeper {
    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "sleep".into(),
            description: "sleeps".into(),
            input_schema: json!({"type": "object"}),
            hints: ToolHints::default(),
        }]
    }
    async fn call(&self, name: &str, _args: Value, _cx: &CallCx) -> Option<ToolResult> {
        if name != "sleep" {
            return None;
        }
        self.0.add_permits(1);
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        Some(ToolResult::Text("woke".into()))
    }
}

#[tokio::test]
async fn cancel_is_scoped_to_the_session() {
    let started = Arc::new(tokio::sync::Semaphore::new(0));
    let addr = spawn_with(
        None,
        true,
        Some(McpOpts {
            catalog: Some(Arc::new(Sleeper(Arc::clone(&started)))),
            tools: vec![],
        }),
    )
    .await;
    let c = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "sleep", "arguments": {}}});
    let post = |sid: &'static str, body: Value| {
        let c = c.clone();
        let url = url.clone();
        async move {
            c.post(&url)
                .header("Mcp-Session-Id", sid)
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    };
    let a = tokio::spawn(post("mcp_A", call.clone()));
    let b = tokio::spawn(post("mcp_B", call.clone()));
    // Both calls are running before the cancel is sent.
    started.acquire_many(2).await.unwrap().forget();
    // Session A cancels request id 1: only A's call is stopped.
    let cancel = json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": {"requestId": 1}});
    assert_eq!(post("mcp_A", cancel).await.status(), 202);
    let a = a.await.unwrap();
    let b = b.await.unwrap();
    let bv: Value = b.json().await.unwrap();
    assert_eq!(bv["result"]["content"][0]["text"], "woke", "{bv}");
    // A's call was cancelled: no reply body (202).
    assert_eq!(a.status(), 202);
}

#[tokio::test]
async fn no_web_serves_api_and_mcp_but_not_the_spa() {
    let addr = spawn_with(None, false, plain_mcp()).await;
    let c = reqwest::Client::new();
    assert_eq!(
        c.get(format!("http://{addr}/"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        c.get(format!("http://{addr}/healthz"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        c.post(format!("http://{addr}/mcp"))
            .json(&init_body())
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
}

#[tokio::test]
async fn web_only_has_no_mcp() {
    let addr = spawn_with(None, true, None).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .json(&init_body())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["error"], "MCP is off (--no-mcp)");
}

async fn init_with_headers(
    addr: std::net::SocketAddr,
    host: Option<String>,
    origin: Option<String>,
) -> u16 {
    let mut req = reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .json(&init_body());
    if let Some(h) = host {
        req = req.header("Host", h);
    }
    if let Some(o) = origin {
        req = req.header("Origin", o);
    }
    req.send().await.unwrap().status().as_u16()
}

#[tokio::test]
async fn rebinding_host_is_refused_on_a_loopback_bind() {
    let addr = spawn_with(None, true, plain_mcp()).await;
    let host = format!("evil.example:{}", addr.port());
    assert_eq!(init_with_headers(addr, Some(host.clone()), None).await, 403);
    // Origin == Host is not enough on its own.
    assert_eq!(
        init_with_headers(addr, Some(host.clone()), Some(format!("http://{host}"))).await,
        403
    );
}

#[tokio::test]
async fn localhost_host_is_served_on_a_loopback_bind() {
    let addr = spawn_with(None, true, plain_mcp()).await;
    for host in ["localhost", "LOCALHOST:7878", "127.0.0.1:9", "[::1]:80"] {
        assert_eq!(
            init_with_headers(addr, Some(host.into()), None).await,
            200,
            "{host}"
        );
    }
    // A different port than the CP's, Origin and Host agreeing: served.
    assert_eq!(
        init_with_headers(
            addr,
            Some("localhost:3000".into()),
            Some("http://localhost:3000".into())
        )
        .await,
        200
    );
    // Different port between Origin and Host: refused.
    assert_eq!(
        init_with_headers(
            addr,
            Some("localhost:3000".into()),
            Some("http://localhost:3001".into())
        )
        .await,
        403
    );
}

#[tokio::test]
async fn null_origin_is_refused() {
    let addr = spawn_with(None, true, plain_mcp()).await;
    assert_eq!(
        init_with_headers(addr, None, Some("null".into())).await,
        403
    );
}

#[tokio::test]
async fn delete_on_mcp_is_405() {
    let addr = spawn_with(None, true, plain_mcp()).await;
    let resp = reqwest::Client::new()
        .delete(format!("http://{addr}/mcp"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 405);
}

#[tokio::test]
async fn no_web_keeps_the_json_404_for_api_paths() {
    let addr = spawn_with(None, false, plain_mcp()).await;
    let c = reqwest::Client::new();
    let resp = c
        .get(format!("http://{addr}/api/nope"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let v: Value = resp.json().await.unwrap();
    assert!(v["error"].as_str().unwrap().contains("/api/nope"), "{v}");
    let plain = c.get(format!("http://{addr}/nope")).send().await.unwrap();
    assert_eq!(plain.status(), 404);
    assert!(plain.text().await.unwrap().is_empty());
}

#[tokio::test]
async fn serve_on_refuses_nothing_to_serve() {
    let dir = tempfile::tempdir().unwrap();
    let mut opts = serve_opts(dir.path());
    opts.web = false;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let err = rupu_cp::serve_on(listener, opts).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "nothing to serve: --no-web and --no-mcp together"
    );
}
