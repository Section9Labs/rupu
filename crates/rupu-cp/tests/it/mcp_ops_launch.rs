//! Plan 3a package B's operator tools: `runs.retry`, `agent_runs.continue`,
//! `workflows.run_yaml`, `agents.validate`, `agents.validate_all`, and the
//! launch tools' new arguments.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use super::launch_api::{get, spawn_launch, AGENT_DONE, AGENT_LIVE, FAILED, LIVE};
use super::mcp::{assert_required_args, call_tool, json_of, rpc, text_of};
use serde_json::{json, Value};

fn is_error(r: &Value) -> bool {
    r["isError"] == json!(true)
}

fn tool(list: &Value, name: &str) -> Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("{name} is not listed"))
        .clone()
}

#[tokio::test]
async fn launch_tools_are_listed_with_their_required_arguments() {
    let (addr, _tmp, _) = spawn_launch().await;
    assert_required_args(
        addr,
        &[
            ("runs.retry", &["id"]),
            ("agent_runs.continue", &["id"]),
            ("workflows.run_yaml", &["yaml"]),
            ("agents.validate", &["raw"]),
            ("agents.validate_all", &[]),
        ],
    )
    .await;
    let list = rpc(addr, "tools/list", json!({})).await["result"]["tools"].clone();
    for name in ["runs.retry", "agent_runs.continue", "workflows.run_yaml"] {
        assert_eq!(
            tool(&list, name)["annotations"]["openWorldHint"],
            json!(true),
            "{name}"
        );
    }
    assert_eq!(
        tool(&list, "agents.validate")["annotations"]["readOnlyHint"],
        json!(true)
    );
    assert_eq!(
        tool(&list, "agents.validate_all")["annotations"]["readOnlyHint"],
        json!(true)
    );
    // The launch tools gained their new arguments; a continue's schema
    // offers an optional prompt and the launch flags it can replace.
    let props = |name: &str| tool(&list, name)["inputSchema"]["properties"].clone();
    for key in ["model", "provider", "engagement_profiles"] {
        assert!(props("agents.run").get(key).is_some(), "agents.run: {key}");
    }
    assert!(props("workflows.run").get("engagement_profiles").is_some());
    assert!(props("agent_runs.continue").get("model").is_some());
    for key in [
        "prompt",
        "findings_profile",
        "engagement_profiles",
        "skills",
        "preload_skills",
        "messaging",
    ] {
        assert!(
            props("agent_runs.continue").get(key).is_some(),
            "agent_runs.continue: {key}"
        );
    }
    assert_eq!(
        tool(&list, "agent_runs.continue")["inputSchema"]["required"],
        json!(["id"]),
        "the prompt is optional"
    );
    assert!(props("runs.retry").get("restart_interrupted").is_some());
}

#[tokio::test]
async fn validate_all_answers_what_the_api_answers() {
    let (addr, _tmp, _) = spawn_launch().await;
    let r = call_tool(addr, "agents.validate_all", json!({})).await;
    assert!(!is_error(&r), "{r}");
    let (status, want) = get(addr, "/api/agents/validate").await;
    assert_eq!(status, 200);
    assert_eq!(json_of(&r), want);
    let r = call_tool(
        addr,
        "agents.validate",
        json!({"raw": "---\nname: fresh\n---\nHi.\n"}),
    )
    .await;
    assert_eq!(json_of(&r), json!({"ok": true, "name": "fresh"}));
    let bad = call_tool(
        addr,
        "agents.validate",
        json!({"raw": "---\nname: t\ntools: [reed_file]\n---\nx\n"}),
    )
    .await;
    assert!(text_of(&bad).starts_with("HTTP 400"), "{bad}");
}

#[tokio::test]
async fn retry_and_continue_record_through_mcp() {
    let (addr, tmp, rec) = spawn_launch().await;
    let r = call_tool(
        addr,
        "runs.retry",
        json!({"id": FAILED, "restart_interrupted": true}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(json_of(&r)["retrying"], json!(true));
    assert_eq!(rec.retries.lock().unwrap()[0].run_id, FAILED);
    assert!(rec.retries.lock().unwrap()[0].restart_interrupted);
    // Recorded before the spawn, attributed to the MCP.
    let recorded = rupu_orchestrator::RunStore::new(tmp.path().join("runs"))
        .load(FAILED)
        .unwrap();
    assert_eq!(recorded.resume_approver.as_deref(), Some("mcp"));
    let live = call_tool(addr, "runs.retry", json!({"id": LIVE})).await;
    assert!(text_of(&live).starts_with("HTTP 409"), "{live}");

    let r = call_tool(
        addr,
        "agent_runs.continue",
        json!({"id": AGENT_DONE, "model": "claude-opus-5-5"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(json_of(&r)["run_id"], json!("run_01CONTINUED"));
    let agents = rec.agents.lock().unwrap().clone();
    assert_eq!(agents[0].continue_from.as_deref(), Some(AGENT_DONE));
    assert_eq!(agents[0].model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(agents[0].prompt, None);
    // With a prompt, the continuation is told it next.
    let r = call_tool(
        addr,
        "agent_runs.continue",
        json!({"id": AGENT_DONE, "prompt": "now summarise"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let agents = rec.agents.lock().unwrap().clone();
    assert_eq!(agents[1].continue_from.as_deref(), Some(AGENT_DONE));
    assert_eq!(agents[1].prompt.as_deref(), Some("now summarise"));
    let blank = call_tool(
        addr,
        "agent_runs.continue",
        json!({"id": AGENT_DONE, "prompt": " "}),
    )
    .await;
    assert!(text_of(&blank).starts_with("HTTP 400"), "{blank}");
    let live = call_tool(addr, "agent_runs.continue", json!({"id": AGENT_LIVE})).await;
    assert!(text_of(&live).starts_with("HTTP 409"), "{live}");
    let remote = call_tool(
        addr,
        "agent_runs.continue",
        json!({"id": AGENT_DONE, "host": "peer"}),
    )
    .await;
    assert!(text_of(&remote).starts_with("HTTP 501"), "{remote}");
}

#[tokio::test]
async fn a_yaml_launch_and_the_new_launch_arguments_record_through_mcp() {
    let (addr, _tmp, rec) = spawn_launch().await;
    let r = call_tool(
        addr,
        "workflows.run_yaml",
        json!({"yaml": super::launch_api::ADHOC, "engagement_profiles": ["web"]}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let file = json_of(&r)["file"].as_str().unwrap().to_string();
    let runs = rec.runs.lock().unwrap().clone();
    assert_eq!(runs[0].file.as_deref(), Some(std::path::Path::new(&file)));
    assert_eq!(runs[0].engagement_profiles, ["web"]);
    let bad = call_tool(addr, "workflows.run_yaml", json!({"yaml": "steps: ["})).await;
    assert!(text_of(&bad).starts_with("HTTP 400"), "{bad}");

    let r = call_tool(
        addr,
        "agents.run",
        json!({"name": "scout", "prompt": "go", "model": "m", "provider": "p"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let agents = rec.agents.lock().unwrap().clone();
    assert_eq!(
        (agents[0].model.as_deref(), agents[0].provider.as_deref()),
        (Some("m"), Some("p"))
    );
}

/// The new literal routes beside `:name` routes: a name equal to one of them
/// is refused before any handler runs, never answered by the literal route.
#[tokio::test]
async fn a_name_equal_to_a_new_literal_route_is_refused() {
    let (addr, tmp, rec) = spawn_launch().await;
    let agents_dir = tmp.path().join("agents");
    std::fs::write(
        agents_dir.join("validate.md"),
        "---\nname: validate\n---\nx\n",
    )
    .unwrap();
    for (name, args) in [
        ("agents.get", json!({"name": "validate"})),
        ("agents.delete", json!({"name": "validate"})),
        ("workflows.get", json!({"name": "run"})),
        ("workflows.delete", json!({"name": "run"})),
    ] {
        let r = call_tool(addr, name, args.clone()).await;
        assert!(is_error(&r), "{name} {args}: {r}");
        assert!(
            text_of(&r).contains("routes to") || text_of(&r).starts_with("HTTP 405"),
            "{name} {args}: {}",
            text_of(&r)
        );
    }
    assert!(agents_dir.join("validate.md").exists());
    assert!(rec.runs.lock().unwrap().is_empty());
}
