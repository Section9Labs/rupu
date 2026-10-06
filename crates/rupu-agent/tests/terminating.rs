//! Once SIGTERM has arrived (`credential_writes::terminating()`), the agent
//! loop starts no new work: no LLM call and no tool dispatch. The handler
//! holds the exit only for credential writes still draining, and nothing
//! started now would finish.
//!
//! Its own test binary: the flag is process-wide and never cleared, so the
//! two phases run in order inside one test — the tool-dispatch check first
//! (the flag is raised mid-turn, by the provider), then the LLM-call check
//! (the flag is already up).

use async_trait::async_trait;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts, RunError};
use rupu_providers::credential_writes;
use rupu_providers::types::{LlmRequest, LlmResponse, StopReason};
use rupu_providers::{LlmProvider, ProviderError, StreamEvent};
use rupu_tools::ToolContext;
use rupu_transcript::{Event, JsonlReader, RunStatus};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Replays a script and raises the termination flag as it answers — SIGTERM
/// landing while the model is mid-response.
struct TerminatingProvider {
    inner: MockProvider,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmProvider for TerminatingProvider {
    async fn send(&mut self, req: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let response = self.inner.send(req).await;
        credential_writes::request_termination();
        response
    }

    async fn stream(
        &mut self,
        req: &LlmRequest,
        _on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        self.send(req).await
    }

    fn default_model(&self) -> &str {
        "mock-1"
    }

    fn provider_id(&self) -> rupu_providers::ProviderId {
        rupu_providers::ProviderId::Anthropic
    }
}

fn opts(
    provider: Box<dyn LlmProvider>,
    transcript: std::path::PathBuf,
    ws: std::path::PathBuf,
) -> AgentRunOpts {
    AgentRunOpts {
        seed_source: None,
        collectors: Vec::new(),
        extra_tools: Vec::new(),
        agent_name: "test".into(),
        agent_system_prompt: "test".into(),
        agent_tools: None,
        provider,
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_terminating".into(),
        workspace_id: "ws_terminating".into(),
        workspace_path: ws,
        transcript_path: transcript,
        max_turns: 5,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext::default(),
        user_message: "go".into(),
        initial_messages: Vec::new(),
        turn_index_offset: 0,
        mode_str: "bypass".into(),
        no_stream: false,
        suppress_stream_stdout: true,
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
        on_usage: None,
        concerns: None,
        limits: rupu_providers::model_limits::ModelLimits::unknown(),
        scope_name: None,
        surface_tag: None,
        pause: None,
        codename: None,
        recovery: Default::default(),
    }
}

fn events(path: &std::path::Path) -> Vec<Event> {
    JsonlReader::iter(path)
        .expect("transcript")
        .collect::<Result<Vec<_>, _>>()
        .expect("well-formed transcript")
}

#[tokio::test]
async fn a_terminating_process_starts_no_tool_dispatch_and_no_llm_call() {
    let tmp = assert_fs::TempDir::new().unwrap();
    assert!(!credential_writes::terminating(), "fresh process");

    // Phase 1: SIGTERM lands while the model answers with a tool call. The
    // call is answered (it was already in flight); the tool is never run.
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = TerminatingProvider {
        inner: MockProvider::new(vec![
            ScriptedTurn::AssistantToolUse {
                text: None,
                tool_id: "call_1".into(),
                tool_name: "read_file".into(),
                tool_input: serde_json::json!({ "path": tmp.path().join("nope").to_str().unwrap() }),
                stop: StopReason::ToolUse,
            },
            ScriptedTurn::AssistantText {
                text: "never reached".into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            },
        ]),
        calls: calls.clone(),
    };
    let transcript = tmp.path().join("tool.jsonl");
    let res = run_agent(opts(
        Box::new(provider),
        transcript.clone(),
        tmp.path().to_path_buf(),
    ))
    .await;
    match res {
        Err(RunError::Terminating) => {}
        Err(other) => panic!("the run stops as terminating, got {other:?}"),
        Ok(_) => panic!("the run stops as terminating, but it completed"),
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the in-flight call answered; no second call"
    );
    let evs = events(&transcript);
    assert!(
        evs.iter().any(|e| matches!(e, Event::ToolCall { .. })),
        "the model's tool call was recorded"
    );
    assert!(
        !evs.iter().any(|e| matches!(e, Event::ToolResult { .. })),
        "but the tool never ran: {evs:?}"
    );
    assert!(
        evs.iter().any(|e| matches!(
            e,
            Event::RunComplete { status: RunStatus::Aborted, error: Some(err), .. } if err.contains("terminat")
        )),
        "the transcript ends aborted, saying why: {evs:?}"
    );

    // Phase 2: the flag is up before the run starts: no LLM call at all.
    assert!(credential_writes::terminating());
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = TerminatingProvider {
        inner: MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: "never reached".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]),
        calls: calls.clone(),
    };
    let transcript = tmp.path().join("llm.jsonl");
    let res = run_agent(opts(
        Box::new(provider),
        transcript.clone(),
        tmp.path().to_path_buf(),
    ))
    .await;
    match res {
        Err(RunError::Terminating) => {}
        Err(other) => panic!("the run stops as terminating, got {other:?}"),
        Ok(_) => panic!("the run stops as terminating, but it completed"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no LLM call was started");
    let evs = events(&transcript);
    assert!(
        !evs.iter()
            .any(|e| matches!(e, Event::AssistantMessage { .. })),
        "nothing was answered: {evs:?}"
    );
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::RunComplete {
            status: RunStatus::Aborted,
            ..
        }
    )));
}
