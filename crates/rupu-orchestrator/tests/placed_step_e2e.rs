//! End-to-end: a linear step with `host:` runs through the UnitDispatcher
//! port (a fake remote), its output feeds the downstream step, and the run
//! is one coherent run with per-step host attribution. A no-host control
//! confirms byte-for-byte local behavior is unchanged.

use async_trait::async_trait;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::{AgentRunOpts, RunError};
use rupu_orchestrator::runner::{
    run_workflow, OrchestratorRunOpts, StepFactory, UnitDispatch, UnitDispatcher, UnitOutcome,
};
use rupu_orchestrator::{RunStatus, RunStore, Workflow};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Fake StepFactory for local (no-host) runs
// ---------------------------------------------------------------------------

/// Echoes `"step {step_id} agent {agent_name} echo: {rendered_prompt}"` as the
/// final assistant turn. Used by the no-host control test.
struct FakeFactory;

#[async_trait]
impl StepFactory for FakeFactory {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: std::path::PathBuf,
        transcript_path: std::path::PathBuf,
        on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: format!("step {step_id} agent {agent_name} echo: {rendered_prompt}"),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        AgentRunOpts {
            agent_name: format!("ag-{agent_name}"),
            agent_system_prompt: "echo".into(),
            agent_tools: None,
            provider: Box::new(provider),
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
            no_stream: false,
            suppress_stream_stdout: false,
            mcp_registry: None,
            effort: None,
            context_window: None,
            output_format: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            parent_run_id: None,
            depth: 0,
            dispatchable_agents: None,
            step_id: step_id.to_string(),
            on_tool_call,
            on_stream_event: None,
            concerns: None,
            max_tokens: rupu_agent::runner::DEFAULT_MAX_TOKENS,
            scope_name: None,
            surface_tag: None,
            context_window_tokens: None,
            compact_at_percent: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Fake UnitDispatcher for placed-step tests
// ---------------------------------------------------------------------------

/// Records every `(step_id, agent, rendered_prompt, host)` dispatched to it.
/// Returns `"REMOTE[{rendered_prompt}]"` as the unit output.
struct RecordingDispatcher {
    calls: Mutex<Vec<(String, String, String, String)>>,
}

impl RecordingDispatcher {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls_snapshot(&self) -> Vec<(String, String, String, String)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl UnitDispatcher for RecordingDispatcher {
    async fn dispatch_unit(&self, unit: UnitDispatch, host: &str) -> Result<UnitOutcome, RunError> {
        self.calls.lock().unwrap().push((
            unit.step_id.clone(),
            unit.agent.clone(),
            unit.rendered_prompt.clone(),
            host.to_string(),
        ));
        Ok(UnitOutcome {
            output: format!("REMOTE[{}]", unit.rendered_prompt),
            success: true,
            error: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Workflow YAML fixtures
// ---------------------------------------------------------------------------

/// Placed workflow: two linear steps each targeting a named remote host.
const WF_PLACED: &str = r#"
name: placed-e2e
steps:
  - id: gather
    agent: gatherer
    prompt: "gather {{ inputs.topic }}"
    host: worker-1
  - id: summarize
    agent: summarizer
    prompt: "summarize: {{ steps.gather.output }}"
    host: worker-2
"#;

/// Control workflow: same two steps without `host:` — runs locally.
const WF_LOCAL: &str = r#"
name: placed-e2e-local
steps:
  - id: gather
    agent: gatherer
    prompt: "gather {{ inputs.topic }}"
  - id: summarize
    agent: summarizer
    prompt: "summarize: {{ steps.gather.output }}"
"#;

// ---------------------------------------------------------------------------
// Test 1 — placed steps route through the dispatcher and chain outputs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn placed_steps_run_remotely_and_chain() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let runs_root = tmp.path().join("runs");
    let store = Arc::new(RunStore::new(runs_root));
    let dispatcher = Arc::new(RecordingDispatcher::new());

    let mut inputs = BTreeMap::new();
    inputs.insert("topic".to_string(), "rust".to_string());

    let wf = Workflow::parse(WF_PLACED).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs,
        workspace_id: "ws_placed_e2e".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(FakeFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_PLACED.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: Some(dispatcher.clone()),
    };

    let res = run_workflow(opts)
        .await
        .expect("placed workflow should succeed");

    // --- Dispatcher saw two calls in declared step order ---
    let calls = dispatcher.calls_snapshot();
    assert_eq!(
        calls.len(),
        2,
        "dispatcher should have been called twice; got: {calls:?}"
    );

    let (step_id_0, agent_0, prompt_0, host_0) = &calls[0];
    assert_eq!(step_id_0, "gather", "first dispatch should be 'gather'");
    assert_eq!(agent_0, "gatherer", "gather agent should be 'gatherer'");
    assert_eq!(
        prompt_0, "gather rust",
        "gather prompt should render inputs.topic"
    );
    assert_eq!(host_0, "worker-1", "gather should target worker-1");

    let (step_id_1, agent_1, prompt_1, host_1) = &calls[1];
    assert_eq!(
        step_id_1, "summarize",
        "second dispatch should be 'summarize'"
    );
    assert_eq!(
        agent_1, "summarizer",
        "summarize agent should be 'summarizer'"
    );
    assert!(
        prompt_1.contains("REMOTE[gather rust]"),
        "summarize prompt should contain gather's remote output; got: {prompt_1}"
    );
    assert_eq!(host_1, "worker-2", "summarize should target worker-2");

    // --- Both steps succeeded and outputs are chained correctly ---
    assert_eq!(res.step_results.len(), 2, "two step results expected");
    assert!(
        res.step_results[0].success,
        "gather step should report success"
    );
    assert!(
        res.step_results[1].success,
        "summarize step should report success"
    );
    assert_eq!(
        res.step_results[1].output, "REMOTE[summarize: REMOTE[gather rust]]",
        "summarize output should be the remote-wrapped chained prompt"
    );

    // --- One coherent run, status Completed ---
    assert!(!res.run_id.is_empty(), "run_id should be populated");
    let record = store.load(&res.run_id).expect("run record should exist");
    assert_eq!(
        record.status,
        RunStatus::Completed,
        "run should reach Completed"
    );
    assert!(record.finished_at.is_some(), "finished_at should be set");
    assert!(
        record.error_message.is_none(),
        "no error_message on success"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — no-host control: same workflow without `host:` runs locally
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_host_control_runs_locally() {
    let tmp = assert_fs::TempDir::new().unwrap();

    let mut inputs = BTreeMap::new();
    inputs.insert("topic".to_string(), "rust".to_string());

    let wf = Workflow::parse(WF_LOCAL).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs,
        workspace_id: "ws_placed_e2e_local".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(FakeFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
    };

    let res = run_workflow(opts)
        .await
        .expect("local workflow should succeed");

    // Both steps ran locally via FakeFactory — dispatcher was never consulted.
    assert_eq!(res.step_results.len(), 2, "two steps should run locally");
    assert!(
        res.step_results[0].success,
        "gather step should succeed locally"
    );
    assert!(
        res.step_results[1].success,
        "summarize step should succeed locally"
    );

    // The gather step's local output contains the FakeFactory echo format.
    assert!(
        res.step_results[0].output.contains("gather rust"),
        "gather local output should contain the rendered prompt; got: {}",
        res.step_results[0].output
    );

    // The summarize step's rendered prompt shows gather's local output was chained in.
    assert!(
        res.step_results[1]
            .rendered_prompt
            .contains("step gather agent gatherer echo"),
        "summarize prompt should reference gather's local FakeFactory output; got: {}",
        res.step_results[1].rendered_prompt
    );
}
