use rupu_agent::runner::{BypassDecider, CapturingMockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::sync::Arc;

#[tokio::test]
async fn run_passes_all_six_default_tools_to_provider() {
    let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "done".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }]);
    let captured = provider.captured.clone();

    let tmp = assert_fs::TempDir::new().unwrap();
    let opts = AgentRunOpts {
        agent_name: "all-tools".into(),
        agent_system_prompt: "test".into(),
        agent_tools: None, // None = all 6 default tools
        provider: Box::new(provider),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_test_tools".into(),
        workspace_id: "ws_test".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_path: tmp.path().join("run.jsonl"),
        max_turns: 5,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext::default(),
        user_message: "go".into(),
        mode_str: "bypass".into(),
    };

    run_agent(opts).await.unwrap();

    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1, "expected exactly one request");
    let tools = &requests[0].tools;
    assert_eq!(
        tools.len(),
        6,
        "expected 6 default tools, got {}",
        tools.len()
    );

    let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "bash",
            "edit_file",
            "glob",
            "grep",
            "read_file",
            "write_file"
        ]
    );

    // Spot-check that descriptions and schemas are populated (not the
    // empty-string defaults that would re-introduce the bug).
    for t in tools.iter() {
        assert!(!t.description.is_empty(), "{}: empty description", t.name);
        assert_eq!(
            t.input_schema.get("type").and_then(|v| v.as_str()),
            Some("object"),
            "{}: schema.type missing or wrong",
            t.name
        );
    }
}

#[tokio::test]
async fn run_with_agent_tools_filter_passes_only_listed_tools() {
    let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "done".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }]);
    let captured = provider.captured.clone();

    let tmp = assert_fs::TempDir::new().unwrap();
    let opts = AgentRunOpts {
        agent_name: "subset".into(),
        agent_system_prompt: "test".into(),
        agent_tools: Some(vec!["bash".into(), "read_file".into()]),
        provider: Box::new(provider),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_test_subset".into(),
        workspace_id: "ws_test".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_path: tmp.path().join("run.jsonl"),
        max_turns: 5,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext::default(),
        user_message: "go".into(),
        mode_str: "bypass".into(),
    };

    run_agent(opts).await.unwrap();

    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let tools = &requests[0].tools;
    assert_eq!(tools.len(), 2, "expected 2 filtered tools");
    let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["bash", "read_file"]);
}
