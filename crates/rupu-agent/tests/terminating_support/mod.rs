//! Shared by the `terminating_fallback*` test binaries: a provider that
//! raises the process-wide termination flag as it answers, a hop builder
//! that counts its calls, and the run/assertion helpers. Each binary holds
//! one run, because only the first run in a process gets its provider call
//! in before the flag is up.

use async_trait::async_trait;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts, Hop, HopBuilder, RecoveryOpts, RunError};
use rupu_providers::credential_writes;
use rupu_providers::model_limits::ModelLimits;
use rupu_providers::types::{LlmRequest, LlmResponse, StopReason};
use rupu_providers::{LlmProvider, ProviderError, StreamEvent};
use rupu_tools::ToolContext;
use rupu_transcript::{Event, JsonlReader, RunStatus};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Replays a script and raises the termination flag as it answers — SIGTERM
/// landing while the provider is mid-response.
struct TerminatingProvider {
    inner: MockProvider,
}

#[async_trait]
impl LlmProvider for TerminatingProvider {
    async fn send(&mut self, req: &LlmRequest) -> Result<LlmResponse, ProviderError> {
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

/// Counts build calls; would hand out a working hop.
struct CountingHops {
    builds: Arc<AtomicUsize>,
}

#[async_trait]
impl HopBuilder for CountingHops {
    async fn build(&self, provider: &str, model: &str) -> Result<Hop, String> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        Ok(Hop {
            provider: Box::new(MockProvider::new(vec![ScriptedTurn::AssistantText {
                text: "the hop's answer — never requested".into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }])),
            provider_name: provider.to_string(),
            model: model.to_string(),
            limits: ModelLimits::unknown(),
        })
    }
}

pub async fn run_terminating(
    turn: ScriptedTurn,
    name: &str,
) -> (Result<rupu_agent::RunResult, RunError>, usize, Vec<Event>) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let builds = Arc::new(AtomicUsize::new(0));
    let transcript = tmp.path().join(format!("{name}.jsonl"));
    let res = run_agent(AgentRunOpts {
        seed_source: None,
        collectors: Vec::new(),
        agent_name: "test".into(),
        agent_system_prompt: "test".into(),
        agent_tools: None,
        provider: Box::new(TerminatingProvider {
            inner: MockProvider::new(vec![turn]),
        }),
        provider_name: "anthropic".into(),
        model: "claude-opus-5-5".into(),
        run_id: format!("run_{name}"),
        workspace_id: "ws_terminating".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_path: transcript.clone(),
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
        step_id: "s1".into(),
        on_tool_call: None,
        on_stream_event: None,
        on_usage: None,
        concerns: None,
        limits: ModelLimits::unknown(),
        scope_name: None,
        surface_tag: None,
        pause: None,
        codename: None,
        recovery: RecoveryOpts {
            chain: vec![
                rupu_config::FallbackEntry {
                    provider: None,
                    model: "claude-opus-4-8".into(),
                },
                rupu_config::FallbackEntry {
                    provider: Some("openai-codex".into()),
                    model: "gpt-test".into(),
                },
            ],
            hop_builder: Some(Arc::new(CountingHops {
                builds: builds.clone(),
            })),
        },
    })
    .await;
    let events: Vec<Event> = JsonlReader::iter(&transcript)
        .expect("transcript")
        .collect::<Result<Vec<_>, _>>()
        .expect("well-formed transcript");
    (res, builds.load(Ordering::SeqCst), events)
}

pub fn assert_aborted_without_a_hop(
    res: &Result<rupu_agent::RunResult, RunError>,
    builds: usize,
    events: &[Event],
) {
    assert!(
        matches!(res, Err(RunError::Terminating)),
        "the run stops as terminating: {:?}",
        res.as_ref().err()
    );
    assert_eq!(builds, 0, "no hop was built");
    assert!(
        !events.iter().any(|e| matches!(e, Event::Recovery { .. })),
        "no hop was selected or announced: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::RunComplete { status: RunStatus::Aborted, error: Some(err), .. } if err.contains("terminat")
        )),
        "the transcript ends aborted, saying why: {events:?}"
    );
}
