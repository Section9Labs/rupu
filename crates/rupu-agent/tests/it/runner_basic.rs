use rupu_agent::runner::{MockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use rupu_transcript::JsonlReader;

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

    let opts = rupu_agent::grant::with_grant(
        AgentRunOpts {
            system_prompt: "You are a noop agent.".into(),
            prompt: rupu_agent::UserTurn::new("say hi"),
            provider: Box::new(provider),
            limits: rupu_providers::model_limits::ModelLimits::unknown(),
            recovery: Default::default(),
            permission: rupu_tools::PermissionPolicy::bypass(),
            grant: Default::default(),
            alias_scope: Default::default(),
            tool_context: rupu_tools::ToolContext {
                identity: std::sync::Arc::new(rupu_tools::RunIdentity {
                    agent: "noop".into(),
                    provider: "mock".into(),
                    model: "mock-1".into(),
                    run_id: "run_test1".into(),
                    ..Default::default()
                }),
                workspace: rupu_tools::WorkspaceScope {
                    id: "ws_test1".into(),
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
            transcript_path: transcript_path.clone(),
        },
        None,
        &Vec::new(),
    )
    .expect("grant");

    let res = run_agent(opts).await.unwrap();
    assert_eq!(res.turns, 1);
    let summary = JsonlReader::summary(&transcript_path).unwrap();
    assert_eq!(summary.run_id, "run_test1");
    assert_eq!(summary.status, rupu_transcript::RunStatus::Ok);
}

#[tokio::test]
async fn run_start_records_the_tool_contexts_customer() {
    let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "Hello! I have nothing to do.".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }]);
    let tmp = assert_fs::TempDir::new().unwrap();
    let transcript_path = tmp.path().join("run.jsonl");

    let opts = rupu_agent::grant::with_grant(
        AgentRunOpts {
            system_prompt: "You are a noop agent.".into(),
            prompt: rupu_agent::UserTurn::new("say hi"),
            provider: Box::new(provider),
            limits: rupu_providers::model_limits::ModelLimits::unknown(),
            recovery: Default::default(),
            permission: rupu_tools::PermissionPolicy::bypass(),
            grant: Default::default(),
            alias_scope: Default::default(),
            tool_context: {
                let mut tc = ToolContext {
                    services: rupu_tools::ToolServices {
                        customer: Some("acme".into()),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                tc.identity = std::sync::Arc::new(rupu_tools::RunIdentity {
                    agent: "noop".into(),
                    provider: "mock".into(),
                    model: "mock-1".into(),
                    run_id: "run_test1".into(),
                    ..Default::default()
                });
                tc.workspace.id = "ws_test1".into();
                tc.workspace.path = tmp.path().to_path_buf();
                tc
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
            transcript_path: transcript_path.clone(),
        },
        None,
        &Vec::new(),
    )
    .expect("grant");

    let res = run_agent(opts).await.unwrap();
    assert_eq!(res.turns, 1);
    let head = JsonlReader::head(&transcript_path).unwrap();
    assert_eq!(head.customer, Some(Some("acme".to_string())));
}
