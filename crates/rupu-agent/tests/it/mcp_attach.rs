//! End-to-end: confirm AgentRunOpts.mcp_registry = Some(...) causes
//! the MCP-backed tools to appear in the runner's tool registry.

use rupu_agent::run_agent;
use rupu_agent::runner::{AgentRunOpts, BypassDecider, CapturingMockProvider, ScriptedTurn};
use rupu_providers::types::StopReason;
use rupu_scm::Registry;
use rupu_tools::ToolContext;
use std::sync::Arc;

/// Structural contract: AgentRunOpts accepts mcp_registry: Some(Registry::empty())
/// at the type level. This verifies the field wiring compiles and is accepted
/// by run_agent without panicking. We use a CapturingMockProvider to confirm
/// the MCP tool names actually appear in the outbound LlmRequest.tools list.
/// Run one scripted turn with an MCP registry attached and return the tool
/// names the provider was offered.
async fn offered_tools(agent_tools: Option<Vec<String>>) -> Vec<String> {
    let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "done".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }]);
    let captured = provider.captured.clone();
    let tmp = assert_fs::TempDir::new().unwrap();

    let opts = AgentRunOpts {
        seed_source: None,
        collectors: Vec::new(),
        extra_tools: Vec::new(),
        agent_name: "mcp-test".into(),
        agent_system_prompt: "test".into(),
        agent_tools,
        provider: Box::new(provider),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_mcp_attach".into(),
        workspace_id: "ws_mcp".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_path: tmp.path().join("run.jsonl"),
        max_turns: 5,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext::default(),
        user_message: "list repos".into(),
        initial_messages: Vec::new(),
        turn_index_offset: 0,
        mode_str: "bypass".into(),
        no_stream: true,
        suppress_stream_stdout: false,
        mcp_registry: Some(Arc::new(Registry::empty())),
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
        on_usage: None,
        concerns: None,
        limits: rupu_providers::model_limits::ModelLimits::unknown(),
        scope_name: None,
        surface_tag: None,
        pause: None,
        codename: None,
        recovery: Default::default(),
    };

    run_agent(opts).await.unwrap();

    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1, "expected exactly one request");
    requests[0].tools.iter().map(|t| t.name.clone()).collect()
}

#[tokio::test]
async fn mcp_registry_attaches_tools_to_run() {
    let names = offered_tools(None).await;
    let tool_names: Vec<&str> = names.iter().map(String::as_str).collect();

    // All builtins plus all MCP tools should be present.
    assert!(
        tool_names.contains(&"bash"),
        "builtin bash should still be present: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"dispatch_agent"),
        "builtin dispatch_agent should be present: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"dispatch_agents_parallel"),
        "builtin dispatch_agents_parallel should be present: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"scm.repos.list"),
        "MCP tool scm.repos.list should be present: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"issues.list"),
        "MCP tool issues.list should be present: {tool_names:?}"
    );
    // The findings MCP trio needs a run context the agent's in-process
    // dispatcher never has, so it is never offered — even under `tools: ["*"]`
    // (here: no allowlist at all). Agents use the findings builtins.
    for name in ["findings.record", "findings.query", "findings.tag"] {
        assert!(
            !tool_names.contains(&name),
            "{name} must not be offered to an agent: {tool_names:?}"
        );
    }
    // Total must be 9 builtins (6 v0 + ast_grep + dispatch_agent + dispatch_agents_parallel)
    // + the 18 native SCM MCP tools = 27.
    assert_eq!(
        tool_names.len(),
        27,
        "expected 9 builtins + 18 MCP tools; got {} tools: {tool_names:?}",
        tool_names.len()
    );
}

/// A `tools: ["*"]` agent (every stock-fleet agent) gets the SCM MCP tools but
/// never the `findings.*` ones: the agent's in-process dispatcher has no run
/// context for them, so offering them only burns turns on refusals.
#[tokio::test]
async fn wildcard_agent_is_not_offered_findings_mcp_tools() {
    let names = offered_tools(Some(vec!["*".to_string()])).await;
    assert!(names.iter().any(|n| n == "scm.repos.list"), "{names:?}");
    assert!(
        !names.iter().any(|n| n.starts_with("findings.")),
        "findings.* MCP tools leaked to a wildcard agent: {names:?}"
    );
    // An explicit `findings.*` grant doesn't bring them back either.
    let names = offered_tools(Some(vec!["findings.*".to_string()])).await;
    assert!(
        !names.iter().any(|n| n.starts_with("findings.")),
        "{names:?}"
    );
}
