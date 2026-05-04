use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use rupu_transcript::JsonlReader;
use std::sync::Arc;

#[tokio::test]
async fn happy_path_one_turn_no_tools() {
    let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "Hello! I have nothing to do.".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }]);
    let tmp = assert_fs::TempDir::new().unwrap();
    let transcript_path = tmp.path().join("run.jsonl");

    let opts = AgentRunOpts {
        agent_name: "noop".into(),
        agent_system_prompt: "You are a noop agent.".into(),
        agent_tools: None,
        provider: Box::new(provider),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_test1".into(),
        workspace_id: "ws_test1".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_path: transcript_path.clone(),
        max_turns: 5,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext::default(),
        user_message: "say hi".into(),
        mode_str: "bypass".into(),
        no_stream: false,
        effort: None,
        context_window: None,
    };

    let res = run_agent(opts).await.unwrap();
    assert_eq!(res.turns, 1);
    let summary = JsonlReader::summary(&transcript_path).unwrap();
    assert_eq!(summary.run_id, "run_test1");
    assert_eq!(summary.status, rupu_transcript::RunStatus::Ok);
}
