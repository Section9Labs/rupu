//! The definitions-area operator tools: agents, workflows, autoflows, claims,
//! skills, tools and models.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use super::mcp::{assert_required_args, call_tool, json_of, rpc, spawn_operator_with, text_of};
use async_trait::async_trait;
use rupu_mcp::{CallCx, ToolDef, ToolHints, ToolResult, ToolSource};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;

const AGENT_MD: &str = "---\nname: seeded\ndescription: a seeded agent\nmodel: opus\n---\nDo it.\n";
const WORKFLOW_YAML: &str =
    "name: seeded-wf\nsteps:\n  - id: one\n    agent: seeded\n    actions: []\n    prompt: hi\n";

fn seed(dir: &std::path::Path) {
    let agents = dir.join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("seeded.md"), AGENT_MD).unwrap();
    let wfs = dir.join("workflows");
    std::fs::create_dir_all(&wfs).unwrap();
    std::fs::write(wfs.join("seeded-wf.yaml"), WORKFLOW_YAML).unwrap();
}

async fn http_get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let r = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

fn is_error(r: &Value) -> bool {
    r["isError"] == json!(true)
}

#[tokio::test]
async fn definitions_tools_are_listed_with_their_required_arguments() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let list = rpc(addr, "tools/list", json!({})).await["result"]["tools"].clone();
    let table: &[(&str, &[&str])] = &[
        ("agents.get", &["name"]),
        ("agents.create", &["raw"]),
        ("agents.save", &["name", "raw"]),
        ("agents.delete", &["name"]),
        ("agents.generate", &["description"]),
        ("workflows.get", &["name"]),
        ("workflows.create", &["raw"]),
        ("workflows.save", &["name", "raw"]),
        ("workflows.delete", &["name"]),
        ("workflows.validate", &["raw"]),
        ("workflows.generate", &["description"]),
        ("generate.models", &[]),
        ("autoflows.list", &[]),
        ("autoflows.enable", &["name"]),
        ("autoflows.disable", &["name"]),
        ("claims.list", &[]),
        ("claims.release", &["issue_ref"]),
        ("claims.requeue", &["issue_ref"]),
        ("skills.list", &[]),
        ("skills.get", &["layer", "name"]),
        ("tools.list", &[]),
        ("models.list", &[]),
        ("models.refresh", &[]),
    ];
    assert_required_args(addr, table).await;
    let hints = |name: &str| -> Value {
        list.as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap()["annotations"]
            .clone()
    };
    assert_eq!(hints("agents.delete")["destructiveHint"], json!(true));
    assert_eq!(hints("agents.generate")["openWorldHint"], json!(true));
    // A read-only POST is also idempotent.
    assert_eq!(hints("workflows.validate")["readOnlyHint"], json!(true));
    assert_eq!(hints("workflows.validate")["idempotentHint"], json!(true));
    assert_eq!(hints("agents.save")["idempotentHint"], json!(true));
    // The scope kind is an enum in the schema.
    let save = list
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "agents.save")
        .unwrap();
    let kind = save["inputSchema"]["properties"]["scope_kind"].to_string();
    assert!(
        kind.contains("global") && kind.contains("project"),
        "{kind}"
    );
}

#[tokio::test]
async fn definitions_reads_answer_what_the_api_answers() {
    let (addr, _tmp) = spawn_operator_with(seed, None).await;
    let cases: &[(&str, Value, &str)] = &[
        (
            "agents.get",
            json!({"name": "seeded"}),
            "/api/agents/seeded",
        ),
        (
            "workflows.get",
            json!({"name": "seeded-wf"}),
            "/api/workflows/seeded-wf",
        ),
        ("autoflows.list", json!({}), "/api/autoflows"),
        ("claims.list", json!({}), "/api/autoflows/claims"),
        ("generate.models", json!({}), "/api/generate/models"),
        ("skills.list", json!({}), "/api/skills"),
        ("tools.list", json!({}), "/api/tools"),
    ];
    for (tool, args, path) in cases {
        let r = call_tool(addr, tool, args.clone()).await;
        let (status, want) = http_get(addr, path).await;
        assert_eq!(status, 200, "{tool}: {path}");
        assert!(!is_error(&r), "{tool}: {r}");
        assert_eq!(json_of(&r), want, "{tool}");
    }
    // No model catalog is installed: the tool answers the API's 501.
    let r = call_tool(addr, "models.list", json!({})).await;
    assert!(is_error(&r), "{r}");
    assert!(text_of(&r).starts_with("HTTP 501"), "{}", text_of(&r));
    assert_eq!(http_get(addr, "/api/models").await.0, 501);
}

#[tokio::test]
async fn agent_create_save_get_delete_round_trip() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let raw =
        |desc: &str| format!("---\nname: t-agent\ndescription: {desc}\nmodel: opus\n---\nHello.\n");
    let r = call_tool(addr, "agents.create", json!({"raw": raw("first")})).await;
    assert!(!is_error(&r), "{r}");
    let got = json_of(&call_tool(addr, "agents.get", json!({"name": "t-agent"})).await);
    assert_eq!(got["description"], json!("first"), "{got}");
    // A second create of the same name is the API's 409.
    let dup = call_tool(addr, "agents.create", json!({"raw": raw("again")})).await;
    assert!(text_of(&dup).starts_with("HTTP 409"), "{dup}");

    let r = call_tool(
        addr,
        "agents.save",
        json!({"name": "t-agent", "raw": raw("edited"), "scope_kind": "global"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let got = json_of(&call_tool(addr, "agents.get", json!({"name": "t-agent"})).await);
    assert_eq!(got["description"], json!("edited"), "{got}");

    let r = call_tool(addr, "agents.delete", json!({"name": "t-agent"})).await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(json_of(&r)["deleted"], json!(true), "{r}");
    let gone = call_tool(addr, "agents.get", json!({"name": "t-agent"})).await;
    assert!(is_error(&gone), "{gone}");
    assert!(text_of(&gone).starts_with("HTTP 404"), "{}", text_of(&gone));
}

#[tokio::test]
async fn workflow_create_save_delete_round_trip() {
    let (addr, _tmp) = spawn_operator_with(seed, None).await;
    let r = call_tool(
        addr,
        "workflows.save",
        json!({"name": "seeded-wf", "raw": WORKFLOW_YAML.replace("hi", "edited")}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let got = json_of(&call_tool(addr, "workflows.get", json!({"name": "seeded-wf"})).await);
    assert!(got["yaml"].as_str().unwrap().contains("edited"), "{got}");
    let r = call_tool(addr, "workflows.delete", json!({"name": "seeded-wf"})).await;
    assert_eq!(json_of(&r)["deleted"], json!(true), "{r}");
    let gone = call_tool(addr, "workflows.get", json!({"name": "seeded-wf"})).await;
    assert!(text_of(&gone).starts_with("HTTP 404"), "{gone}");
}

#[tokio::test]
async fn workflows_validate_reports_errors_as_json() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    for raw in [WORKFLOW_YAML, "name: [unclosed\n  - nope"] {
        let r = call_tool(addr, "workflows.validate", json!({"raw": raw})).await;
        let resp = reqwest::Client::new()
            .post(format!("http://{addr}/api/workflows/validate"))
            .json(&json!({"raw": raw}))
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap();
        if status / 100 == 2 {
            assert!(!is_error(&r), "{r}");
            assert_eq!(json_of(&r), serde_json::from_str::<Value>(&body).unwrap());
        } else {
            assert!(is_error(&r), "{r}");
            assert!(
                text_of(&r).starts_with(&format!("HTTP {status}")),
                "{}",
                text_of(&r)
            );
        }
    }
    let ok = call_tool(addr, "workflows.validate", json!({"raw": WORKFLOW_YAML})).await;
    assert_eq!(json_of(&ok), json!({"ok": true}));
    let bad = call_tool(addr, "workflows.validate", json!({"raw": "name: [x"})).await;
    assert!(is_error(&bad), "{bad}");
}

#[tokio::test]
async fn models_refresh_accepts_an_empty_argument_set() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    // The tool sends `{}`, which the handler's body parser accepts: with no
    // catalog installed the answer is the API's 501, not a body error (400).
    let r = call_tool(addr, "models.refresh", json!({})).await;
    assert!(text_of(&r).starts_with("HTTP 501"), "{}", text_of(&r));
    let r = call_tool(addr, "models.refresh", json!({"provider": "anthropic"})).await;
    assert!(text_of(&r).starts_with("HTTP 501"), "{}", text_of(&r));
}

#[tokio::test]
async fn generate_without_a_generator_is_the_apis_501() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    for tool in ["agents.generate", "workflows.generate"] {
        let r = call_tool(addr, tool, json!({"description": "x"})).await;
        assert!(
            text_of(&r).starts_with("HTTP 501"),
            "{tool}: {}",
            text_of(&r)
        );
    }
}

/// A catalog that also serves `agents.get`.
struct CatalogAgentsGet;

#[async_trait]
impl ToolSource for CatalogAgentsGet {
    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "agents.get".into(),
            description: "catalog agents.get".into(),
            input_schema: json!({"type": "object"}),
            hints: ToolHints::default(),
        }]
    }
    async fn call(&self, name: &str, _args: Value, _cx: &CallCx) -> Option<ToolResult> {
        Some(ToolResult::Text(format!("catalog {name}")))
    }
}

#[tokio::test]
async fn agents_get_is_the_operator_tool_not_the_catalogs() {
    let catalog: Arc<dyn ToolSource> = Arc::new(CatalogAgentsGet);
    let (addr, _tmp) = spawn_operator_with(seed, Some(catalog)).await;
    let r = call_tool(addr, "agents.get", json!({"name": "seeded"})).await;
    assert!(!is_error(&r), "{r}");
    let got = json_of(&r);
    for key in ["name", "system_prompt", "raw"] {
        assert!(got.get(key).is_some(), "{key} missing: {got}");
    }
    assert_eq!(got["name"], json!("seeded"));
}
