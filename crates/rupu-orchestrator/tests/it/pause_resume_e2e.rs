//! E2e — pause/resume round-trips across run / workflow / fan-out (T9).
//!
//! Mirrors the harness shape of `tests/it/distributed_fanout_e2e.rs` and
//! `tests/it/linear_runner.rs`: a real disk-backed `RunStore`, `run_workflow`
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
use rupu_agent::continuation::CONTINUATION_NOTE;
use rupu_agent::runner::{BypassDecider, CapturingMockProvider, MockProvider, ScriptedTurn};
use rupu_agent::{AgentRunOpts, RunError};
use rupu_orchestrator::executor::{AttemptResumeMode, Event, EventSink};
use rupu_orchestrator::recovery::{discover, AttemptPlan, RecoveryPlans};
use rupu_orchestrator::runner::{
    run_workflow, ItemResult, OrchestratorRunOpts, OrchestratorRunResult, PauseReason, PausedStep,
    ResumeState, StepFactory, UnitCoverage, UnitDispatch, UnitDispatcher, UnitFailure, UnitOutcome,
};
use rupu_orchestrator::runs::AttemptRecord;
use rupu_orchestrator::{RunStatus, RunStore, StepResult, Workflow};
use rupu_providers::types::{
    ContentBlock, LlmRequest, LlmResponse, Message, Role, StopReason, StreamEvent,
};
use rupu_providers::{LlmProvider, ProviderError, ProviderId};
use rupu_tools::ToolContext;
use std::collections::{BTreeMap, BTreeSet};
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
    /// `UnitStarted` / `UnitCompleted` / `AttemptResumed`, in emission order.
    unit_events: Mutex<Vec<Event>>,
}
impl EventRecorder {
    fn labels(&self) -> Vec<String> {
        self.labels.lock().unwrap().clone()
    }
    fn unit_events(&self) -> Vec<Event> {
        self.unit_events.lock().unwrap().clone()
    }
}
impl EventSink for EventRecorder {
    fn emit(&self, _run_id: &str, ev: &Event) {
        if matches!(
            ev,
            Event::UnitStarted { .. } | Event::UnitCompleted { .. } | Event::AttemptResumed { .. }
        ) {
            self.unit_events.lock().unwrap().push(ev.clone());
        }
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

/// Build a minimal `AgentRunOpts` around `provider`. The runner always
/// streams (`no_stream: true` only quiets the display), and it races that
/// `provider.stream` call against the pause token — the deterministic
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
        seed_source: None,
        collectors: Vec::new(),
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
        on_usage: None,
        concerns: None,
        limits: rupu_providers::model_limits::ModelLimits::unknown(),
        scope_name: None,
        surface_tag: None,
        pause: None,
        codename: None,
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
        run_step: Default::default(),
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
        naming: None,
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
        run_step: Default::default(),
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
            paused_steps: vec![PausedStep {
                step_id: "solo".into(),
                seed_messages: seed,
            }],
            rejected_reason: None,
            ..Default::default()
        }),
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(recorder2.clone()),
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
        naming: None,
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
        run_step: Default::default(),
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
        naming: None,
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
        run_step: Default::default(),
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
            paused_steps: Vec::new(),
            rejected_reason: None,
            ..Default::default()
        }),
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(recorder2.clone()),
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
        naming: None,
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
    async fn strip_delta_coverage(
        &self,
        delta: &rupu_orchestrator::runner::WorkspaceDelta,
    ) -> Result<rupu_orchestrator::runner::WorkspaceDelta, String> {
        Ok(delta.clone())
    }

    async fn dispatch_unit(
        &self,
        unit: UnitDispatch,
        host: &str,
    ) -> Result<UnitOutcome, UnitFailure> {
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
            coverage: UnitCoverage::NotLaunched,
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
    async fn strip_delta_coverage(
        &self,
        delta: &rupu_orchestrator::runner::WorkspaceDelta,
    ) -> Result<rupu_orchestrator::runner::WorkspaceDelta, String> {
        Ok(delta.clone())
    }

    async fn dispatch_unit(
        &self,
        unit: UnitDispatch,
        host: &str,
    ) -> Result<UnitOutcome, UnitFailure> {
        self.calls
            .lock()
            .unwrap()
            .push((unit.index, host.to_string()));
        Ok(UnitOutcome {
            output: format!("out-{}-on-{host}", unit.index),
            success: true,
            error: None,
            workspace_delta: None,
            coverage: UnitCoverage::NotLaunched,
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
        run_step: Default::default(),
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
        naming: None,
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
                    is_fixer: false,
                    codename: None,
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
        run_step: Default::default(),
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
            paused_steps: Vec::new(),
            rejected_reason: None,
            ..Default::default()
        }),
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(recorder2.clone()),
        unit_dispatcher: Some(dispatcher2.clone()),
        action_dispatcher: None,
        pause: None,
        naming: None,
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

// ---------------------------------------------------------------------------
// A finished fan-out unit is durable while its siblings are still running
// ---------------------------------------------------------------------------

const WF_FANOUT_ONE_HANGS: &str = r#"
name: fanout-durable
steps:
  - id: process
    for_each: "fast\nslow"
    agent: worker
    prompt: "Process {{ item }}"
    max_parallel: 2
"#;

/// `fast` units answer at once; `slow` units never return, keeping the
/// fan-out in flight for as long as the test needs.
struct FastOrHangFactory;
#[async_trait]
impl StepFactory for FastOrHangFactory {
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
        let provider: Box<dyn LlmProvider> = if rendered_prompt.contains("slow") {
            Box::new(BlockingProvider)
        } else {
            Box::new(MockProvider::new(vec![ScriptedTurn::AssistantText {
                text: "fast done".into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }]))
        };
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

/// A runner that dies mid-fan-out (terminal closed, process killed, gate
/// sweep reap) must not take its finished units with it: each unit's
/// checkpoint has to land the moment that unit finishes, not when the
/// whole fan-out joins — otherwise a resume re-runs every finished unit.
#[tokio::test]
async fn fanout_unit_checkpoint_is_durable_while_siblings_still_run() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_FANOUT_ONE_HANGS).unwrap();
    let run_id = "run_durable_fanout".to_string();

    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_durable_fanout".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(FastOrHangFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_FANOUT_ONE_HANGS.to_string()),
        resume_from: None,
        run_id_override: Some(run_id.clone()),
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
        naming: None,
    };
    let run = tokio::spawn(run_workflow(opts));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let checkpoints = loop {
        let cps = store.read_unit_checkpoints(&run_id).unwrap_or_default();
        if !cps.is_empty() || tokio::time::Instant::now() >= deadline {
            break cps;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(
        !run.is_finished(),
        "the hanging `slow` unit must keep the fan-out in flight"
    );
    // The runner dies here, with `slow` still running.
    run.abort();

    assert_eq!(
        checkpoints.len(),
        1,
        "the finished `fast` unit must be checkpointed before its sibling finishes, got {checkpoints:?}"
    );
    let cp = &checkpoints[0];
    assert_eq!(cp.step_id, "process");
    assert_eq!(cp.index, 0);
    assert!(cp.success);
    assert_eq!(cp.output, "fast done");
}

// ---------------------------------------------------------------------------
// Resume continues / recovers / restarts what a dead runner left mid-fan-out
// ---------------------------------------------------------------------------

const WF_FANOUT_RECOVER: &str = r#"
name: fanout-recover
steps:
  - id: process
    for_each: "a\nb\nc"
    agent: worker
    prompt: "Process {{ item }}"
    max_parallel: 1
"#;

/// Runs one tool turn, then hangs on the next call — an agent whose process
/// was killed mid-turn.
struct ToolThenHangProvider {
    first: MockProvider,
    calls: usize,
}
impl ToolThenHangProvider {
    fn new() -> Self {
        Self {
            first: MockProvider::new(vec![ScriptedTurn::AssistantToolUse {
                text: None,
                tool_id: "call_1".into(),
                tool_name: "read_file".into(),
                tool_input: serde_json::json!({ "path": "notes.txt" }),
                stop: StopReason::ToolUse,
            }]),
            calls: 0,
        }
    }
}
#[async_trait]
impl LlmProvider for ToolThenHangProvider {
    async fn send(&mut self, req: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        self.calls += 1;
        if self.calls == 1 {
            return self.first.send(req).await;
        }
        BlockingProvider.send(req).await
    }
    async fn stream(
        &mut self,
        req: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        self.calls += 1;
        if self.calls == 1 {
            return self.first.stream(req, on_event).await;
        }
        BlockingProvider.send(req).await
    }
    fn default_model(&self) -> &str {
        "mock-1"
    }
    fn provider_id(&self) -> ProviderId {
        ProviderId::Anthropic
    }
}

/// Unit `x` (prompt `Process x`) answers `x done`. With `hang_b`, unit `b`
/// instead runs one tool turn and hangs. Records every prompt it is asked to
/// build and the requests each answering provider receives.
struct RecoverFactory {
    hang_b: bool,
    built: Mutex<Vec<String>>,
    captured: Mutex<BTreeMap<String, Arc<Mutex<Vec<LlmRequest>>>>>,
}
impl RecoverFactory {
    fn new(hang_b: bool) -> Arc<Self> {
        Arc::new(Self {
            hang_b,
            built: Mutex::new(Vec::new()),
            captured: Mutex::new(BTreeMap::new()),
        })
    }
    /// The rendered prompts that reached a local dispatch, in order.
    fn built(&self) -> Vec<String> {
        self.built.lock().unwrap().clone()
    }
    /// The requests the provider for `prompt` received.
    fn requests(&self, prompt: &str) -> Vec<LlmRequest> {
        self.captured
            .lock()
            .unwrap()
            .get(prompt)
            .map(|c| c.lock().unwrap().clone())
            .unwrap_or_default()
    }
}
#[async_trait]
impl StepFactory for RecoverFactory {
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
        self.built.lock().unwrap().push(rendered_prompt.clone());
        let unit = rendered_prompt.trim_start_matches("Process ").to_string();
        let provider: Box<dyn LlmProvider> = if self.hang_b && unit == "b" {
            Box::new(ToolThenHangProvider::new())
        } else {
            let p = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
                text: format!("{unit} done"),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }]);
            self.captured
                .lock()
                .unwrap()
                .insert(rendered_prompt.clone(), p.captured.clone());
            Box::new(p)
        };
        let mut opts = linear_agent_opts(
            provider,
            agent_name,
            rendered_prompt,
            run_id,
            workspace_id,
            workspace_path.clone(),
            transcript_path,
            on_tool_call,
        );
        opts.tool_context.workspace_path = workspace_path;
        opts
    }
}

/// A run whose runner died after `a` finished, with `b` mid-turn (one tool
/// turn on disk, the next model call in flight) and `c` (if any) never
/// started. `a`/`b`/`c` are `for_each` units or linear steps, per the
/// workflow.
struct Killed {
    tmp: tempfile::TempDir,
    store: Arc<RunStore>,
    wf: Workflow,
    /// The workflow's YAML, as the run's snapshot.
    yaml: &'static str,
    run_id: String,
    /// `b`'s interrupted attempt, as the ledger recorded it.
    b: AttemptRecord,
}

async fn kill_fanout_mid_unit_b() -> Killed {
    kill_mid_b(
        WF_FANOUT_RECOVER,
        "run_recover_fanout",
        // `a` is checkpointed ...
        |store, run_id| {
            !store
                .read_unit_checkpoints(run_id)
                .unwrap_or_default()
                .is_empty()
        },
        // ... and `b` is the second unit.
        |attempt| attempt.unit_index == Some(1),
    )
    .await
}

/// Run `yaml` (via [`RecoverFactory`] with `b` hanging) until `a_done` says
/// `a` is recorded and `b` (the attempt `is_b` picks out of the ledger) has
/// its first turn on disk — the runner is then stuck inside the next model
/// call — and kill the runner there.
async fn kill_mid_b(
    yaml: &'static str,
    run_id: &str,
    a_done: fn(&RunStore, &str) -> bool,
    is_b: fn(&AttemptRecord) -> bool,
) -> Killed {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("notes.txt"), "alpha\nbeta\n").unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(yaml).unwrap();
    // The runner registry is process-wide and keyed by run id, so tests
    // running side by side in this process must not share one: a resume that
    // met another test's live runner would hand off and do nothing.
    static NEXT_RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let run_id = format!(
        "{run_id}_{}",
        NEXT_RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: wf.clone(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_recover".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: RecoverFactory::new(true),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(yaml.to_string()),
        resume_from: None,
        run_id_override: Some(run_id.clone()),
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
        naming: None,
    };
    let run = tokio::spawn(run_workflow(opts));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let b = loop {
        let b = store
            .read_attempts(&run_id)
            .unwrap_or_default()
            .into_iter()
            .find(is_b);
        let b_tool_turn_done = b.as_ref().is_some_and(|a| {
            std::fs::read_to_string(&a.transcript_path)
                .map(|t| t.contains("\"turn_end\""))
                .unwrap_or(false)
        });
        if a_done(&store, &run_id) && b_tool_turn_done {
            break b.unwrap();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the runner never reached b's second model call"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(!run.is_finished(), "b must keep the run in flight");
    // The runner dies here. Awaiting the aborted task is what drops its
    // future — and with it the in-process runner registration — before the
    // resume claims the run; without it a resume racing that drop would meet
    // a "live" runner and hand off, doing nothing.
    run.abort();
    let _ = run.await;
    Killed {
        tmp,
        store,
        wf,
        yaml,
        run_id,
        b,
    }
}

impl Killed {
    /// Successful unit checkpoints grouped the way `rupu workflow resume`
    /// groups them into `ResumeState::completed_units`.
    fn completed_units(&self) -> BTreeMap<String, BTreeMap<usize, ItemResult>> {
        let mut completed: BTreeMap<String, BTreeMap<usize, ItemResult>> = BTreeMap::new();
        for cp in self.store.read_unit_checkpoints(&self.run_id).unwrap() {
            let per_step = completed.entry(cp.step_id.clone()).or_default();
            if cp.success {
                per_step.insert(
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
                        is_fixer: false,
                        codename: cp.codename.clone(),
                    },
                );
            } else {
                per_step.remove(&cp.index);
            }
        }
        completed.retain(|_, units| !units.is_empty());
        completed
    }

    /// Steps with a recorded result, as `rupu workflow resume` reads them.
    fn done_step_ids(&self) -> BTreeSet<String> {
        self.store
            .read_step_results(&self.run_id)
            .unwrap()
            .into_iter()
            .map(|r| r.step_id)
            .collect()
    }

    /// What resume's discovery makes of the attempts the dead runner left.
    fn plans(&self) -> RecoveryPlans {
        let settled: BTreeMap<String, BTreeMap<usize, ()>> = self
            .completed_units()
            .into_iter()
            .map(|(step, units)| (step, units.keys().map(|i| (*i, ())).collect()))
            .collect();
        discover(
            &self.store,
            &self.run_id,
            &self.wf,
            &self.done_step_ids(),
            &settled,
        )
    }

    /// Re-enter `run_workflow` as a resume carrying `plans`.
    async fn resume(
        &self,
        factory: Arc<RecoverFactory>,
        plans: RecoveryPlans,
    ) -> (OrchestratorRunResult, Arc<EventRecorder>) {
        self.resume_on(self.wf.clone(), factory, plans).await
    }

    /// [`Self::resume`] against `wf` (the workflow as it reads now).
    async fn resume_on(
        &self,
        wf: Workflow,
        factory: Arc<RecoverFactory>,
        plans: RecoveryPlans,
    ) -> (OrchestratorRunResult, Arc<EventRecorder>) {
        self.resume_with(wf, factory, plans, Vec::new()).await
    }

    /// [`Self::resume_on`], also carrying `paused_steps` (what a manual
    /// pause left, as `rupu workflow resume` reads them off the run).
    async fn resume_with(
        &self,
        wf: Workflow,
        factory: Arc<RecoverFactory>,
        plans: RecoveryPlans,
        paused_steps: Vec<PausedStep>,
    ) -> (OrchestratorRunResult, Arc<EventRecorder>) {
        let record = self.store.load(&self.run_id).unwrap();
        let recorder = Arc::new(EventRecorder::default());
        let opts = OrchestratorRunOpts {
            run_step: Default::default(),
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: record.workspace_id.clone(),
            workspace_path: record.workspace_path.clone(),
            transcript_dir: record.transcript_dir.clone(),
            factory,
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&self.store)),
            workflow_yaml: Some(self.yaml.to_string()),
            resume_from: Some(ResumeState {
                run_id: self.run_id.clone(),
                prior_step_results: self
                    .store
                    .read_step_results(&self.run_id)
                    .unwrap()
                    .iter()
                    .map(StepResult::from)
                    .collect(),
                completed_units: self.completed_units(),
                reason: PauseReason::Approval,
                paused_steps,
                recovery: plans,
                ..Default::default()
            }),
            run_id_override: None,
            strict_templates: false,
            event_sink: Some(recorder.clone()),
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
            naming: None,
        };
        let res = run_workflow(opts).await.expect("resume completes");
        (res, recorder)
    }

    /// The ledger rows of unit `idx`, in append order.
    fn attempts_of(&self, idx: usize) -> Vec<AttemptRecord> {
        self.store
            .read_attempts(&self.run_id)
            .unwrap()
            .into_iter()
            .filter(|a| a.unit_index == Some(idx))
            .collect()
    }
}

/// `(unit index, mode, from_agent_run_id, reason)` of one `AttemptResumed`.
type Resumed = (
    Option<usize>,
    AttemptResumeMode,
    Option<String>,
    Option<String>,
);

/// Every `AttemptResumed` the recorder saw, in order.
fn attempt_resumed(recorder: &EventRecorder) -> Vec<Resumed> {
    recorder
        .unit_events()
        .into_iter()
        .filter_map(|e| match e {
            Event::AttemptResumed {
                unit_index,
                mode,
                from_agent_run_id,
                reason,
                ..
            } => Some((unit_index, mode, from_agent_run_id, reason)),
            _ => None,
        })
        .collect()
}

/// The lifecycle events one unit emitted, in order: `started`, `resumed:<mode>`
/// and `completed`.
fn unit_lifecycle(recorder: &EventRecorder, idx: usize) -> Vec<String> {
    recorder
        .unit_events()
        .into_iter()
        .filter_map(|e| match e {
            Event::UnitStarted { index, .. } if index == idx => Some("started".to_string()),
            Event::AttemptResumed {
                unit_index, mode, ..
            } if unit_index == Some(idx) => Some(format!("resumed:{mode:?}")),
            Event::UnitCompleted { index, .. } if index == idx => Some("completed".to_string()),
            _ => None,
        })
        .collect()
}

fn outputs(res: &OrchestratorRunResult) -> Vec<String> {
    assert_eq!(res.step_results.len(), 1);
    res.step_results[0]
        .items
        .iter()
        .map(|i| i.output.clone())
        .collect()
}

/// The conversation a resumed provider was sent, flattened to JSON for
/// substring assertions.
fn sent_text(req: &LlmRequest) -> String {
    serde_json::to_string(&req.messages).unwrap()
}

#[tokio::test]
async fn resume_continues_a_unit_killed_mid_turn() {
    let killed = kill_fanout_mid_unit_b().await;

    // Discovery: `a` is settled by its checkpoint, `c` never started; only
    // `b` has something to continue.
    let plans = killed.plans();
    let step_plans = &plans.0["process"];
    assert_eq!(step_plans.units.len(), 1, "got {step_plans:?}");
    match &step_plans.units[&1] {
        AttemptPlan::Continue {
            from_agent_run_id, ..
        } => assert_eq!(from_agent_run_id, &killed.b.agent_run_id),
        other => panic!("expected Continue for unit b, got {other:?}"),
    }

    let factory = RecoverFactory::new(false);
    let (res, recorder) = killed.resume(factory.clone(), plans).await;
    assert!(res.awaiting.is_none(), "the resumed run completes");
    assert!(res.step_results[0].success);
    assert_eq!(outputs(&res), ["a done", "b done", "c done"]);

    // `a` was replayed from its checkpoint, never re-dispatched.
    assert_eq!(
        factory.built(),
        ["Process b", "Process c"],
        "unit a must not be re-dispatched"
    );

    // `b`'s provider was sent the rebuilt conversation — prompt, the tool
    // turn it had completed — with the continuation note joined onto the
    // trailing user turn (the tool result).
    let b_requests = factory.requests("Process b");
    assert_eq!(b_requests.len(), 1);
    let b_messages = &b_requests[0].messages;
    assert!(sent_text(&b_requests[0]).contains("Process b"));
    assert_eq!(b_messages.len(), 3, "user, assistant tool call, user");
    let last = b_messages.last().unwrap();
    assert_eq!(last.role, Role::User);
    assert!(last.content.iter().any(
        |b| matches!(b, ContentBlock::ToolResult { content, .. } if content.contains("alpha"))
    ));
    assert!(last
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::Text { text } if text == CONTINUATION_NOTE)));

    // `c` ran fresh: just its prompt, no note.
    let c_requests = factory.requests("Process c");
    assert_eq!(c_requests.len(), 1);
    assert_eq!(c_requests[0].messages.len(), 1);
    assert!(!sent_text(&c_requests[0]).contains(CONTINUATION_NOTE));

    // The resumed attempt is its own ledger row, linked to the one it
    // continued; `c`'s is not linked to anything.
    let b_rows = killed.attempts_of(1);
    assert_eq!(b_rows.len(), 2, "interrupted attempt + its continuation");
    assert_eq!(b_rows[0].agent_run_id, killed.b.agent_run_id);
    assert_eq!(b_rows[0].continued_from, None);
    assert_ne!(b_rows[1].agent_run_id, killed.b.agent_run_id);
    assert_eq!(
        b_rows[1].continued_from.as_deref(),
        Some(killed.b.agent_run_id.as_str())
    );
    assert_eq!(killed.attempts_of(2)[0].continued_from, None);
    assert_eq!(killed.attempts_of(0).len(), 1, "a was not re-attempted");

    // Announced once, for `b` only, between its start and its completion.
    assert_eq!(
        attempt_resumed(&recorder),
        vec![(
            Some(1),
            AttemptResumeMode::Continued,
            Some(killed.b.agent_run_id.clone()),
            None
        )]
    );
    assert_eq!(
        unit_lifecycle(&recorder, 1),
        ["started", "resumed:Continued", "completed"]
    );
    assert_eq!(unit_lifecycle(&recorder, 2), ["started", "completed"]);

    // Every unit is now durably checkpointed.
    let cps = killed.store.read_unit_checkpoints(&killed.run_id).unwrap();
    let mut done: Vec<(usize, bool)> = cps.iter().map(|c| (c.index, c.success)).collect();
    done.sort();
    assert_eq!(done, [(0, true), (1, true), (2, true)]);
    assert_eq!(
        killed.store.load(&killed.run_id).unwrap().status,
        RunStatus::Completed
    );
}

#[tokio::test]
async fn resume_recovers_a_finished_unit_whose_checkpoint_never_landed() {
    let killed = kill_fanout_mid_unit_b().await;
    let a = killed.attempts_of(0).remove(0);

    // The runner died after `a`'s agent finished but before its checkpoint
    // was written.
    std::fs::remove_file(
        killed
            .tmp
            .path()
            .join("runs")
            .join(&killed.run_id)
            .join("unit_checkpoints.jsonl"),
    )
    .unwrap();
    assert!(killed.completed_units().is_empty());

    let plans = killed.plans();
    match &plans.0["process"].units[&0] {
        AttemptPlan::Recovered {
            output,
            agent_run_id,
            ..
        } => {
            assert_eq!(output, "a done");
            assert_eq!(agent_run_id, &a.agent_run_id);
        }
        other => panic!("expected Recovered for unit a, got {other:?}"),
    }

    let factory = RecoverFactory::new(false);
    let (res, recorder) = killed.resume(factory.clone(), plans).await;
    assert!(res.awaiting.is_none());
    assert_eq!(outputs(&res), ["a done", "b done", "c done"]);

    // No dispatch — and so no model call — for the recovered unit.
    assert_eq!(factory.built(), ["Process b", "Process c"]);
    assert_eq!(killed.attempts_of(0).len(), 1, "no new attempt for a");

    // The recovered unit is the finished attempt itself.
    let a_item = &res.step_results[0].items[0];
    assert_eq!(a_item.run_id, a.agent_run_id);
    assert_eq!(a_item.transcript_path, a.transcript_path);
    assert_eq!(a_item.item, serde_json::json!("a"));
    assert!(a_item.success);

    // ... re-checkpointed from the transcript, so the next resume sees it
    // as done.
    let cps = killed.store.read_unit_checkpoints(&killed.run_id).unwrap();
    let a_cp = cps
        .iter()
        .find(|c| c.index == 0)
        .expect("a is checkpointed again");
    assert!(a_cp.success);
    assert_eq!(a_cp.run_id, a.agent_run_id);
    assert_eq!(a_cp.output, "a done");
    assert_eq!(a_cp.item, serde_json::json!("a"));

    // Announced as recovered and completed — the unit shows up, resumed, and
    // is done at once.
    assert_eq!(
        unit_lifecycle(&recorder, 0),
        ["started", "resumed:Recovered", "completed"]
    );
    let unit_events = recorder.unit_events();
    assert!(
        unit_events.iter().any(|e| matches!(
            e,
            Event::UnitCompleted {
                index: 0,
                success: true,
                ..
            }
        )),
        "got {unit_events:?}"
    );
    let resumed = attempt_resumed(&recorder);
    assert!(
        resumed.contains(&(
            Some(0),
            AttemptResumeMode::Recovered,
            Some(a.agent_run_id.clone()),
            None
        )),
        "got {resumed:?}"
    );
    assert!(
        resumed
            .iter()
            .any(|(i, m, ..)| *i == Some(1) && *m == AttemptResumeMode::Continued),
        "b is still continued, got {resumed:?}"
    );
}

#[tokio::test]
async fn resume_restarts_a_unit_whose_transcript_can_not_be_continued() {
    let killed = kill_fanout_mid_unit_b().await;
    // `b`'s transcript is gone: nothing to continue.
    std::fs::remove_file(&killed.b.transcript_path).unwrap();

    let plans = killed.plans();
    assert!(
        matches!(
            plans.0["process"].units.get(&1),
            Some(AttemptPlan::Restart { .. })
        ),
        "got {plans:?}"
    );

    let factory = RecoverFactory::new(false);
    let (res, recorder) = killed.resume(factory.clone(), plans).await;
    assert_eq!(outputs(&res), ["a done", "b done", "c done"]);
    assert_eq!(factory.built(), ["Process b", "Process c"]);

    // A fresh attempt: just the prompt, no note, and no link to the old one.
    let b_requests = factory.requests("Process b");
    assert_eq!(b_requests[0].messages.len(), 1);
    assert!(!sent_text(&b_requests[0]).contains(CONTINUATION_NOTE));
    assert_eq!(killed.attempts_of(1)[1].continued_from, None);

    let resumed = attempt_resumed(&recorder);
    assert_eq!(resumed.len(), 1, "got {resumed:?}");
    let (idx, mode, from, reason) = &resumed[0];
    assert_eq!(*idx, Some(1));
    assert_eq!(*mode, AttemptResumeMode::Restarted);
    assert_eq!(*from, None);
    assert!(reason.is_some(), "a restart says why");
}

#[tokio::test]
async fn resume_restarts_when_a_planned_continuation_turns_out_unreadable() {
    let killed = kill_fanout_mid_unit_b().await;
    // Discovery saw a continuable transcript ...
    let plans = killed.plans();
    assert!(matches!(
        plans.0["process"].units.get(&1),
        Some(AttemptPlan::Continue { .. })
    ));
    // ... which is gone by the time the unit is dispatched.
    std::fs::remove_file(&killed.b.transcript_path).unwrap();

    let factory = RecoverFactory::new(false);
    let (res, recorder) = killed.resume(factory.clone(), plans).await;
    assert_eq!(outputs(&res), ["a done", "b done", "c done"]);

    let b_requests = factory.requests("Process b");
    assert_eq!(b_requests[0].messages.len(), 1, "started from the prompt");
    assert!(!sent_text(&b_requests[0]).contains(CONTINUATION_NOTE));
    assert_eq!(
        killed.attempts_of(1)[1].continued_from,
        None,
        "a restart is not recorded as a continuation"
    );

    let resumed = attempt_resumed(&recorder);
    assert_eq!(resumed.len(), 1, "got {resumed:?}");
    assert_eq!(resumed[0].0, Some(1));
    assert_eq!(resumed[0].1, AttemptResumeMode::Restarted);
    assert!(resumed[0].3.is_some(), "a restart says why");
}

const WF_FANOUT_RECOVER_SHORTER: &str = r#"
name: fanout-recover
steps:
  - id: process
    for_each: "a\nb"
    agent: worker
    prompt: "Process {{ item }}"
    max_parallel: 1
"#;

/// A plan is keyed by unit index, so it is only as good as that index: when
/// the checkpoints say the list used to be longer than it renders now, resume
/// already re-runs every unit rather than trust the mapping — and the plans
/// are dropped with it.
#[tokio::test]
async fn resume_ignores_plans_when_the_fanout_list_changed() {
    let killed = kill_fanout_mid_unit_b().await;
    let plans = killed.plans();
    assert!(matches!(
        plans.0["process"].units.get(&1),
        Some(AttemptPlan::Continue { .. })
    ));
    // The run had also got `c` done — the list has since shrunk to `a`, `b`.
    killed
        .store
        .append_unit_checkpoint(
            &killed.run_id,
            &rupu_orchestrator::runs::UnitCheckpoint {
                step_id: "process".into(),
                index: 2,
                item: serde_json::json!("c"),
                run_id: "run_c".into(),
                transcript_path: killed.tmp.path().join("transcripts/run_c.jsonl"),
                output: "c done".into(),
                success: true,
                finished_at: chrono::Utc::now(),
                host: None,
                codename: None,
            },
        )
        .unwrap();

    let factory = RecoverFactory::new(false);
    let (res, recorder) = killed
        .resume_on(
            Workflow::parse(WF_FANOUT_RECOVER_SHORTER).unwrap(),
            factory.clone(),
            plans,
        )
        .await;
    assert_eq!(outputs(&res), ["a done", "b done"]);
    assert_eq!(
        factory.built(),
        ["Process a", "Process b"],
        "every unit re-runs"
    );
    assert!(!sent_text(&factory.requests("Process b")[0]).contains(CONTINUATION_NOTE));
    assert!(
        attempt_resumed(&recorder).is_empty(),
        "no plan was honoured, got {:?}",
        attempt_resumed(&recorder)
    );
}

// ---------------------------------------------------------------------------
// Resume continues / recovers / restarts what a dead runner left mid-step
// (linear steps)
// ---------------------------------------------------------------------------

const WF_LINEAR_RECOVER: &str = r#"
name: linear-recover
steps:
  - id: first
    agent: worker
    prompt: "Process a"
  - id: second
    agent: worker
    prompt: "Process b"
  - id: third
    agent: worker
    prompt: "Process c"
"#;

/// A three-step linear run whose runner died after `first` finished, with
/// `second` mid-turn and `third` never started.
async fn kill_linear_mid_second() -> Killed {
    kill_mid_b(
        WF_LINEAR_RECOVER,
        "run_recover_linear",
        // `first` has its result recorded ...
        |store, run_id| {
            store
                .read_step_results(run_id)
                .unwrap_or_default()
                .iter()
                .any(|r| r.step_id == "first")
        },
        // ... and `second` is the attempt in flight.
        |attempt| attempt.step_id == "second",
    )
    .await
}

impl Killed {
    /// The ledger rows of step `step_id`, in append order.
    fn attempts_of_step(&self, step_id: &str) -> Vec<AttemptRecord> {
        self.store
            .read_attempts(&self.run_id)
            .unwrap()
            .into_iter()
            .filter(|a| a.step_id == step_id)
            .collect()
    }
}

/// The recorded outputs of a linear run, by step id.
fn step_outputs(res: &OrchestratorRunResult) -> Vec<(String, String)> {
    res.step_results
        .iter()
        .map(|r| (r.step_id.clone(), r.output.clone()))
        .collect()
}

#[tokio::test]
async fn resume_continues_a_linear_step_killed_mid_turn() {
    let killed = kill_linear_mid_second().await;

    // Discovery: `first` is done, `third` never started; only `second` has
    // something to continue.
    let plans = killed.plans();
    assert_eq!(plans.0.len(), 1, "got {plans:?}");
    match plans.0["second"].linear.as_ref() {
        Some(AttemptPlan::Continue {
            from_agent_run_id, ..
        }) => assert_eq!(from_agent_run_id, &killed.b.agent_run_id),
        other => panic!("expected Continue for second, got {other:?}"),
    }

    let factory = RecoverFactory::new(false);
    let (res, recorder) = killed.resume(factory.clone(), plans).await;
    assert!(res.awaiting.is_none(), "the resumed run completes");
    assert_eq!(
        step_outputs(&res),
        [
            ("first".to_string(), "a done".to_string()),
            ("second".to_string(), "b done".to_string()),
            ("third".to_string(), "c done".to_string()),
        ]
    );

    // `first` was replayed from its result, never re-dispatched.
    assert_eq!(
        factory.built(),
        ["Process b", "Process c"],
        "first must not be re-dispatched"
    );

    // `second`'s provider was sent the rebuilt conversation — prompt, the
    // tool turn it had completed — with the continuation note joined onto the
    // trailing user turn (the tool result), and NOT its prompt a second time.
    let b_requests = factory.requests("Process b");
    assert_eq!(b_requests.len(), 1);
    let b_messages = &b_requests[0].messages;
    assert_eq!(b_messages.len(), 3, "user, assistant tool call, user");
    assert!(sent_text(&b_requests[0]).contains("Process b"));
    let last = b_messages.last().unwrap();
    assert_eq!(last.role, Role::User);
    assert!(last.content.iter().any(
        |b| matches!(b, ContentBlock::ToolResult { content, .. } if content.contains("alpha"))
    ));
    assert!(last
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::Text { text } if text == CONTINUATION_NOTE)));

    // `third` ran fresh: just its prompt, no note.
    let c_requests = factory.requests("Process c");
    assert_eq!(c_requests[0].messages.len(), 1);
    assert!(!sent_text(&c_requests[0]).contains(CONTINUATION_NOTE));

    // The resumed attempt is its own ledger row, linked to the one it
    // continued; the others are not linked to anything.
    let b_rows = killed.attempts_of_step("second");
    assert_eq!(b_rows.len(), 2, "interrupted attempt + its continuation");
    assert_eq!(b_rows[0].agent_run_id, killed.b.agent_run_id);
    assert_eq!(b_rows[0].continued_from, None);
    assert_ne!(b_rows[1].agent_run_id, killed.b.agent_run_id);
    assert_eq!(
        b_rows[1].continued_from.as_deref(),
        Some(killed.b.agent_run_id.as_str())
    );
    assert_eq!(killed.attempts_of_step("first").len(), 1);
    assert_eq!(killed.attempts_of_step("third")[0].continued_from, None);

    // Announced once, for `second` only (a linear step has no unit index).
    assert_eq!(
        attempt_resumed(&recorder),
        vec![(
            None,
            AttemptResumeMode::Continued,
            Some(killed.b.agent_run_id.clone()),
            None
        )]
    );
    // A continuation is not a pause resume: no `StepResumed`.
    assert!(
        !recorder.labels().contains(&"StepResumed".to_string()),
        "got {:?}",
        recorder.labels()
    );
    assert_eq!(
        killed.store.load(&killed.run_id).unwrap().status,
        RunStatus::Completed
    );
}

/// `--restart-interrupted` leaves discovery out: the resume carries no plans,
/// so the interrupted step starts over from its prompt, as resume always did.
#[tokio::test]
async fn resume_restarts_a_linear_step_when_no_plans_are_carried() {
    let killed = kill_linear_mid_second().await;

    let factory = RecoverFactory::new(false);
    let (res, recorder) = killed
        .resume(factory.clone(), RecoveryPlans::default())
        .await;
    assert!(res.awaiting.is_none());
    assert_eq!(step_outputs(&res)[1], ("second".into(), "b done".into()));
    assert_eq!(factory.built(), ["Process b", "Process c"]);

    let b_requests = factory.requests("Process b");
    assert_eq!(b_requests[0].messages.len(), 1, "just the prompt");
    assert!(!sent_text(&b_requests[0]).contains(CONTINUATION_NOTE));
    assert_eq!(killed.attempts_of_step("second")[1].continued_from, None);
    assert!(
        attempt_resumed(&recorder).is_empty(),
        "got {:?}",
        attempt_resumed(&recorder)
    );
}

#[tokio::test]
async fn resume_recovers_a_finished_linear_step_whose_result_never_landed() {
    let killed = kill_linear_mid_second().await;
    let first = killed.attempts_of_step("first").remove(0);

    // The runner died after `first`'s agent finished but before its step
    // result was written.
    std::fs::write(
        killed
            .tmp
            .path()
            .join("runs")
            .join(&killed.run_id)
            .join("step_results.jsonl"),
        "",
    )
    .unwrap();
    assert!(killed.done_step_ids().is_empty());

    let plans = killed.plans();
    match plans.0["first"].linear.as_ref() {
        Some(AttemptPlan::Recovered {
            output,
            agent_run_id,
            ..
        }) => {
            assert_eq!(output, "a done");
            assert_eq!(agent_run_id, &first.agent_run_id);
        }
        other => panic!("expected Recovered for first, got {other:?}"),
    }

    let factory = RecoverFactory::new(false);
    let (res, recorder) = killed.resume(factory.clone(), plans).await;
    assert!(res.awaiting.is_none());
    assert_eq!(
        step_outputs(&res),
        [
            ("first".to_string(), "a done".to_string()),
            ("second".to_string(), "b done".to_string()),
            ("third".to_string(), "c done".to_string()),
        ]
    );

    // No dispatch — and so no model call — for the recovered step.
    assert_eq!(factory.built(), ["Process b", "Process c"]);
    assert_eq!(
        killed.attempts_of_step("first").len(),
        1,
        "no new attempt for first"
    );

    // The recovered step is the finished attempt itself ...
    let first_result = &res.step_results[0];
    assert!(first_result.success);
    assert_eq!(first_result.run_id, first.agent_run_id);
    assert_eq!(first_result.transcript_path, first.transcript_path);
    // ... recorded again, so the next resume sees it as done.
    assert!(killed.done_step_ids().contains("first"));

    let resumed = attempt_resumed(&recorder);
    assert!(
        resumed.contains(&(
            None,
            AttemptResumeMode::Recovered,
            Some(first.agent_run_id.clone()),
            None
        )),
        "got {resumed:?}"
    );
    assert!(
        resumed.contains(&(
            None,
            AttemptResumeMode::Continued,
            Some(killed.b.agent_run_id.clone()),
            None
        )),
        "second is still continued, got {resumed:?}"
    );
}

#[tokio::test]
async fn resume_restarts_a_linear_step_whose_transcript_can_not_be_continued() {
    let killed = kill_linear_mid_second().await;
    // `second`'s transcript is gone: nothing to continue.
    std::fs::remove_file(&killed.b.transcript_path).unwrap();

    let plans = killed.plans();
    assert!(
        matches!(plans.0["second"].linear, Some(AttemptPlan::Restart { .. })),
        "got {plans:?}"
    );

    let factory = RecoverFactory::new(false);
    let (res, recorder) = killed.resume(factory.clone(), plans).await;
    assert_eq!(step_outputs(&res)[1], ("second".into(), "b done".into()));

    // A fresh attempt: just the prompt, no note, and no link to the old one.
    let b_requests = factory.requests("Process b");
    assert_eq!(b_requests[0].messages.len(), 1);
    assert!(!sent_text(&b_requests[0]).contains(CONTINUATION_NOTE));
    assert_eq!(killed.attempts_of_step("second")[1].continued_from, None);

    let resumed = attempt_resumed(&recorder);
    assert_eq!(resumed.len(), 1, "got {resumed:?}");
    let (idx, mode, from, reason) = &resumed[0];
    assert_eq!(*idx, None);
    assert_eq!(*mode, AttemptResumeMode::Restarted);
    assert_eq!(*from, None);
    assert!(reason.is_some(), "a restart says why");
}

#[tokio::test]
async fn resume_restarts_a_linear_step_when_a_planned_continuation_turns_out_unreadable() {
    let killed = kill_linear_mid_second().await;
    // Discovery saw a continuable transcript ...
    let plans = killed.plans();
    assert!(matches!(
        plans.0["second"].linear,
        Some(AttemptPlan::Continue { .. })
    ));
    // ... which is gone by the time the step is dispatched.
    std::fs::remove_file(&killed.b.transcript_path).unwrap();

    let factory = RecoverFactory::new(false);
    let (res, recorder) = killed.resume(factory.clone(), plans).await;
    assert_eq!(step_outputs(&res)[1], ("second".into(), "b done".into()));

    let b_requests = factory.requests("Process b");
    assert_eq!(b_requests[0].messages.len(), 1, "started from the prompt");
    assert!(!sent_text(&b_requests[0]).contains(CONTINUATION_NOTE));
    assert_eq!(
        killed.attempts_of_step("second")[1].continued_from,
        None,
        "a restart is not recorded as a continuation"
    );

    let resumed = attempt_resumed(&recorder);
    assert_eq!(resumed.len(), 1, "got {resumed:?}");
    assert_eq!(resumed[0].1, AttemptResumeMode::Restarted);
    assert!(resumed[0].3.is_some(), "a restart says why");
}

/// A step can carry both a paused-step seed (a manual pause landed inside it)
/// and a recovery plan. A continuation rebuilds the conversation from the
/// attempt's own transcript — the most recent thing the step did — so it wins,
/// and the step is seeded once, not twice.
#[tokio::test]
async fn a_continuation_wins_over_a_paused_step_seed() {
    let killed = kill_linear_mid_second().await;
    let plans = killed.plans();
    assert!(matches!(
        plans.0["second"].linear,
        Some(AttemptPlan::Continue { .. })
    ));

    let factory = RecoverFactory::new(false);
    let (res, recorder) = killed
        .resume_with(
            killed.wf.clone(),
            factory.clone(),
            plans,
            vec![PausedStep {
                step_id: "second".into(),
                seed_messages: vec![Message::user("STALE-PAUSED-SEED")],
            }],
        )
        .await;
    assert_eq!(step_outputs(&res)[1], ("second".into(), "b done".into()));

    let b_requests = factory.requests("Process b");
    assert_eq!(b_requests.len(), 1);
    let sent = sent_text(&b_requests[0]);
    assert!(!sent.contains("STALE-PAUSED-SEED"), "double-seeded: {sent}");
    assert_eq!(b_requests[0].messages.len(), 3);
    assert!(sent.contains(CONTINUATION_NOTE));
    assert_eq!(
        killed.attempts_of_step("second")[1]
            .continued_from
            .as_deref(),
        Some(killed.b.agent_run_id.as_str())
    );
    assert_eq!(
        attempt_resumed(&recorder)
            .iter()
            .map(|(_, mode, ..)| *mode)
            .collect::<Vec<_>>(),
        [AttemptResumeMode::Continued]
    );
}

/// A restart carries no conversation, so when the step also paused with a seed
/// that seed — the only conversation left — is what it resumes from, as a plain
/// paused-step resume (`StepResumed`), not as an announced restart.
#[tokio::test]
async fn a_restart_leaves_a_paused_step_seed_in_charge() {
    let killed = kill_linear_mid_second().await;
    std::fs::remove_file(&killed.b.transcript_path).unwrap();
    let plans = killed.plans();
    assert!(matches!(
        plans.0["second"].linear,
        Some(AttemptPlan::Restart { .. })
    ));

    let factory = RecoverFactory::new(false);
    let (res, recorder) = killed
        .resume_with(
            killed.wf.clone(),
            factory.clone(),
            plans,
            vec![PausedStep {
                step_id: "second".into(),
                seed_messages: vec![Message::user("Process b")],
            }],
        )
        .await;
    assert_eq!(step_outputs(&res)[1], ("second".into(), "b done".into()));
    assert!(
        attempt_resumed(&recorder).is_empty(),
        "no restart is announced: {:?}",
        attempt_resumed(&recorder)
    );
    assert!(recorder.labels().contains(&"StepResumed".to_string()));
    assert_eq!(killed.attempts_of_step("second")[1].continued_from, None);
}

const WF_LINEAR_RECOVER_GATED: &str = r#"
name: linear-recover
steps:
  - id: first
    agent: worker
    prompt: "Process a"
  - id: second
    agent: worker
    prompt: "Process b"
    approval:
      required: true
      prompt: "Run second?"
  - id: third
    agent: worker
    prompt: "Process c"
"#;

/// A step with an attempt on record is past its `approval:` gate (the gate
/// precedes dispatch), so continuing that attempt must not park the run at the
/// gate a second time.
#[tokio::test]
async fn a_continued_step_is_not_gated_again() {
    let killed = kill_linear_mid_second().await;
    let plans = killed.plans();

    let factory = RecoverFactory::new(false);
    let (res, _recorder) = killed
        .resume_on(
            Workflow::parse(WF_LINEAR_RECOVER_GATED).unwrap(),
            factory.clone(),
            plans,
        )
        .await;
    assert!(
        res.awaiting.is_none(),
        "the continued step must not re-park at its approval gate: {:?}",
        res.awaiting
    );
    assert_eq!(step_outputs(&res)[1], ("second".into(), "b done".into()));
    assert!(sent_text(&factory.requests("Process b")[0]).contains(CONTINUATION_NOTE));
}
