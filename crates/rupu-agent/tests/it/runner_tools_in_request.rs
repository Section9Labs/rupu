use rupu_agent::runner::{CapturingMockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_providers::types::StopReason;

#[tokio::test]
async fn run_passes_all_default_tools_to_provider() {
    let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "done".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }]);
    let captured = provider.captured.clone();

    let tmp = assert_fs::TempDir::new().unwrap();
    let opts = rupu_agent::grant::with_grant(
        AgentRunOpts {
            system_prompt: "test".into(),
            prompt: rupu_agent::UserTurn::new("go"),
            provider: Box::new(provider),
            limits: rupu_providers::model_limits::ModelLimits::unknown(),
            recovery: Default::default(),
            permission: rupu_tools::PermissionPolicy::bypass(),
            grant: Default::default(),
            alias_scope: Default::default(),
            tool_context: rupu_tools::ToolContext {
                identity: std::sync::Arc::new(rupu_tools::RunIdentity {
                    agent: "all-tools".into(),
                    provider: "mock".into(),
                    model: "mock-1".into(),
                    run_id: "run_test_tools".into(),
                    ..Default::default()
                }),
                workspace: rupu_tools::WorkspaceScope {
                    id: "ws_test".into(),
                    path: tmp.path().to_path_buf(),
                    ..Default::default()
                },
                ..Default::default()
            },
            pins: Default::default(),
            concerns: None,
            max_turns: 5,
            stream: rupu_agent::StreamOpts {
                no_stream: false,
                suppress_stdout: false,
                on_stream_event: None,
            },
            hooks: Default::default(),
            pause: None,
            collectors: Vec::new(),
            extra_tools: Vec::new(),
            transcript_path: tmp.path().join("run.jsonl"),
        },
        None,
        &Vec::new(),
    )
    .expect("grant");

    run_agent(opts).await.unwrap();

    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1, "expected exactly one request");
    let tools = &requests[0].tools;
    assert_eq!(
        tools.len(),
        7,
        "expected the 7 core tools (no dispatcher, no SCM registry), got {}",
        tools.len()
    );

    let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "ast_grep",
            "bash",
            "edit_file",
            "glob",
            "grep",
            "read_file",
            "write_file",
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
    let opts = rupu_agent::grant::with_grant(
        AgentRunOpts {
            system_prompt: "test".into(),
            prompt: rupu_agent::UserTurn::new("go"),
            provider: Box::new(provider),
            limits: rupu_providers::model_limits::ModelLimits::unknown(),
            recovery: Default::default(),
            permission: rupu_tools::PermissionPolicy::bypass(),
            grant: Default::default(),
            alias_scope: Default::default(),
            tool_context: rupu_tools::ToolContext {
                identity: std::sync::Arc::new(rupu_tools::RunIdentity {
                    agent: "subset".into(),
                    provider: "mock".into(),
                    model: "mock-1".into(),
                    run_id: "run_test_subset".into(),
                    ..Default::default()
                }),
                workspace: rupu_tools::WorkspaceScope {
                    id: "ws_test".into(),
                    path: tmp.path().to_path_buf(),
                    ..Default::default()
                },
                ..Default::default()
            },
            pins: Default::default(),
            concerns: None,
            max_turns: 5,
            stream: rupu_agent::StreamOpts {
                no_stream: false,
                suppress_stdout: false,
                on_stream_event: None,
            },
            hooks: Default::default(),
            pause: None,
            collectors: Vec::new(),
            extra_tools: Vec::new(),
            transcript_path: tmp.path().join("run.jsonl"),
        },
        (Some(vec!["bash".into(), "read_file".into()])).as_deref(),
        &Vec::new(),
    )
    .expect("grant");

    run_agent(opts).await.unwrap();

    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let tools = &requests[0].tools;
    assert_eq!(tools.len(), 2, "expected 2 filtered tools");
    let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["bash", "read_file"]);
}
