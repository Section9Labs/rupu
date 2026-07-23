//! E2e — pause/resume round-trips across run / workflow / fan-out (T9).
//!
//! Mirrors the harness shape of `tests/distributed_fanout_e2e.rs` and
//! `tests/linear_runner.rs`: a real disk-backed `RunStore`, `run_workflow`
//! driven directly through its public `OrchestratorRunOpts`, and fake
//! `StepFactory` / `UnitDispatcher` implementations. "Resume" is built the
//! way `rupu workflow resume` (and the CP resume worker) really do it: read
//! the persisted `RunRecord` / `step_results.jsonl` / `unit_checkpoints.jsonl`
//! / paused-seed sidecar back off disk and construct a fresh `ResumeState`,
//! then re-enter `run_workflow` — proving the round-trip survives a process
//! boundary, not just an in-memory struct hand-off.
//!
//! Pause timing is controlled deterministically — no wall-clock races:
//!   - Test 1 (mid-run) blocks the in-flight agent turn on a provider whose
//!     `send` never returns; a background task cancels the pause token
//!     after a short, generous delay. The non-pause branch of `run_agent`'s
//!     `select!` can never win (it never resolves), so this is
//!     deterministic regardless of scheduling jitter — the same mechanism
//!     `rupu_orchestrator::runner`'s own `agent_run_pauses_and_resumes` unit
//!     test uses (see `BlockingProvider` there).
//!   - Test 2 (step boundary) and Test 3 (mid-fan-out) cancel the token as
//!     the LAST action inside the in-flight unit's own async body (a
//!     provider wrapper / a `UnitDispatcher::dispatch_unit`), right before
//!     it returns its result. `run_agent`'s pause check (`select!` against
//!     `wait_pause`) and the orchestrator's step/unit-boundary check
//!     (`pause_triggered`) can only ever observe the cancellation on a poll
//!     that happens strictly AFTER this same async body has already
//!     returned — so the in-flight unit always completes intact, and only
//!     the NEXT boundary check sees the pause. This is the same technique
//!     `runner.rs`'s own `CancelAfterFirstDispatcher` uses for its
//!     step-boundary / mid-fan-out pause tests.

use async_trait::async_trait;
use rupu_agent::runner::{
    BypassDecider, CapturingMockProvider, MockProvider, ScriptedTurn, DEFAULT_MAX_TOKENS,
};
use rupu_agent::{AgentRunOpts, RunError};
use rupu_orchestrator::executor::{Event, EventSink};
use rupu_orchestrator::runner::{
    run_workflow, ItemResult, OrchestratorRunOpts, PauseReason, PausedStep, ResumeState,
    StepFactory, UnitDispatch, UnitDispatcher, UnitOutcome,
};
use rupu_orchestrator::{RunStatus, RunStore, StepResult, Workflow};
use rupu_providers::types::{LlmRequest, LlmResponse, StopReason, StreamEvent};
use rupu_providers::{LlmProvider, ProviderError, ProviderId};
use rupu_tools::ToolContext;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Collects the label of every pause/resume/terminal-run event emitted, in
/// order. Real assertion material — not a vacuous "some event fired" check.
#[derive(Default)]
struct EventRecorder {
    labels: Mutex<Vec<String>>,
}
impl EventRecorder {
    fn labels(&self) -> Vec<String> {
        self.labels.lock().unwrap().clone()
    }
}
impl EventSink for EventRecorder {
    fn emit(&self, _run_id: &str, ev: &Event) {
        let label = match ev {
            Event::RunPaused { .. } => "RunPaused",
            Event::RunResumed { .. } => "RunResumed",
            Event::StepPaused { .. } => "StepPaused",
            Event::StepResumed { .. } => "StepResumed",
            Event::RunCompleted { .. } => "RunCompleted",
            Event::RunFailed { .. } => "RunFailed",
            _ => return,
        };
        self.labels.lock().unwrap().push(label.to_string());
    }
}

/// A provider whose `send` blocks effectively forever, so a pause token
/// racing it in `run_agent`'s `select!` always wins deterministically.
struct BlockingProvider;
#[async_trait]
impl LlmProvider for BlockingProvider {
    async fn send(&mut self, _req: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        tokio::time::sleep(Duration::from_secs(3600)).await;
        Err(ProviderError::Http("unreachable — pause should win".into()))
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
    fn provider_id(&self) -> ProviderId {
        ProviderId::Anthropic
    }
}

/// Wraps another provider and cancels `token` immediately after the inner
/// `send`/`stream` call returns — see the module docs for why this makes
/// the pause land deterministically at the NEXT boundary check rather than
/// racing the in-flight call.
struct CancelAfterInner<P> {
    inner: P,
    token: CancellationToken,
}
#[async_trait]
impl<P: LlmProvider> LlmProvider for CancelAfterInner<P> {
    async fn send(&mut self, req: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        let r = self.inner.send(req).await;
        self.token.cancel();
        r
    }
    async fn stream(
        &mut self,
        req: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        let r = self.inner.stream(req, on_event).await;
        self.token.cancel();
        r
    }
    fn default_model(&self) -> &str {
        self.inner.default_model()
    }
    fn provider_id(&self) -> ProviderId {
        self.inner.provider_id()
    }
}

/// Build a minimal `AgentRunOpts` around `provider`. `no_stream: true` races
/// `provider.send` directly against the pause token — the deterministic
/// boundary these tests exploit (mirrors `rupu_orchestrator::runner`'s own
/// pause tests).
#[allow(clippy::too_many_arguments)]
fn linear_agent_opts(
    provider: Box<dyn LlmProvider>,
    agent_name: &str,
    rendered_prompt: String,
    run_id: String,
    workspace_id: String,
    workspace_path: PathBuf,
    transcript_path: PathBuf,
    on_tool_call: Option<rupu_agent::OnToolCallCallback>,
) -> AgentRunOpts {
    AgentRunOpts {
        agent_name: agent_name.to_string(),
        agent_system_prompt: "test".into(),
        agent_tools: None,
        provider,
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id,
        workspace_id,
        workspace_path,
        transcript_path,
        max_turns: 5,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext::default(),
        user_message: rendered_prompt,
        initial_messages: Vec::new(),
        turn_index_offset: 0,
        mode_str: "bypass".into(),
        no_stream: true,
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
        on_tool_call,
        on_stream_event: None,
        concerns: None,
        max_tokens: DEFAULT_MAX_TOKENS,
        context_window_tokens: None,
        compact_at_percent: None,
        scope_name: None,
        surface_tag: None,
        pause: None,
    }
}

/// Panics if `build_opts_for_step` is ever called — used where every unit
/// is routed through a `UnitDispatcher` (fully-distributed fan-out), so
/// local dispatch must never happen.
struct PanicFactory;
#[async_trait]
impl StepFactory for PanicFactory {
    async fn build_opts_for_step(
        &self,
        _step_id: &str,
        _agent_name: &str,
        _rendered_prompt: String,
        _run_id: String,
        _workspace_id: String,
        _workspace_path: PathBuf,
        _transcript_path: PathBuf,
        _on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        panic!(
            "PanicFactory: build_opts_for_step must not be called for a fully-distributed fan-out"
        )
    }
}

// ---------------------------------------------------------------------------
// Test 1 — a single agent run pauses mid-turn, then resumes to completion.
// ---------------------------------------------------------------------------

const WF_SOLO: &str = r#"
name: pause-solo
steps:
  - id: solo
    agent: worker
    prompt: "do work"
"#;

/// Hands out one pre-built provider (mirrors `OneShotFactory` in
/// `runner.rs`'s own pause tests) and records the transcript path it was
/// asked to write to, so the test can inspect that file directly after the
/// pause lands.
struct OneShotFactory {
    provider: Mutex<Option<Box<dyn LlmProvider>>>,
    transcript_path_out: Arc<Mutex<Option<PathBuf>>>,
}
impl OneShotFactory {
    fn new(
        provider: Box<dyn LlmProvider>,
        transcript_path_out: Arc<Mutex<Option<PathBuf>>>,
    ) -> Self {
        Self {
            provider: Mutex::new(Some(provider)),
            transcript_path_out,
        }
    }
}
#[async_trait]
impl StepFactory for OneShotFactory {
    async fn build_opts_for_step(
        &self,
        _step_id: &str,
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: PathBuf,
        transcript_path: PathBuf,
        on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        *self.transcript_path_out.lock().unwrap() = Some(transcript_path.clone());
        let provider = self
            .provider
            .lock()
            .unwrap()
            .take()
            .expect("OneShotFactory: provider already taken");
        linear_agent_opts(
            provider,
            agent_name,
            rendered_prompt,
            run_id,
            workspace_id,
            workspace_path,
            transcript_path,
            on_tool_call,
        )
    }
}

#[tokio::test]
async fn run_pause_then_resume_completes() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_SOLO).unwrap();

    // --- Phase 1: pause mid-run. The provider never returns, so the
    // background cancel is the only branch that can ever win. ---
    let token = CancellationToken::new();
    let token2 = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        token2.cancel();
    });

    let transcript_path_out: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let factory1 = Arc::new(OneShotFactory::new(
        Box::new(BlockingProvider),
        transcript_path_out.clone(),
    ));
    let recorder1 = Arc::new(EventRecorder::default());

    let opts1 = OrchestratorRunOpts {
        workflow: wf.clone(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_pause_run".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: factory1,
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_SOLO.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(recorder1.clone()),
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: Some(token),
    };

    let res1 = run_workflow(opts1)
        .await
        .expect("a pause is not an Err — phase 1 must return Ok");
    let awaiting = res1
        .awaiting
        .clone()
        .expect("run must report a paused wait state");
    assert_eq!(awaiting.reason, PauseReason::Manual);
    assert_eq!(awaiting.step_id, "solo");
    assert!(
        res1.step_results.is_empty(),
        "the in-flight step must not be recorded as complete"
    );
    assert!(
        !awaiting.resume_seed.is_empty(),
        "a mid-step pause must carry a resume seed"
    );

    // Genuine events — RunPaused + StepPaused fired, RunCompleted did not.
    let labels1 = recorder1.labels();
    assert!(
        labels1.contains(&"StepPaused".to_string()),
        "got {labels1:?}"
    );
    assert!(
        labels1.contains(&"RunPaused".to_string()),
        "got {labels1:?}"
    );
    assert!(
        !labels1.contains(&"RunCompleted".to_string()),
        "a paused run must not also report completion; got {labels1:?}"
    );

    // Durable state (not just the in-memory `awaiting` struct): the
    // RunRecord is genuinely `Paused` and non-terminal.
    assert!(!res1.run_id.is_empty());
    let record1 = store.load(&res1.run_id).expect("run record persisted");
    assert_eq!(record1.status, RunStatus::Paused);
    assert!(
        record1.finished_at.is_none(),
        "a paused run is non-terminal"
    );

    // No partial/half-done state persisted: no step_result checkpoint for
    // the step that never finished...
    let persisted_steps = store
        .read_step_results(&res1.run_id)
        .expect("read step_results.jsonl");
    assert!(
        persisted_steps.is_empty(),
        "no step result may be checkpointed for a step that paused mid-run"
    );

    // ...and the mid-step seed persisted to disk matches the in-memory one
    // (the resume path reads it back from disk in a fresh process).
    let disk_seed = store
        .read_paused_seed(&res1.run_id)
        .expect("read persisted paused-step seed");
    assert_eq!(disk_seed.len(), awaiting.resume_seed.len());
    assert!(!disk_seed.is_empty());

    // ...and the step's own transcript never committed a partial assistant
    // message, nor a tool_call left without a matching tool_result. The
    // provider never returned anything (it blocks forever) so nothing
    // beyond the run/turn-start bookkeeping should be on disk at all.
    let transcript_path = transcript_path_out
        .lock()
        .unwrap()
        .clone()
        .expect("factory must have captured the step's transcript path");
    let events: Vec<rupu_transcript::Event> = rupu_transcript::JsonlReader::iter(&transcript_path)
        .expect("transcript file must exist")
        .flatten()
        .collect();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, rupu_transcript::Event::AssistantMessage { .. })),
        "a paused mid-turn run must not persist a committed assistant message; got {events:?}"
    );
    let mut open_tool_calls: std::collections::HashSet<String> = Default::default();
    for ev in &events {
        match ev {
            rupu_transcript::Event::ToolCall { call_id, .. } => {
                open_tool_calls.insert(call_id.clone());
            }
            rupu_transcript::Event::ToolResult { call_id, .. } => {
                open_tool_calls.remove(call_id);
            }
            _ => {}
        }
    }
    assert!(
        open_tool_calls.is_empty(),
        "no tool_call may be left dangling without a matching tool_result; got {open_tool_calls:?}"
    );

    // --- Phase 2: resume from disk → completes, issuing a fresh request. ---
    let mut record2 = store.load(&res1.run_id).unwrap();
    record2.status = RunStatus::Running;
    record2.finished_at = None;
    store.update(&record2).expect("flip run back to Running");

    let seed = store
        .read_paused_seed(&res1.run_id)
        .expect("read paused seed for resume");
    store.clear_paused_seed(&res1.run_id).unwrap();
    let prior_step_results: Vec<StepResult> = store
        .read_step_results(&res1.run_id)
        .unwrap()
        .iter()
        .map(StepResult::from)
        .collect();

    let provider2 = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "done".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }]);
    let captured_requests = provider2.captured.clone();
    let factory2 = Arc::new(OneShotFactory::new(
        Box::new(provider2),
        Arc::new(Mutex::new(None)),
    ));
    let recorder2 = Arc::new(EventRecorder::default());

    let opts2 = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: record2.workspace_id.clone(),
        workspace_path: record2.workspace_path.clone(),
        transcript_dir: record2.transcript_dir.clone(),
        factory: factory2,
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_SOLO.to_string()),
        resume_from: Some(ResumeState {
            run_id: res1.run_id.clone(),
            prior_step_results,
            approved_step_id: String::new(),
            completed_units: BTreeMap::new(),
            reason: PauseReason::Manual,
            paused_step: Some(PausedStep {
                step_id: "solo".into(),
                seed_messages: seed,
            }),
            rejected_reason: None,
        }),
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(recorder2.clone()),
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };

    let res2 = run_workflow(opts2).await.expect("resume completes");
    assert!(
        res2.awaiting.is_none(),
        "resumed run must run to completion"
    );
    assert_eq!(res2.step_results.len(), 1);
    assert!(res2.step_results[0].success);
    assert_eq!(res2.step_results[0].output, "done");

    let record_final = store.load(&res1.run_id).unwrap();
    assert_eq!(record_final.status, RunStatus::Completed);
    assert!(record_final.finished_at.is_some());

    let labels2 = recorder2.labels();
    assert!(
        labels2.contains(&"RunResumed".to_string()),
        "got {labels2:?}"
    );
    assert!(
        labels2.contains(&"StepResumed".to_string()),
        "got {labels2:?}"
    );
    assert!(
        labels2.contains(&"RunCompleted".to_string()),
        "got {labels2:?}"
    );

    // A genuinely fresh provider call was issued on resume (not a replay of
    // stale state).
    assert_eq!(
        captured_requests.lock().unwrap().len(),
        1,
        "resume must issue exactly one fresh provider request"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — a 2-step workflow pauses at the step boundary; resume runs only
// the remaining step.
// ---------------------------------------------------------------------------

const WF_TWO_STEP: &str = r#"
name: two-step-pause
steps:
  - id: alpha
    agent: worker
    prompt: "step one"
  - id: beta
    agent: worker
    prompt: "step two, prior: {{ steps.alpha.output }}"
"#;

/// Dispatches step `alpha` with a provider that cancels the pause token
/// right after it answers; panics if asked to dispatch anything else (step
/// `beta` must never be reached in phase 1 — the boundary pause must stop
/// the loop before it).
struct CancelOnAlphaFactory {
    token: CancellationToken,
}
#[async_trait]
impl StepFactory for CancelOnAlphaFactory {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: PathBuf,
        transcript_path: PathBuf,
        on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        assert_eq!(
            step_id, "alpha",
            "step-boundary pause must stop the loop before step 2 is ever dispatched"
        );
        let provider = CancelAfterInner {
            inner: MockProvider::new(vec![ScriptedTurn::AssistantText {
                text: "alpha done".into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }]),
            token: self.token.clone(),
        };
        linear_agent_opts(
            Box::new(provider),
            agent_name,
            rendered_prompt,
            run_id,
            workspace_id,
            workspace_path,
            transcript_path,
            on_tool_call,
        )
    }
}

/// Records every step id it was asked to build opts for; echoes the
/// rendered prompt as the final answer.
#[derive(Default)]
struct EchoFactory {
    seen: Mutex<Vec<String>>,
}
#[async_trait]
impl StepFactory for EchoFactory {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: PathBuf,
        transcript_path: PathBuf,
        on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        self.seen.lock().unwrap().push(step_id.to_string());
        let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: format!("done: {rendered_prompt}"),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        linear_agent_opts(
            Box::new(provider),
            agent_name,
            rendered_prompt,
            run_id,
            workspace_id,
            workspace_path,
            transcript_path,
            on_tool_call,
        )
    }
}

#[tokio::test]
async fn workflow_pause_resume_runs_remaining_steps() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_TWO_STEP).unwrap();

    // --- Phase 1: step 1 runs to completion, pause lands before step 2. ---
    let token = CancellationToken::new();
    let factory1 = Arc::new(CancelOnAlphaFactory {
        token: token.clone(),
    });
    let recorder1 = Arc::new(EventRecorder::default());

    let opts1 = OrchestratorRunOpts {
        workflow: wf.clone(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_pause_wf".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: factory1,
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_TWO_STEP.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(recorder1.clone()),
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: Some(token),
    };

    let res1 = run_workflow(opts1).await.expect("phase 1 returns Ok");
    let awaiting = res1
        .awaiting
        .clone()
        .expect("must pause at the step boundary");
    assert_eq!(awaiting.reason, PauseReason::Manual);
    assert_eq!(
        awaiting.step_id, "beta",
        "must pause BEFORE dispatching step 2"
    );
    assert_eq!(res1.step_results.len(), 1, "step 1 must have completed");
    assert_eq!(res1.step_results[0].step_id, "alpha");
    assert!(res1.step_results[0].success);
    assert!(res1.step_results[0].output.contains("alpha done"));

    let labels1 = recorder1.labels();
    assert!(
        labels1.contains(&"RunPaused".to_string()),
        "got {labels1:?}"
    );
    assert!(
        !labels1.contains(&"RunCompleted".to_string()),
        "got {labels1:?}"
    );

    // Durable checkpoint of step 1 — read back off disk, not memory.
    let record1 = store.load(&res1.run_id).unwrap();
    assert_eq!(record1.status, RunStatus::Paused);
    let persisted_steps = store.read_step_results(&res1.run_id).unwrap();
    assert_eq!(persisted_steps.len(), 1);
    assert_eq!(persisted_steps[0].step_id, "alpha");
    assert!(persisted_steps[0].success);

    // --- Phase 2: resume from disk → only step 2 runs. ---
    let mut record2 = store.load(&res1.run_id).unwrap();
    record2.status = RunStatus::Running;
    record2.finished_at = None;
    store.update(&record2).unwrap();

    let prior_step_results: Vec<StepResult> = store
        .read_step_results(&res1.run_id)
        .unwrap()
        .iter()
        .map(StepResult::from)
        .collect();

    let factory2 = Arc::new(EchoFactory::default());
    let recorder2 = Arc::new(EventRecorder::default());
    let opts2 = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: record2.workspace_id.clone(),
        workspace_path: record2.workspace_path.clone(),
        transcript_dir: record2.transcript_dir.clone(),
        factory: factory2.clone(),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_TWO_STEP.to_string()),
        resume_from: Some(ResumeState {
            run_id: res1.run_id.clone(),
            prior_step_results,
            approved_step_id: String::new(),
            completed_units: BTreeMap::new(),
            reason: PauseReason::Manual,
            paused_step: None,
            rejected_reason: None,
        }),
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(recorder2.clone()),
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };

    let res2 = run_workflow(opts2).await.expect("resume completes");
    assert!(res2.awaiting.is_none());
    assert_eq!(
        res2.step_results.len(),
        2,
        "both steps present after resume"
    );
    assert_eq!(res2.step_results[0].step_id, "alpha");
    assert_eq!(res2.step_results[1].step_id, "beta");
    assert!(res2.step_results[1].success);
    assert!(res2.step_results[1].output.contains("step two"));

    // Resume dispatched ONLY step 2 — step 1 is NOT re-run.
    assert_eq!(
        factory2.seen.lock().unwrap().clone(),
        vec!["beta".to_string()],
        "resume must dispatch only the remaining step"
    );

    let record_final = store.load(&res1.run_id).unwrap();
    assert_eq!(record_final.status, RunStatus::Completed);
    let labels2 = recorder2.labels();
    assert!(
        labels2.contains(&"RunResumed".to_string()),
        "got {labels2:?}"
    );
    assert!(
        labels2.contains(&"RunCompleted".to_string()),
        "got {labels2:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — a `distribute:` fan-out pauses mid-flight; resume re-dispatches
// only the incomplete units.
// ---------------------------------------------------------------------------

const WF_FANOUT: &str = r#"
name: fanout-pause
steps:
  - id: process
    for_each: "a\nb\nc"
    agent: worker
    prompt: "Process {{ item }}"
    max_parallel: 1
    distribute:
      hosts: [h1]
"#;

/// Cancels the pause token immediately after its FIRST dispatch returns —
/// see the module docs for why the in-flight unit still completes intact.
struct CancelFirstUnitDispatcher {
    token: CancellationToken,
    calls: Mutex<Vec<(usize, String)>>,
}
#[async_trait]
impl UnitDispatcher for CancelFirstUnitDispatcher {
    async fn dispatch_unit(&self, unit: UnitDispatch, host: &str) -> Result<UnitOutcome, RunError> {
        let is_first = self.calls.lock().unwrap().is_empty();
        self.calls
            .lock()
            .unwrap()
            .push((unit.index, host.to_string()));
        let outcome = UnitOutcome {
            output: format!("out-{}-on-{host}", unit.index),
            success: true,
            error: None,
            workspace_delta: None,
        };
        if is_first {
            self.token.cancel();
        }
        Ok(outcome)
    }
}

/// Records every `(index, host)` pair dispatched to it. No cancellation —
/// used for the resume pass.
#[derive(Default)]
struct RecordingUnitDispatcher {
    calls: Mutex<Vec<(usize, String)>>,
}
#[async_trait]
impl UnitDispatcher for RecordingUnitDispatcher {
    async fn dispatch_unit(&self, unit: UnitDispatch, host: &str) -> Result<UnitOutcome, RunError> {
        self.calls
            .lock()
            .unwrap()
            .push((unit.index, host.to_string()));
        Ok(UnitOutcome {
            output: format!("out-{}-on-{host}", unit.index),
            success: true,
            error: None,
            workspace_delta: None,
        })
    }
}

#[tokio::test]
async fn fanout_pause_resumes_only_incomplete_units() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_FANOUT).unwrap();

    // --- Phase 1: unit 0 completes, pause lands mid-fan-out. ---
    let token = CancellationToken::new();
    let dispatcher1 = Arc::new(CancelFirstUnitDispatcher {
        token: token.clone(),
        calls: Mutex::new(Vec::new()),
    });
    let recorder1 = Arc::new(EventRecorder::default());

    let opts1 = OrchestratorRunOpts {
        workflow: wf.clone(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_pause_fanout".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_FANOUT.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(recorder1.clone()),
        unit_dispatcher: Some(dispatcher1.clone()),
        action_dispatcher: None,
        pause: Some(token),
    };

    let res1 = run_workflow(opts1).await.expect("phase 1 returns Ok");
    let awaiting = res1.awaiting.clone().expect("must pause mid-fan-out");
    assert_eq!(awaiting.reason, PauseReason::Manual);
    assert_eq!(awaiting.step_id, "process");
    assert!(
        res1.step_results.is_empty(),
        "the fan-out step must not be recorded complete while paused"
    );

    let labels1 = recorder1.labels();
    assert!(
        labels1.contains(&"RunPaused".to_string()),
        "got {labels1:?}"
    );
    assert!(
        labels1.contains(&"StepPaused".to_string()),
        "got {labels1:?}"
    );

    // Only unit 0 ever reached the dispatcher — units 1 and 2 never
    // started (not just "failed").
    assert_eq!(
        dispatcher1.calls.lock().unwrap().clone(),
        vec![(0, "h1".to_string())],
        "only the first unit should have been dispatched"
    );
    assert_eq!(awaiting.fanout_completed_units.len(), 1);
    assert!(awaiting.fanout_completed_units.contains_key(&0));

    // Durable: run Paused, exactly one (successful) unit checkpoint on
    // disk — the not-yet-started units are simply absent, not "failed".
    let record1 = store.load(&res1.run_id).unwrap();
    assert_eq!(record1.status, RunStatus::Paused);
    let checkpoints = store.read_unit_checkpoints(&res1.run_id).unwrap();
    assert_eq!(
        checkpoints.len(),
        1,
        "only the completed unit is checkpointed"
    );
    assert_eq!(checkpoints[0].index, 0);
    assert!(checkpoints[0].success);
    assert_eq!(checkpoints[0].output, "out-0-on-h1");

    // --- Phase 2: resume, built the way `rupu workflow resume` does —
    // only SUCCESSFUL checkpoints replay; everything else re-dispatches. ---
    let mut completed_units: BTreeMap<String, BTreeMap<usize, ItemResult>> = BTreeMap::new();
    for cp in checkpoints.iter().filter(|c| c.success) {
        completed_units
            .entry(cp.step_id.clone())
            .or_default()
            .insert(
                cp.index,
                ItemResult {
                    index: cp.index,
                    item: cp.item.clone(),
                    sub_id: String::new(),
                    rendered_prompt: String::new(),
                    run_id: cp.run_id.clone(),
                    transcript_path: cp.transcript_path.clone(),
                    output: cp.output.clone(),
                    success: true,
                },
            );
    }

    let mut record2 = store.load(&res1.run_id).unwrap();
    record2.status = RunStatus::Running;
    record2.finished_at = None;
    store.update(&record2).unwrap();

    let dispatcher2 = Arc::new(RecordingUnitDispatcher::default());
    let recorder2 = Arc::new(EventRecorder::default());
    let opts2 = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: record2.workspace_id.clone(),
        workspace_path: record2.workspace_path.clone(),
        transcript_dir: record2.transcript_dir.clone(),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_FANOUT.to_string()),
        resume_from: Some(ResumeState {
            run_id: res1.run_id.clone(),
            prior_step_results: Vec::new(),
            approved_step_id: String::new(),
            completed_units,
            reason: PauseReason::Manual,
            paused_step: None,
            rejected_reason: None,
        }),
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(recorder2.clone()),
        unit_dispatcher: Some(dispatcher2.clone()),
        action_dispatcher: None,
        pause: None,
    };

    let res2 = run_workflow(opts2).await.expect("resume completes");
    assert!(res2.awaiting.is_none(), "resumed run runs to completion");
    assert_eq!(res2.step_results.len(), 1);
    let step = &res2.step_results[0];
    assert!(step.success);
    assert_eq!(step.items.len(), 3, "all three units present, in order");
    assert_eq!(
        step.items[0].output, "out-0-on-h1",
        "unit 0 preserved from checkpoint"
    );
    assert_eq!(step.items[1].output, "out-1-on-h1");
    assert_eq!(step.items[2].output, "out-2-on-h1");

    // No duplicate execution: resume dispatched ONLY units 1 and 2.
    assert_eq!(
        dispatcher2.calls.lock().unwrap().clone(),
        vec![(1, "h1".to_string()), (2, "h1".to_string())],
        "resume must re-dispatch only the paused/not-yet-started units"
    );

    let record_final = store.load(&res1.run_id).unwrap();
    assert_eq!(record_final.status, RunStatus::Completed);
    let labels2 = recorder2.labels();
    assert!(
        labels2.contains(&"RunResumed".to_string()),
        "got {labels2:?}"
    );
    assert!(
        labels2.contains(&"RunCompleted".to_string()),
        "got {labels2:?}"
    );
}
