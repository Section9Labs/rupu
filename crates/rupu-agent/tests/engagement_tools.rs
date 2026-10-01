//! Engagement profiles reach the agent: the `asset_mark` tool, the `asset`
//! field on `report_finding`, and the engagement guidance — all only when an
//! engagement is active, so a code-only run's tool surface is unchanged.

use rupu_agent::runner::{BypassDecider, CapturingMockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_coverage::{
    read_asset_graph, target_id, CatalogMode, ConcernsBlock, ConcernsEntry, CoveragePaths,
    FindingWriteOptions, IncludeDirective,
};
use rupu_providers::types::{LlmRequest, StopReason};
use rupu_tools::ToolContext;
use std::sync::{Arc, Mutex};

fn engagement(ids: &[&str]) -> Option<Arc<rupu_coverage::profile::ActiveSet>> {
    let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
    let set = rupu_coverage::profile::builtin_registry()
        .unwrap()
        .active_set(&ids)
        .unwrap();
    Some(Arc::new(set))
}

fn stride() -> ConcernsBlock {
    ConcernsBlock {
        entries: vec![ConcernsEntry::Include(IncludeDirective {
            include: "stride".to_string(),
            overrides: vec![],
            mode: CatalogMode::Auto,
            filter: None,
        })],
    }
}

fn done() -> ScriptedTurn {
    ScriptedTurn::AssistantText {
        text: "Done.".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }
}

fn asset_mark_call() -> ScriptedTurn {
    ScriptedTurn::AssistantToolUse {
        text: None,
        tool_id: "t1".into(),
        tool_name: "asset_mark".into(),
        tool_input: serde_json::json!({
            "kind": "binary:function",
            "locator": [{ "sha256": "ab".repeat(32) }, { "address": 4198400 }, { "symbol": "main" }],
            "depth": "analyzed"
        }),
        stop: StopReason::ToolUse,
    }
}

/// Run one agent and return its captured LLM requests.
async fn run(
    ws: &std::path::Path,
    tools: Option<Vec<&str>>,
    concerns: Option<ConcernsBlock>,
    findings: FindingWriteOptions,
    turns: Vec<ScriptedTurn>,
) -> Vec<LlmRequest> {
    let provider = CapturingMockProvider::new(turns);
    let captured: Arc<Mutex<Vec<LlmRequest>>> = provider.captured.clone();
    let opts = AgentRunOpts {
        on_usage: None,
        seed_source: None,
        agent_name: "re-agent".into(),
        agent_system_prompt: "You reverse engineer.".into(),
        agent_tools: tools.map(|t| t.into_iter().map(String::from).collect()),
        provider: Box::new(provider),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_engagement".into(),
        workspace_id: "ws_engagement".into(),
        workspace_path: ws.to_path_buf(),
        transcript_path: ws.join("run.jsonl"),
        max_turns: 6,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext {
            workspace_path: ws.to_path_buf(),
            findings: Some(findings),
            ..Default::default()
        },
        user_message: "Go.".into(),
        initial_messages: Vec::new(),
        turn_index_offset: 0,
        mode_str: "bypass".into(),
        no_stream: true,
        suppress_stream_stdout: false,
        mcp_registry: None,
        effort: None,
        context_window: None,
        output_format: None,
        output_schema: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
        parent_run_id: None,
        depth: 0,
        dispatchable_agents: None,
        step_id: String::new(),
        on_tool_call: None,
        on_stream_event: None,
        concerns,
        scope_name: None,
        max_tokens: rupu_agent::runner::DEFAULT_MAX_TOKENS,
        surface_tag: Some("agent".into()),
        context_window_tokens: None,
        compact_at_percent: None,
        pause: None,
        codename: None,
    };
    run_agent(opts).await.expect("run succeeds");
    let out = captured.lock().unwrap().clone();
    out
}

fn tool<'a>(req: &'a LlmRequest, name: &str) -> Option<&'a rupu_providers::types::ToolDefinition> {
    req.tools.iter().find(|t| t.name == name)
}

fn summary() -> FindingWriteOptions {
    FindingWriteOptions::default().with_profile(rupu_coverage::FindingProfile::Summary)
}

#[tokio::test]
async fn granted_asset_mark_records_an_asset_without_a_concerns_block() {
    let ws = tempfile::TempDir::new().unwrap();
    let reqs = run(
        ws.path(),
        Some(vec!["report_finding", "asset_mark"]),
        None,
        summary().with_engagement(engagement(&["binary"])),
        vec![asset_mark_call(), done()],
    )
    .await;

    // The tool is advertised, and the finding tool offers `asset`.
    assert!(tool(&reqs[0], "asset_mark").is_some());
    let rf = tool(&reqs[0], "report_finding").unwrap();
    assert!(
        rf.input_schema["properties"]["asset"].is_object(),
        "{}",
        rf.input_schema
    );
    // The engagement guidance is part of the prompt.
    let system = reqs[0].system.as_deref().unwrap_or_default();
    assert!(system.contains("## Engagement profiles"), "{system}");
    assert!(system.contains("`binary:function`"), "{system}");

    // The call landed in the asset store, at the depth the agent named.
    let paths = CoveragePaths::new(ws.path(), &target_id(ws.path(), "re-agent"));
    let graph = read_asset_graph(&paths);
    let assets: Vec<_> = graph.iter().collect();
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].kind, "binary:function");
    assert_eq!(assets[0].depth.as_deref(), Some("analyzed"));
}

#[tokio::test]
async fn granted_asset_mark_is_registered_on_the_concerns_path_too() {
    let ws = tempfile::TempDir::new().unwrap();
    let reqs = run(
        ws.path(),
        Some(vec!["read_file", "asset_mark"]),
        Some(stride()),
        summary().with_engagement(engagement(&["binary"])),
        vec![asset_mark_call(), done()],
    )
    .await;
    assert!(tool(&reqs[0], "asset_mark").is_some());
    assert!(tool(&reqs[0], "coverage_mark").is_some());

    let paths = CoveragePaths::new(ws.path(), &target_id(ws.path(), "re-agent"));
    assert_eq!(read_asset_graph(&paths).iter().count(), 1);
}

#[tokio::test]
async fn asset_mark_is_not_registered_unless_granted() {
    // Concerns path with no `tools:` list, and a findings-only grant: neither
    // names `asset_mark`, so neither gets it, engagement or not.
    let ws = tempfile::TempDir::new().unwrap();
    let reqs = run(
        ws.path(),
        None,
        Some(stride()),
        summary().with_engagement(engagement(&["binary"])),
        vec![done()],
    )
    .await;
    assert!(tool(&reqs[0], "asset_mark").is_none());

    let ws = tempfile::TempDir::new().unwrap();
    let reqs = run(
        ws.path(),
        Some(vec!["report_finding"]),
        None,
        summary().with_engagement(engagement(&["binary"])),
        vec![done()],
    )
    .await;
    assert!(tool(&reqs[0], "asset_mark").is_none());
}

#[tokio::test]
async fn without_an_engagement_the_tool_surface_and_prompt_are_unchanged() {
    let ws = tempfile::TempDir::new().unwrap();
    let plain = run(
        ws.path(),
        Some(vec!["report_finding"]),
        None,
        summary(),
        vec![done()],
    )
    .await;
    let rf = tool(&plain[0], "report_finding").unwrap();
    assert!(
        rf.input_schema["properties"].get("asset").is_none(),
        "no engagement => the agent is not offered `asset`"
    );
    assert!(tool(&plain[0], "asset_mark").is_none());
    let system = plain[0].system.as_deref().unwrap_or_default();
    assert!(!system.contains("Engagement profiles"), "{system}");

    // With an engagement the schema differs ONLY by the `asset` property.
    let ws = tempfile::TempDir::new().unwrap();
    let engaged = run(
        ws.path(),
        Some(vec!["report_finding"]),
        None,
        summary().with_engagement(engagement(&["binary"])),
        vec![done()],
    )
    .await;
    let mut with = tool(&engaged[0], "report_finding")
        .unwrap()
        .input_schema
        .clone();
    with["properties"].as_object_mut().unwrap().remove("asset");
    assert_eq!(with, rf.input_schema);
}
