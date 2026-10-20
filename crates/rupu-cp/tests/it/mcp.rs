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
        agentiflow_launcher: None,
        skill_installer: None,
    }
}

/// The CP serving over a fresh global dir, held for the test's lifetime by
/// the returned `TempDir`.
async fn spawn_with(
    token: Option<&str>,
    web: bool,
    mcp: Option<McpOpts>,
) -> (std::net::SocketAddr, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = rupu_cp::build_state(&serve_opts(dir.path()));
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
    (addr, dir)
}

fn plain_mcp() -> Option<McpOpts> {
    Some(McpOpts {
        catalog: None,
        tools: vec![],
    })
}

#[tokio::test]
async fn initialize_returns_a_session_header_and_tools_capability() {
    let (addr, _tmp) = spawn_with(None, true, plain_mcp()).await;
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
    let (addr, _tmp) = spawn_with(None, true, plain_mcp()).await;
    let resp = reqwest::get(format!("http://{addr}/mcp")).await.unwrap();
    assert_eq!(resp.status(), 405);
}

#[tokio::test]
async fn notification_only_post_is_202() {
    let (addr, _tmp) = spawn_with(None, true, plain_mcp()).await;
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
    let (addr, _tmp) = spawn_with(None, true, plain_mcp()).await;
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
    let (addr, _tmp) = spawn_with(None, true, plain_mcp()).await;
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
    let (addr, _tmp) = spawn_with(Some("t"), true, plain_mcp()).await;
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
    let (addr, _tmp) = spawn_with(
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
    let (addr, _tmp) = spawn_with(None, false, plain_mcp()).await;
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
    let (addr, _tmp) = spawn_with(None, true, None).await;
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
    let (addr, _tmp) = spawn_with(None, true, plain_mcp()).await;
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
    let (addr, _tmp) = spawn_with(None, true, plain_mcp()).await;
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
    let (addr, _tmp) = spawn_with(None, true, plain_mcp()).await;
    assert_eq!(
        init_with_headers(addr, None, Some("null".into())).await,
        403
    );
}

#[tokio::test]
async fn delete_on_mcp_is_405() {
    let (addr, _tmp) = spawn_with(None, true, plain_mcp()).await;
    let resp = reqwest::Client::new()
        .delete(format!("http://{addr}/mcp"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 405);
}

#[tokio::test]
async fn no_web_keeps_the_json_404_for_api_paths() {
    let (addr, _tmp) = spawn_with(None, false, plain_mcp()).await;
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

pub(super) struct NoopLauncher;

#[async_trait]
impl rupu_cp::launcher::RunLauncher for NoopLauncher {
    async fn launch(
        &self,
        _req: rupu_cp::launcher::LaunchRequest,
    ) -> Result<String, rupu_cp::launcher::LaunchError> {
        Ok("run_unused".into())
    }
}

pub(super) const OP_RUN: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0T1OPS";
pub(super) const OP_SESSION: &str = "ses_01J9ZQ3K4M5N6P7Q8R9S0T1OPS";

/// A writable CP with one run and one live session, serving `/mcp`.
async fn spawn_operator() -> (std::net::SocketAddr, tempfile::TempDir) {
    spawn_operator_with(|_| {}, None).await
}

/// [`spawn_operator`] with `seed` run over its global dir first, and
/// `catalog` as the catalog source.
pub(super) async fn spawn_operator_with(
    seed: impl FnOnce(&std::path::Path),
    catalog: Option<Arc<dyn ToolSource>>,
) -> (std::net::SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    seed(&dir);
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
            catalog,
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

pub(super) async fn rpc(addr: std::net::SocketAddr, method: &str, params: Value) -> Value {
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

pub(super) async fn call_tool(addr: std::net::SocketAddr, name: &str, args: Value) -> Value {
    rpc(addr, "tools/call", json!({"name": name, "arguments": args})).await["result"].clone()
}

pub(super) fn text_of(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

#[tokio::test]
async fn tools_list_matches_the_pinned_names() {
    let (addr, _tmp) = spawn_operator().await;
    let v = rpc(addr, "tools/list", json!({})).await;
    let tools = v["result"]["tools"].as_array().unwrap();
    let mut names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    names.sort();
    // The operator tools plus `wait` (its own source).
    let pinned = super::mcp_routes::pinned_tool_names();
    let mut want: Vec<&str> = pinned.iter().map(String::as_str).collect();
    want.push("wait");
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
    assert!(t.contains("is not a run id"), "{t}");
    assert!(!t.contains("\"providers\""), "{t}");
}

#[tokio::test]
async fn a_sibling_route_s_segment_is_not_a_run_id() {
    let (addr, _tmp) = spawn_operator().await;
    for id in ["workflows", "archived", "agents", "autoflows"] {
        for tool in ["runs.get", "runs.cancel"] {
            let r = call_tool(addr, tool, json!({ "id": id })).await;
            assert_eq!(r["isError"], json!(true), "{tool} {id}: {r}");
            assert_eq!(text_of(&r), format!("`{id}` is not a run id"), "{r}");
        }
    }
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
    assert_eq!(get["annotations"]["idempotentHint"], json!(true), "{get}");
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

#[tokio::test]
async fn an_x_rupu_client_header_is_not_mcp_attribution() {
    let (addr, tmp) = spawn_operator().await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/sessions/{OP_SESSION}/messages"))
        .header("x-rupu-client", "mcp")
        .json(&json!({"body": "spoofed"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);
    let lines = log_lines(tmp.path());
    let line = lines.iter().find(|l| l["body"] == "spoofed").unwrap();
    assert!(line.get("client").is_none(), "{line}");
}

/// Parks `id` at its one gate `gate` in `dir`'s run store.
fn seed_parked(dir: &std::path::Path, id: &str) {
    let mut rec = super::sse::seed_run(id, rupu_orchestrator::runs::RunStatus::AwaitingApproval);
    rec.awaiting = vec![rupu_orchestrator::runs::AwaitingGate {
        step_id: "gate".into(),
        prompt: None,
        since: chrono::Utc::now(),
        expires_at: None,
    }];
    rec.sync_awaiting_compat();
    rupu_orchestrator::runs::RunStore::new(dir.join("runs"))
        .create(rec, "name: test\nsteps: []\n")
        .unwrap();
}

/// Who `verb` on `id` was recorded as decided by.
fn decided_by(dir: &std::path::Path, verb: &str, id: &str) -> Option<String> {
    let rec = rupu_orchestrator::runs::RunStore::new(dir.join("runs"))
        .load(id)
        .unwrap();
    match verb {
        "approve" => rec.gate_decisions.first().and_then(|d| d.approver.clone()),
        "resume" => rec.resume_approver,
        _ => rec.reject_cleanup_pending.and_then(|m| m.approver),
    }
}

#[tokio::test]
async fn run_control_through_mcp_records_actor_mcp_and_plain_http_web() {
    const VERBS: [&str; 3] = ["approve", "reject", "cancel"];
    let id = |verb: &str, how: &str| format!("run_{verb}_{how}");
    let (addr, tmp) = spawn_operator_with(
        |dir| {
            for verb in VERBS {
                for how in ["mcp", "http", "spoof"] {
                    seed_parked(dir, &id(verb, how));
                }
            }
            for how in ["http", "spoof"] {
                let rec = super::sse::seed_run(
                    &id("resume", how),
                    rupu_orchestrator::runs::RunStatus::Paused,
                );
                rupu_orchestrator::runs::RunStore::new(dir.join("runs"))
                    .create(rec, "name: test\nsteps: []\n")
                    .unwrap();
            }
        },
        None,
    )
    .await;
    let c = reqwest::Client::new();
    for verb in VERBS {
        let r = call_tool(
            addr,
            &format!("runs.{verb}"),
            json!({"id": id(verb, "mcp")}),
        )
        .await;
        assert_ne!(r["isError"], json!(true), "{verb}: {r}");
    }
    // Plain HTTP, with and without the header the operator source once
    // sent: never `mcp`, `resume` included (its MCP side is
    // `mcp_ops_runs::runs_pause_resume_record_through_mcp`).
    for verb in VERBS.iter().copied().chain(["resume"]) {
        for (how, header) in [("http", false), ("spoof", true)] {
            let mut req = c.post(format!("http://{addr}/api/runs/{}/{verb}", id(verb, how)));
            if header {
                req = req.header("x-rupu-client", "mcp");
            }
            let resp = req.send().await.unwrap();
            assert_eq!(
                resp.status(),
                200,
                "{verb} {how}: {}",
                resp.text().await.unwrap()
            );
        }
    }
    for verb in VERBS {
        assert_eq!(
            decided_by(tmp.path(), verb, &id(verb, "mcp")).as_deref(),
            Some("mcp"),
            "{verb}"
        );
    }
    for verb in VERBS.iter().copied().chain(["resume"]) {
        for how in ["http", "spoof"] {
            assert_eq!(
                decided_by(tmp.path(), verb, &id(verb, how)).as_deref(),
                Some("web"),
                "{verb} {how}"
            );
        }
    }
}

#[tokio::test]
async fn list_tools_answer_arrays_as_text_without_structured_content() {
    let (addr, _tmp) = spawn_operator().await;
    for tool in ["runs.list", "workflows.list", "agents.list"] {
        let r = call_tool(addr, tool, json!({})).await;
        assert_ne!(r["isError"], json!(true), "{tool}: {r}");
        // The protocol allows only an object here.
        assert!(
            r.get("structuredContent").is_none_or(Value::is_object),
            "{tool}: {r}"
        );
        let text: Value = serde_json::from_str(&text_of(&r)).unwrap();
        assert!(text.is_array(), "{tool}: {r}");
        assert!(r.get("structuredContent").is_none(), "{tool}: {r}");
    }
    // With `facets`, `runs.list` answers an object: structured content.
    let r = call_tool(addr, "runs.list", json!({"facets": "1"})).await;
    assert!(r["structuredContent"].is_object(), "{r}");
}

/// A catalog with tools named like operator tools and one no operator tool
/// covers; each answers with its own name.
struct NamedLikeOperator;

#[async_trait]
impl ToolSource for NamedLikeOperator {
    fn tools(&self) -> Vec<ToolDef> {
        ["agents.list", "agents.get", "wait", "findings.query"]
            .into_iter()
            .map(|n| ToolDef {
                name: n.into(),
                description: format!("catalog {n}"),
                input_schema: json!({"type": "object"}),
                hints: ToolHints::default(),
            })
            .collect()
    }
    async fn call(&self, name: &str, _args: Value, _cx: &CallCx) -> Option<ToolResult> {
        Some(ToolResult::Text(format!("catalog {name}")))
    }
}

#[tokio::test]
async fn a_catalog_tool_is_left_out_only_where_an_earlier_source_serves_its_name() {
    let catalog: Arc<dyn ToolSource> = Arc::new(NamedLikeOperator);
    let (addr, _tmp) = spawn_operator_with(|_| {}, Some(Arc::clone(&catalog))).await;
    let v = rpc(addr, "tools/list", json!({})).await;
    let tools = v["result"]["tools"].as_array().unwrap();
    let named = |n: &str| tools.iter().filter(|t| t["name"] == n).collect::<Vec<_>>();
    // Served once each: the operator's `agents.list` and `agents.get`,
    // `wait`'s own `wait`, the catalog's `findings.query`.
    for (name, from_catalog) in [
        ("agents.list", false),
        ("wait", false),
        ("agents.get", false),
        ("findings.query", true),
    ] {
        let t = named(name);
        assert_eq!(t.len(), 1, "{name}: {t:?}");
        assert_eq!(
            t[0]["description"] == json!(format!("catalog {name}")),
            from_catalog,
            "{name}: {t:?}"
        );
    }
    let r = call_tool(addr, "findings.query", json!({})).await;
    assert_eq!(text_of(&r), "catalog findings.query", "{r}");
    let r = call_tool(addr, "agents.list", json!({})).await;
    assert!(text_of(&r).starts_with('['), "{r}");
    // `--tools` checks names the same way.
    rupu_cp::mcp::validate_tools(&["agents.get".into()], Some(&catalog)).unwrap();
    rupu_cp::mcp::validate_tools(&["agents.list".into(), "wait".into()], Some(&catalog)).unwrap();
    assert!(rupu_cp::mcp::validate_tools(&["nope.x".into()], Some(&catalog)).is_err());
}

/// Reports progress twice, then returns.
struct Ticker;

#[async_trait]
impl ToolSource for Ticker {
    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "tick".into(),
            description: "ticks".into(),
            input_schema: json!({"type": "object"}),
            hints: ToolHints::default(),
        }]
    }
    async fn call(&self, name: &str, _args: Value, cx: &CallCx) -> Option<ToolResult> {
        if name != "tick" {
            return None;
        }
        if let Some(p) = &cx.progress {
            p.report(1.0, Some(2.0), Some("one"));
            p.report(2.0, Some(2.0), None);
        }
        Some(ToolResult::Text("ticked".into()))
    }
}

async fn tick_post(accept: &str) -> reqwest::Response {
    let (addr, _tmp) = spawn_with(
        None,
        true,
        Some(McpOpts {
            catalog: Some(Arc::new(Ticker)),
            tools: vec![],
        }),
    )
    .await;
    reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .header("Accept", accept)
        .json(&json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {"name": "tick", "arguments": {},
                       "_meta": {"progressToken": "p9"}}}))
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn a_progress_call_is_answered_as_sse() {
    let resp = tick_post("application/json, text/event-stream").await;
    assert_eq!(resp.status(), 200);
    let ct = resp.headers()["content-type"].to_str().unwrap().to_string();
    assert!(ct.starts_with("text/event-stream"), "{ct}");
    let body = resp.text().await.unwrap();
    let frames: Vec<Value> = body
        .split("\n\n")
        .filter(|f| !f.trim().is_empty())
        .map(|f| {
            assert!(f.contains("event: message"), "{f}");
            let data = f.lines().find_map(|l| l.strip_prefix("data:")).unwrap();
            serde_json::from_str(data.trim()).unwrap()
        })
        .collect();
    assert_eq!(frames.len(), 3, "{body}");
    for (i, f) in frames[..2].iter().enumerate() {
        assert_eq!(f["method"], "notifications/progress");
        assert_eq!(f["params"]["progressToken"], "p9");
        assert_eq!(f["params"]["progress"], (i + 1) as f64);
    }
    assert_eq!(frames[2]["id"], 4);
    assert_eq!(frames[2]["result"]["content"][0]["text"], "ticked");
}

#[tokio::test]
async fn without_sse_in_accept_the_reply_is_json() {
    let resp = tick_post("application/json").await;
    assert_eq!(resp.status(), 200);
    let ct = resp.headers()["content-type"].to_str().unwrap().to_string();
    assert!(ct.starts_with("application/json"), "{ct}");
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["result"]["content"][0]["text"], "ticked");
}

/// Serves `agents.list` (an operator tool also) and `stub.only`.
struct Overlapping;

#[async_trait]
impl ToolSource for Overlapping {
    fn tools(&self) -> Vec<ToolDef> {
        ["agents.list", "stub.only"]
            .into_iter()
            .map(|n| ToolDef {
                name: n.into(),
                description: format!("catalog {n}"),
                input_schema: json!({"type": "object"}),
                hints: ToolHints::default(),
            })
            .collect()
    }
    async fn call(&self, name: &str, _args: Value, _cx: &CallCx) -> Option<ToolResult> {
        Some(ToolResult::Text(format!("catalog {name}")))
    }
}

#[test]
fn served_names_counts_a_shadowed_catalog_tool_once() {
    let catalog: Arc<dyn ToolSource> = Arc::new(Overlapping);
    let with = rupu_cp::mcp::served_names(Some(&catalog));
    let without = rupu_cp::mcp::served_names(None);
    assert!(with.contains("agents.list") && with.contains("stub.only"));
    assert!(without.contains("agents.list") && without.contains("wait"));
    assert!(!without.contains("stub.only"));
    assert_eq!(with.len(), without.len() + 1, "{with:?}");
}

/// An MCP result's JSON: `structuredContent`, else the parsed text.
pub(super) fn json_of(result: &Value) -> Value {
    result
        .get("structuredContent")
        .cloned()
        .unwrap_or_else(|| serde_json::from_str(&text_of(result)).unwrap_or(Value::Null))
}

/// Assert `tools/list` carries each `(name, required args)` of `table`.
pub(super) async fn assert_required_args(addr: std::net::SocketAddr, table: &[(&str, &[&str])]) {
    let list = rpc(addr, "tools/list", json!({})).await["result"]["tools"].clone();
    for (name, required) in table {
        let tool = list
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == *name)
            .unwrap_or_else(|| panic!("{name} is not listed"));
        let mut got: Vec<String> = tool["inputSchema"]["required"]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
            .unwrap_or_default();
        got.sort();
        let mut want: Vec<String> = required.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(got, want, "{name}");
    }
}
