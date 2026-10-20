//! The fleet-area operator tools: hosts and workers, config writes,
//! customers, the launch preview and repos.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use super::mcp::{assert_required_args, call_tool, json_of, spawn_operator_with, text_of};
use serde_json::{json, Value};
use std::net::SocketAddr;

async fn http_get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let r = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

fn is_error(r: &Value) -> bool {
    r["isError"] == json!(true)
}

#[tokio::test]
async fn fleet_tools_are_listed_with_their_required_arguments() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let table: &[(&str, &[&str])] = &[
        ("hosts.list", &[]),
        ("hosts.registered", &[]),
        ("hosts.add", &["name", "base_url"]),
        ("hosts.add_ssh", &["name", "host"]),
        ("hosts.add_bucket", &["name", "url"]),
        ("hosts.enroll_node", &["name"]),
        ("hosts.remove", &["id"]),
        ("workers.list", &[]),
        ("host.info", &[]),
        ("config.get", &[]),
        ("config.put_global", &[]),
        ("config.put_customer", &["slug"]),
        ("config.put_project", &["id"]),
        ("config.put_policy", &["lock"]),
        ("customers.list", &[]),
        ("customers.create", &["slug", "name"]),
        ("customers.get", &["slug"]),
        ("customers.update", &["slug"]),
        ("customers.delete", &["slug"]),
        ("customers.archive", &["slug"]),
        ("customers.unarchive", &["slug"]),
        ("customers.assign_project", &["slug", "ws_id"]),
        ("customers.unassign_project", &["slug", "ws_id"]),
        ("launch.preview", &[]),
        ("repos.list", &[]),
    ];
    assert_required_args(addr, table).await;
}

#[tokio::test]
async fn fleet_reads_answer_what_the_api_answers() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    // A customer to list.
    let r = call_tool(
        addr,
        "customers.create",
        json!({"slug": "acme", "name": "Acme"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let cases: &[(&str, Value, &str)] = &[
        ("hosts.registered", json!({}), "/api/hosts/registered"),
        ("host.info", json!({}), "/api/host/info"),
        ("workers.list", json!({}), "/api/workers"),
        ("config.get", json!({}), "/api/config"),
        (
            "customers.list",
            json!({"range": "all"}),
            "/api/customers?range=all",
        ),
        (
            "customers.get",
            json!({"slug": "acme", "range": "all"}),
            "/api/customers/acme?range=all",
        ),
    ];
    for (tool, args, path) in cases {
        let r = call_tool(addr, tool, args.clone()).await;
        let (status, want) = http_get(addr, path).await;
        assert_eq!(status, 200, "{tool}: {path}");
        assert!(!is_error(&r), "{tool}: {r}");
        let got = json_of(&r);
        if *tool == "customers.list" {
            assert_eq!(got.as_array().unwrap().len(), 1);
        }
        assert_eq!(got, want, "{tool}");
    }
    // The local host is listed live.
    let r = call_tool(addr, "hosts.list", json!({})).await;
    assert!(!is_error(&r), "{r}");
    let hosts = json_of(&r);
    assert_eq!(hosts[0]["id"], "local");
    assert_eq!(hosts[0]["status"], "online");
}

/// A CP bound to a non-loopback address (so `/mcp` takes any `Host`), with
/// the MCP endpoint and a launcher.
async fn spawn_routable() -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let state = rupu_cp::state::AppState::new(
        tmp.path().to_path_buf(),
        rupu_config::PricingConfig::default(),
    )
    .with_launcher(Some(std::sync::Arc::new(super::mcp::NoopLauncher)))
    .with_bind("0.0.0.0:0".to_string());
    let server = rupu_cp::mcp::build_server(
        &state,
        &rupu_cp::mcp::McpOpts {
            catalog: None,
            tools: vec![],
        },
    )
    .unwrap();
    let app = rupu_cp::server::router_with(
        state,
        None,
        rupu_cp::server::RouterOpts {
            web: false,
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

#[tokio::test]
async fn enroll_node_builds_its_command_from_the_mcp_host() {
    let (addr, _tmp) = spawn_routable().await;
    let resp: Value = reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .header("Host", "10.1.2.3:7879")
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "hosts.enroll_node", "arguments": {"name": "n1"}},
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let r = &resp["result"];
    assert!(!is_error(r), "{resp}");
    let body = json_of(r);
    let command = body["command"].as_str().unwrap();
    assert!(
        command.contains("ws://10.1.2.3:7879/api/node/connect"),
        "{command}"
    );
    assert_eq!(body["host"]["name"], "n1");
    assert_eq!(body["host"]["transport_kind"], "tunnel");
    assert!(body["token"].as_str().is_some_and(|t| !t.is_empty()));
    // The host is registered.
    let r = call_tool(addr, "hosts.registered", json!({})).await;
    assert!(json_of(&r)
        .as_array()
        .unwrap()
        .iter()
        .any(|h| h["name"] == "n1"));
}

#[tokio::test]
async fn hosts_are_added_and_removed() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let r = call_tool(
        addr,
        "hosts.add_ssh",
        json!({"name": "box", "host": "box.example", "port": 2222}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let added = json_of(&r);
    assert_eq!(added["transport_kind"], "ssh");
    assert_eq!(added["status"], "offline");
    let id = added["id"].as_str().unwrap().to_string();

    let r = call_tool(
        addr,
        "hosts.add_bucket",
        json!({"name": "drop", "url": "s3://bucket/x", "prefix": "p"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(json_of(&r)["transport_kind"], "bucket");

    let r = call_tool(addr, "hosts.remove", json!({"id": id})).await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(r["structuredContent"], json!({"status": 204}));
    let r = call_tool(addr, "hosts.remove", json!({"id": id})).await;
    assert!(is_error(&r) && text_of(&r).starts_with("HTTP 404"), "{r}");
    let r = call_tool(addr, "hosts.remove", json!({"id": "local"})).await;
    assert!(is_error(&r) && text_of(&r).starts_with("HTTP 400"), "{r}");
}

#[tokio::test]
async fn customers_delete_answers_204_as_status() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let r = call_tool(
        addr,
        "customers.create",
        json!({"slug": "gone", "name": "Gone"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(json_of(&r)["slug"], "gone");
    let r = call_tool(addr, "customers.delete", json!({"slug": "gone"})).await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(r["structuredContent"], json!({"status": 204}));
    let r = call_tool(addr, "customers.get", json!({"slug": "gone"})).await;
    assert!(is_error(&r) && text_of(&r).starts_with("HTTP 404"), "{r}");
}

#[tokio::test]
async fn customers_are_updated_archived_and_assigned() {
    let (addr, _tmp) = spawn_operator_with(
        |dir| {
            std::fs::create_dir_all(dir.join("workspaces")).unwrap();
            std::fs::write(
                dir.join("workspaces").join("ws_1.toml"),
                format!(
                    "id = \"ws_1\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
                    dir.display()
                ),
            )
            .unwrap();
        },
        None,
    )
    .await;
    call_tool(
        addr,
        "customers.create",
        json!({"slug": "acme", "name": "Acme"}),
    )
    .await;
    let r = call_tool(
        addr,
        "customers.update",
        json!({"slug": "acme", "name": "Acme Corp", "notes": "n"}),
    )
    .await;
    assert_eq!(json_of(&r)["name"], "Acme Corp");
    let r = call_tool(addr, "customers.archive", json!({"slug": "acme"})).await;
    assert_eq!(json_of(&r)["archived"], true);
    let r = call_tool(addr, "customers.unarchive", json!({"slug": "acme"})).await;
    assert_eq!(json_of(&r)["archived"], false);

    let r = call_tool(
        addr,
        "customers.assign_project",
        json!({"slug": "acme", "ws_id": "ws_1"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(json_of(&r)["customer"]["slug"], "acme");
    // Deleting now is a 409 naming the project.
    let r = call_tool(addr, "customers.delete", json!({"slug": "acme"})).await;
    assert!(is_error(&r) && text_of(&r).starts_with("HTTP 409"), "{r}");
    assert!(text_of(&r).contains("ws_1"), "{r}");
    let r = call_tool(
        addr,
        "customers.unassign_project",
        json!({"slug": "acme", "ws_id": "ws_1"}),
    )
    .await;
    assert_eq!(r["structuredContent"], json!({"status": 204}));
    let r = call_tool(
        addr,
        "customers.unassign_project",
        json!({"slug": "acme", "ws_id": "ws_1"}),
    )
    .await;
    assert!(is_error(&r) && text_of(&r).starts_with("HTTP 404"), "{r}");
}

#[tokio::test]
async fn config_writes_are_destructive_and_idempotent() {
    let (addr, tmp) = spawn_operator_with(|_| {}, None).await;
    let list = super::mcp::rpc(addr, "tools/list", json!({})).await["result"]["tools"].clone();
    for name in [
        "config.put_global",
        "config.put_customer",
        "config.put_project",
        "config.put_policy",
    ] {
        let t = list
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap();
        assert_eq!(t["annotations"]["destructiveHint"], json!(true), "{name}");
        assert_eq!(t["annotations"]["idempotentHint"], json!(true), "{name}");
    }
    // Writing the same patch twice leaves the same file.
    let patch = json!({"patch": {"policy.lock": ["bash.timeout_secs"]}});
    let r = call_tool(addr, "config.put_global", patch.clone()).await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(json_of(&r)["ok"], true);
    let first = std::fs::read_to_string(tmp.path().join("config.toml")).unwrap();
    let r = call_tool(addr, "config.put_global", patch).await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("config.toml")).unwrap(),
        first
    );
    // Invalid text is refused and nothing changes.
    let r = call_tool(addr, "config.put_global", json!({"raw": "not = [toml"})).await;
    assert!(is_error(&r) && text_of(&r).starts_with("HTTP 400"), "{r}");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("config.toml")).unwrap(),
        first
    );
    // The policy tool replaces the lock list.
    let r = call_tool(addr, "config.put_policy", json!({"lock": []})).await;
    assert_eq!(json_of(&r), json!({"ok": true}));
}

#[tokio::test]
async fn launch_preview_names_one_target_and_echoes_the_host() {
    let (addr, _tmp) = spawn_operator_with(
        |dir| {
            std::fs::create_dir_all(dir.join("agents")).unwrap();
            std::fs::write(
                dir.join("agents/foo.md"),
                "---\nname: foo\ndescription: \"a test agent\"\n---\nYou are foo.\n",
            )
            .unwrap();
        },
        None,
    )
    .await;
    let r = call_tool(addr, "launch.preview", json!({})).await;
    assert!(is_error(&r) && text_of(&r).starts_with("HTTP 400"), "{r}");
    let r = call_tool(
        addr,
        "launch.preview",
        json!({"agent": "foo", "host": "other"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let body = json_of(&r);
    assert_eq!(body["host"], "other");
    assert!(body["warnings"].as_array().is_some_and(|w| !w.is_empty()));
}

#[tokio::test]
async fn repos_without_a_lister_is_a_501() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let r = call_tool(addr, "repos.list", json!({})).await;
    assert!(is_error(&r) && text_of(&r).starts_with("HTTP 501"), "{r}");
}
