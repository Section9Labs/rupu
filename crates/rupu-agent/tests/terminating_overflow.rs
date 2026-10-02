//! Once SIGTERM has arrived (`credential_writes::terminating()`), the agent
//! loop starts no new LLM call — the compaction summariser included. Here
//! the flag is raised while the provider answers with a context-overflow
//! error, so the loop's next step would be to learn the limit and compact
//! with a summariser call before retrying. It must not start either call.
//!
//! Its own test binary: the flag is process-wide and never cleared.

use async_trait::async_trait;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts, RunError};
use rupu_providers::credential_writes;
use rupu_providers::model_limits::ModelLimits;
use rupu_providers::types::{ContentBlock, LlmRequest, LlmResponse, Message, Role, StopReason};
use rupu_providers::{LlmProvider, ProviderError, StreamEvent};
use rupu_tools::ToolContext;
use rupu_transcript::{Event, JsonlReader, RunStatus};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Replays a script and raises the termination flag as it answers — SIGTERM
/// landing while the provider is mid-response.
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

/// Enough prior exchanges that compaction has a middle to summarise.
fn history() -> Vec<Message> {
    (0..4)
        .flat_map(|i| {
            [
                Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: format!("question {i} {}", "x".repeat(200)),
                    }],
                },
                Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text {
                        text: format!("answer {i} {}", "y".repeat(200)),
                    }],
                },
            ]
        })
        .collect()
}

#[tokio::test]
async fn a_terminating_process_starts_no_overflow_compaction_call() {
    let tmp = assert_fs::TempDir::new().unwrap();
    assert!(!credential_writes::terminating(), "fresh process");

    let calls = Arc::new(AtomicUsize::new(0));
    let provider = TerminatingProvider {
        inner: MockProvider::new(vec![
            // An overflow with a parsed max: the loop learns the limit and
            // would compact (a summariser call) before retrying.
            ScriptedTurn::ProviderError(
                "API error 400: prompt is too long: 1200 tokens > 1000 maximum".into(),
            ),
            ScriptedTurn::AssistantText {
                text: "the summary — never requested".into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            },
            ScriptedTurn::AssistantText {
                text: "the retry — never sent".into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            },
        ]),
        calls: calls.clone(),
    };
    let transcript = tmp.path().join("overflow.jsonl");
    let res = run_agent(AgentRunOpts {
        seed_source: None,
        collectors: Vec::new(),
        agent_name: "test".into(),
        agent_system_prompt: "test".into(),
        agent_tools: None,
        provider: Box::new(provider),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_terminating_overflow".into(),
        workspace_id: "ws_terminating".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_path: transcript.clone(),
        max_turns: 5,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext::default(),
        user_message: "go".into(),
        initial_messages: history(),
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
        limits: ModelLimits::fixed(2_000, 100),
        scope_name: None,
        surface_tag: None,
        pause: None,
        codename: None,
    })
    .await;
    match res {
        Err(RunError::Terminating) => {}
        Err(other) => panic!("the run stops as terminating, got {other:?}"),
        Ok(_) => panic!("the run stops as terminating, but it completed"),
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the overflowing call answered; no summariser call, no retry"
    );
    let evs: Vec<Event> = JsonlReader::iter(&transcript)
        .expect("transcript")
        .collect::<Result<Vec<_>, _>>()
        .expect("well-formed transcript");
    assert!(
        !evs.iter().any(|e| matches!(e, Event::Compaction { .. })),
        "nothing was compacted: {evs:?}"
    );
    assert!(
        evs.iter().any(|e| matches!(
            e,
            Event::RunComplete { status: RunStatus::Aborted, error: Some(err), .. } if err.contains("terminat")
        )),
        "the transcript ends aborted, saying why: {evs:?}"
    );
}
