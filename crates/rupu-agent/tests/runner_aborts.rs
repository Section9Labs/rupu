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
        concerns: None,
        max_tokens: rupu_agent::runner::DEFAULT_MAX_TOKENS,
        scope_name: None,
        surface_tag: None,
        context_window_tokens: None,
        compact_at_percent: None,
        pause: None,
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

/// A provider whose first call fails with `ProviderError::Preflight` — the
/// shape rupu-orchestrator's agent-load error stub produces for a step whose
/// agent file failed to load.
struct PreflightErrorProvider;

#[async_trait::async_trait]
impl rupu_providers::LlmProvider for PreflightErrorProvider {
    async fn send(
        &mut self,
        _request: &rupu_providers::LlmRequest,
    ) -> Result<rupu_providers::LlmResponse, rupu_providers::ProviderError> {
        Err(rupu_providers::ProviderError::Preflight(
            "agent `ghost` not found or failed to load: agent not found: ghost".into(),
        ))
    }

    async fn stream(
        &mut self,
        request: &rupu_providers::LlmRequest,
        _on_event: &mut (dyn FnMut(rupu_providers::StreamEvent) + Send),
    ) -> Result<rupu_providers::LlmResponse, rupu_providers::ProviderError> {
        self.send(request).await
    }

    fn default_model(&self) -> &str {
        "-"
    }

    fn provider_id(&self) -> rupu_providers::ProviderId {
        rupu_providers::ProviderId::Anthropic
    }
}

#[tokio::test]
async fn preflight_error_surfaces_verbatim_without_provider_prefix() {
    // An agent-load failure is not a provider failure: the run error and the
    // transcript's RunComplete must carry the loader's message verbatim, not
    // `provider: auth config error: unresolved: …` with an auth-login hint.
    let mut o = opts(
        MockProvider::new(vec![]),
        5,
        std::path::PathBuf::new(),
        std::path::PathBuf::new(),
    );
    let tmp = assert_fs::TempDir::new().unwrap();
    let path = tmp.path().join("run.jsonl");
    o.provider = Box::new(PreflightErrorProvider);
    o.transcript_path = path.clone();
    o.workspace_path = tmp.path().to_path_buf();
    let res = run_agent(o).await;
    let msg = match res {
        Err(RunError::Preflight(m)) => m,
        Err(e) => panic!("expected RunError::Preflight, got {e:?}"),
        Ok(_) => panic!("expected the run to fail"),
    };
    assert!(msg.contains("agent `ghost` not found"), "got: {msg}");

    let raw = std::fs::read_to_string(&path).unwrap();
    let complete_line = raw
        .lines()
        .find(|l| l.contains("RunComplete") || l.contains("run_complete"))
        .expect("transcript must contain a RunComplete event");
    assert!(
        complete_line.contains("agent `ghost` not found"),
        "RunComplete missing loader message: {complete_line}",
    );
    assert!(
        !complete_line.contains("provider:") && !complete_line.contains("auth"),
        "RunComplete must not attribute an agent-load failure to a provider: {complete_line}",
    );
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
