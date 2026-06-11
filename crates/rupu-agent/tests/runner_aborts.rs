use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts, RunError};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::sync::Arc;

fn opts(
    provider: MockProvider,
    max_turns: u32,
    transcript: std::path::PathBuf,
    ws: std::path::PathBuf,
) -> AgentRunOpts {
    AgentRunOpts {
        agent_name: "test".into(),
        agent_system_prompt: "test".into(),
        agent_tools: None,
        provider: Box::new(provider),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_xx".into(),
        workspace_id: "ws_xx".into(),
        workspace_path: ws,
        transcript_path: transcript,
        max_turns,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext::default(),
        user_message: "go".into(),
        initial_messages: Vec::new(),
        turn_index_offset: 0,
        mode_str: "bypass".into(),
        no_stream: false,
        suppress_stream_stdout: false,
        mcp_registry: None,
        effort: None,
        context_window: None,
        output_format: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
        parent_run_id: None,
        depth: 0,
        dispatchable_agents: None,
        step_id: String::new(),
        on_tool_call: None,
        on_stream_event: None,
        concerns: None,
        scope_name: None,
        surface_tag: None,
    }
}

#[tokio::test]
async fn provider_error_propagates_and_writes_run_complete() {
    let provider = MockProvider::new(vec![ScriptedTurn::ProviderError("boom".into())]);
    let tmp = assert_fs::TempDir::new().unwrap();
    let path = tmp.path().join("run.jsonl");
    let res = run_agent(opts(provider, 5, path.clone(), tmp.path().to_path_buf())).await;
    assert!(matches!(res, Err(RunError::Provider(_))));
    let summary = rupu_transcript::JsonlReader::summary(&path).unwrap();
    assert_eq!(summary.status, rupu_transcript::RunStatus::Error);
}

#[tokio::test]
async fn max_turns_aborts_with_run_complete() {
    // A script that genuinely keeps requesting tool calls — each tool call
    // yields a tool_result the runner sends back as a user message, so the loop
    // legitimately continues and must hit max_turns and abort with Error.
    // (A text-only turn now correctly terminates the loop, so max_turns must be
    // exercised with real tool calls, not a spurious non-EndTurn stop reason.)
    let provider = MockProvider::new(vec![
        ScriptedTurn::AssistantToolUse {
            text: None,
            tool_id: "c1".into(),
            tool_name: "read_file".into(),
            tool_input: serde_json::json!({ "path": "." }),
            stop: StopReason::ToolUse,
        },
        ScriptedTurn::AssistantToolUse {
            text: None,
            tool_id: "c2".into(),
            tool_name: "read_file".into(),
            tool_input: serde_json::json!({ "path": "." }),
            stop: StopReason::ToolUse,
        },
    ]);
    let tmp = assert_fs::TempDir::new().unwrap();
    let path = tmp.path().join("run.jsonl");
    let res = run_agent(opts(provider, 1, path.clone(), tmp.path().to_path_buf())).await;
    let _ = res; // either Ok or Err; we mainly care about the transcript
    let summary = rupu_transcript::JsonlReader::summary(&path).unwrap();
    assert_eq!(summary.status, rupu_transcript::RunStatus::Error);
}
