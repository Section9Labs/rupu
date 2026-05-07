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
#[tokio::test]
async fn mcp_registry_attaches_tools_to_run() {
    let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "done".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }]);
    let captured = provider.captured.clone();
    let tmp = assert_fs::TempDir::new().unwrap();

    let opts = AgentRunOpts {
        agent_name: "mcp-test".into(),
        agent_system_prompt: "test".into(),
        agent_tools: None,
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
        mode_str: "bypass".into(),
        no_stream: true,
        suppress_stream_stdout: false,
        mcp_registry: Some(Arc::new(Registry::empty())),
        effort: None,
        context_window: None,
        output_format: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
    };

    run_agent(opts).await.unwrap();

    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1, "expected exactly one request");
    let tool_names: Vec<&str> = requests[0].tools.iter().map(|t| t.name.as_str()).collect();

    // The six builtins plus all MCP tools should be present.
    assert!(
        tool_names.contains(&"bash"),
        "builtin bash should still be present: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"scm.repos.list"),
        "MCP tool scm.repos.list should be present: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"issues.list"),
        "MCP tool issues.list should be present: {tool_names:?}"
    );
    // Total must be 6 builtins + 17 MCP tools = 23.
    assert_eq!(
        tool_names.len(),
        23,
        "expected 6 builtins + 17 MCP tools; got {} tools: {tool_names:?}",
        tool_names.len()
    );
}
