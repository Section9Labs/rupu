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
        mode_str: "bypass".into(),
        no_stream: false,
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
    // A script that always continues — the loop should hit max_turns.
    let provider = MockProvider::new(vec![
        ScriptedTurn::AssistantText {
            text: "1".into(),
            stop: StopReason::ToolUse,
            input_tokens: 1,
            output_tokens: 1,
        },
        ScriptedTurn::AssistantText {
            text: "2".into(),
            stop: StopReason::ToolUse,
            input_tokens: 1,
            output_tokens: 1,
        },
    ]);
    let tmp = assert_fs::TempDir::new().unwrap();
    let path = tmp.path().join("run.jsonl");
    let res = run_agent(opts(provider, 1, path.clone(), tmp.path().to_path_buf())).await;
    let _ = res; // either Ok or Err; we mainly care about the transcript
    let summary = rupu_transcript::JsonlReader::summary(&path).unwrap();
    assert_eq!(summary.status, rupu_transcript::RunStatus::Error);
}
