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

// ---- operator tools (in-process calls over the CP API) ----

struct NoopLauncher;

#[async_trait]
impl rupu_cp::launcher::RunLauncher for NoopLauncher {
    async fn launch(
        &self,
        _req: rupu_cp::launcher::LaunchRequest,
    ) -> Result<String, rupu_cp::launcher::LaunchError> {
        Ok("run_unused".into())
    }
}

const OP_RUN: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0T1OPS";
const OP_SESSION: &str = "ses_01J9ZQ3K4M5N6P7Q8R9S0T1OPS";

/// A writable CP with one run and one live session, serving `/mcp`.
async fn spawn_operator() -> (std::net::SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let store = rupu_orchestrator::runs::RunStore::new(dir.join("runs"));
    store
        .create(
            super::sse::seed_run(OP_RUN, rupu_orchestrator::runs::RunStatus::Completed),
            "name: test\nsteps: []\n",
        )
        .unwrap();
    let session = dir.join("sessions").join(OP_SESSION);
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(session.join("session.json"), r#"{"status":"active"}"#).unwrap();
    let state = rupu_cp::state::AppState::new(dir.clone(), rupu_config::PricingConfig::default())
        .with_launcher(Some(Arc::new(NoopLauncher)));
    let server = rupu_cp::mcp::build_server(
        &state,
        &McpOpts {
            catalog: None,
            tools: vec![],
        },
    )
    .unwrap();
    let app = router_with(
        state,
        None,
        RouterOpts {
            web: true,
            mcp: Some(server),
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, tmp)
}

async fn rpc(addr: std::net::SocketAddr, method: &str, params: Value) -> Value {
    reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn call_tool(addr: std::net::SocketAddr, name: &str, args: Value) -> Value {
    rpc(addr, "tools/call", json!({"name": name, "arguments": args})).await["result"].clone()
}

fn text_of(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

const OPERATOR_TOOLS: [&str; 13] = [
    "runs.list",
    "runs.get",
    "runs.approve",
    "runs.reject",
    "runs.cancel",
    "workflows.list",
    "workflows.run",
    "agents.list",
    "agents.run",
    "sessions.start",
    "sessions.send",
    "messages.list",
    "messages.send",
];

#[tokio::test]
async fn tools_list_has_the_13_operator_tools_with_schemas() {
    let (addr, _tmp) = spawn_operator().await;
    let v = rpc(addr, "tools/list", json!({})).await;
    let tools = v["result"]["tools"].as_array().unwrap();
    let mut names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    names.sort();
    let mut want = OPERATOR_TOOLS.to_vec();
    want.sort();
    assert_eq!(names, want);
    let approve = tools.iter().find(|t| t["name"] == "runs.approve").unwrap();
    let required = approve["inputSchema"]["required"].as_array().unwrap();
    assert!(required.contains(&json!("id")), "{approve}");
    let props = approve["inputSchema"]["properties"].as_object().unwrap();
    assert!(
        props.contains_key("gate") && props.contains_key("mode"),
        "{approve}"
    );
}

#[tokio::test]
async fn runs_get_returns_the_run_detail_as_structured_content() {
    let (addr, _tmp) = spawn_operator().await;
    let r = call_tool(addr, "runs.get", json!({"id": OP_RUN})).await;
    assert_ne!(r["isError"], json!(true), "{r}");
    assert_eq!(r["structuredContent"]["run"]["id"], OP_RUN, "{r}");
}

#[tokio::test]
async fn a_missing_required_argument_names_it() {
    let (addr, _tmp) = spawn_operator().await;
    let r = call_tool(addr, "runs.get", json!({})).await;
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(text_of(&r).contains("`id`"), "{r}");
}

#[tokio::test]
async fn a_traversal_id_never_reaches_another_route() {
    let (addr, _tmp) = spawn_operator().await;
    let r = call_tool(addr, "runs.get", json!({"id": "../../config"})).await;
    assert_eq!(r["isError"], json!(true), "{r}");
    let t = text_of(&r);
    assert!(t.contains("HTTP 4"), "{t}");
    assert!(!t.contains("\"providers\""), "{t}");
}

#[tokio::test]
async fn an_http_error_keeps_its_status_and_body() {
    let (addr, _tmp) = spawn_operator().await;
    let r = call_tool(addr, "runs.get", json!({"id": "run_nope"})).await;
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(text_of(&r).starts_with("HTTP 404"), "{r}");
}

#[tokio::test]
async fn messages_send_picks_the_route_from_the_id_prefix() {
    let (addr, _tmp) = spawn_operator().await;
    let r = call_tool(
        addr,
        "messages.send",
        json!({"id": OP_SESSION, "body": "hi"}),
    )
    .await;
    assert_ne!(r["isError"], json!(true), "{r}");
    assert!(
        r["structuredContent"]["id"]
            .as_str()
            .unwrap_or("")
            .starts_with("msg_"),
        "{r}"
    );
}

#[tokio::test]
async fn reject_and_cancel_are_marked_destructive() {
    let (addr, _tmp) = spawn_operator().await;
    let v = rpc(addr, "tools/list", json!({})).await;
    let tools = v["result"]["tools"].as_array().unwrap();
    for name in ["runs.reject", "runs.cancel"] {
        let t = tools.iter().find(|t| t["name"] == name).unwrap();
        assert_eq!(t["annotations"]["destructiveHint"], json!(true), "{t}");
    }
    let get = tools.iter().find(|t| t["name"] == "runs.get").unwrap();
    assert_eq!(get["annotations"]["readOnlyHint"], json!(true), "{get}");
}

#[tokio::test]
async fn an_unknown_argument_is_refused_naming_it() {
    let (addr, _tmp) = spawn_operator().await;
    let r = call_tool(
        addr,
        "runs.approve",
        json!({"id": OP_RUN, "permission_mode": "readonly"}),
    )
    .await;
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(
        text_of(&r).contains("unknown argument `permission_mode`"),
        "{r}"
    );
}

#[tokio::test]
async fn a_non_scalar_query_value_is_refused() {
    let (addr, _tmp) = spawn_operator().await;
    let r = call_tool(
        addr,
        "messages.list",
        json!({"id": OP_SESSION, "kind": ["direct"]}),
    )
    .await;
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(
        text_of(&r).contains("argument `kind` must be a string, number or boolean"),
        "{r}"
    );
}

#[tokio::test]
async fn null_arguments_are_absent_and_an_empty_id_is_missing() {
    let (addr, _tmp) = spawn_operator().await;
    let r = call_tool(
        addr,
        "messages.send",
        json!({"id": OP_SESSION, "body": "hi", "to": null}),
    )
    .await;
    assert_ne!(r["isError"], json!(true), "{r}");
    let r = call_tool(addr, "runs.get", json!({"id": ""})).await;
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(
        text_of(&r).contains("missing required argument `id`"),
        "{r}"
    );
}

fn log_lines(dir: &std::path::Path) -> Vec<Value> {
    let path = dir
        .join("sessions")
        .join(OP_SESSION)
        .join("messages/log.jsonl");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[tokio::test]
async fn messages_sent_through_mcp_record_client_mcp_and_keep_via_cp() {
    let (addr, tmp) = spawn_operator().await;
    let r = call_tool(
        addr,
        "messages.send",
        json!({"id": OP_SESSION, "body": "from mcp"}),
    )
    .await;
    assert_ne!(r["isError"], json!(true), "{r}");
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/sessions/{OP_SESSION}/messages"))
        .json(&json!({"body": "plain http"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);
    let lines = log_lines(tmp.path());
    let find = |body: &str| lines.iter().find(|l| l["body"] == body).cloned().unwrap();
    let mcp = find("from mcp");
    assert_eq!(mcp["via"], "cp", "{mcp}");
    assert_eq!(mcp["client"], "mcp", "{mcp}");
    let http = find("plain http");
    assert_eq!(http["via"], "cp", "{http}");
    assert!(http.get("client").is_none(), "{http}");
}
