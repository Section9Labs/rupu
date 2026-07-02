//! E2e integration test — distributed `for_each` fan-out across two placements.
//!
//! Drives `run_workflow` with a real `Workflow`, a run-store-backed
//! `OrchestratorRunOpts`, and a fake `UnitDispatcher`.  Asserts:
//!
//! 1. Units dispatched round-robin (h1/h2 by index 0→h1, 1→h2, 2→h1, 3→h2).
//! 2. `step.items` aggregated in INDEX order from the dispatcher outputs.
//! 3. The run completes as ONE run (one run_id, `RunStatus::Completed`).
//! 4. Each unit's `host` is persisted in `unit_checkpoints.jsonl`.
//! 5. A no-`distribute` control workflow runs locally, produces the same
//!    structural results (same items processed in order), and its checkpoints
//!    carry `host: None` — proving backward compatibility.
//!
//! Harness mirrors `tests/linear_runner.rs` (run_store + FakeFactory pattern).
//! The `PanicFactory` mirrors the in-module `PanicFactory` from `runner.rs`
//! — when every unit is remote, `build_opts_for_step` must never be called.

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
// Shared fake factories
// ---------------------------------------------------------------------------

/// Panics if `build_opts_for_step` is ever called.  Used for the distributed
/// workflow where every unit is routed to a remote host — local dispatch must
/// never happen.
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
        _workspace_path: std::path::PathBuf,
        _transcript_path: std::path::PathBuf,
        _on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        panic!("PanicFactory: build_opts_for_step must not be called for fully-distributed units");
    }
}

/// Echoes `"step {step_id} agent {agent_name} echo: {rendered_prompt}"` as the
/// final assistant turn.  Mirrors `FakeFactory` in `tests/linear_runner.rs`.
struct EchoFactory;

#[async_trait]
impl StepFactory for EchoFactory {
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
            pause: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Fake UnitDispatcher
// ---------------------------------------------------------------------------

/// Records every `(unit.index, host)` pair dispatched to it.
/// Returns `out-{index}-on-{host}` as the unit output.
struct RecordingDispatcher {
    calls: Mutex<Vec<(usize, String)>>,
}

impl RecordingDispatcher {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls_sorted(&self) -> Vec<(usize, String)> {
        let mut v = self.calls.lock().unwrap().clone();
        v.sort_by_key(|(idx, _)| *idx);
        v
    }
}

#[async_trait]
impl UnitDispatcher for RecordingDispatcher {
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

// ---------------------------------------------------------------------------
// Workflow YAML fixtures
// ---------------------------------------------------------------------------

/// Distributed workflow: `for_each` + `distribute.hosts = [h1, h2]`, 4 items.
const WF_DISTRIBUTED: &str = r#"
name: e2e-distributed
steps:
  - id: process
    agent: dummy
    actions: []
    for_each: "a\nb\nc\nd"
    prompt: "Process {{ item }} ({{ loop.index }}/{{ loop.length }})"
    max_parallel: 4
    distribute:
      hosts: [h1, h2]
"#;

/// Control workflow: same items, same prompt, but NO `distribute:`.
/// Units run locally via `EchoFactory`.
const WF_LOCAL: &str = r#"
name: e2e-local-control
steps:
  - id: process
    agent: dummy
    actions: []
    for_each: "a\nb\nc\nd"
    prompt: "Process {{ item }} ({{ loop.index }}/{{ loop.length }})"
    max_parallel: 4
"#;

// ---------------------------------------------------------------------------
// Test 1 — distributed fan-out: round-robin, results in index order,
//           one run, per-unit host persisted in unit_checkpoints.jsonl
// ---------------------------------------------------------------------------

#[tokio::test]
async fn distributed_fanout_round_robin_results_and_host_persisted() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let runs_root = tmp.path().join("runs");
    let store = Arc::new(RunStore::new(runs_root));
    let dispatcher = Arc::new(RecordingDispatcher::new());

    let wf = Workflow::parse(WF_DISTRIBUTED).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_e2e_dist".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_DISTRIBUTED.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: Some(dispatcher.clone()),
        pause: None,
    };

    let res = run_workflow(opts)
        .await
        .expect("distributed workflow should succeed");

    // --- (1) Round-robin host dispatch ---
    let calls = dispatcher.calls_sorted();
    assert_eq!(
        calls,
        vec![
            (0, "h1".to_string()),
            (1, "h2".to_string()),
            (2, "h1".to_string()),
            (3, "h2".to_string()),
        ],
        "units should be dispatched round-robin by index; got: {calls:?}"
    );

    // --- (2) Results aggregated in index order ---
    assert_eq!(res.step_results.len(), 1);
    let step = &res.step_results[0];
    assert!(step.success, "all units succeeded → step success");
    assert_eq!(step.items.len(), 4);
    assert_eq!(step.items[0].output, "out-0-on-h1", "index 0 → h1");
    assert_eq!(step.items[1].output, "out-1-on-h2", "index 1 → h2");
    assert_eq!(step.items[2].output, "out-2-on-h1", "index 2 → h1");
    assert_eq!(step.items[3].output, "out-3-on-h2", "index 3 → h2");

    // --- (3) One run, status Completed ---
    assert!(!res.run_id.is_empty(), "run_id should be populated");
    let record = store.load(&res.run_id).expect("run record should exist");
    assert_eq!(
        record.status,
        RunStatus::Completed,
        "run should be Completed"
    );
    assert!(record.finished_at.is_some(), "finished_at should be set");
    assert!(
        record.error_message.is_none(),
        "no error_message on success"
    );

    // --- (4) Per-unit host persisted in unit_checkpoints.jsonl ---
    let mut checkpoints = store
        .read_unit_checkpoints(&res.run_id)
        .expect("unit_checkpoints.jsonl should be readable");
    // Sort by index so assertions are order-stable regardless of concurrency.
    checkpoints.sort_by_key(|c| c.index);

    assert_eq!(checkpoints.len(), 4, "one checkpoint per unit");
    assert_eq!(checkpoints[0].host, Some("h1".to_string()), "unit 0 → h1");
    assert_eq!(checkpoints[1].host, Some("h2".to_string()), "unit 1 → h2");
    assert_eq!(checkpoints[2].host, Some("h1".to_string()), "unit 2 → h1");
    assert_eq!(checkpoints[3].host, Some("h2".to_string()), "unit 3 → h2");
    // Step id preserved in each checkpoint.
    for cp in &checkpoints {
        assert_eq!(cp.step_id, "process");
        assert!(cp.success);
    }
}

// ---------------------------------------------------------------------------
// Test 2 — no-distribute control: same items, local, host: None on checkpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_fanout_control_produces_results_with_no_host_attribution() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let runs_root = tmp.path().join("runs");
    let store = Arc::new(RunStore::new(runs_root));

    let wf = Workflow::parse(WF_LOCAL).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_e2e_local".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(EchoFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_LOCAL.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        pause: None,
    };

    let res = run_workflow(opts)
        .await
        .expect("local workflow should succeed");

    // The step has 4 items processed in declared order.
    assert_eq!(res.step_results.len(), 1);
    let step = &res.step_results[0];
    assert!(step.success, "all local units should succeed");
    assert_eq!(step.items.len(), 4, "4 items processed");

    // Items remain in declared (index) order.
    let item_values: Vec<&str> = step
        .items
        .iter()
        .map(|i| i.item.as_str().unwrap_or(""))
        .collect();
    assert_eq!(item_values, vec!["a", "b", "c", "d"]);

    // Each local item's output contains the echoed prompt — proving the
    // EchoFactory ran correctly and backward compat is preserved.
    assert!(
        step.items[0].output.contains("Process a"),
        "item 0 output should contain 'Process a'; got: {}",
        step.items[0].output
    );
    assert!(
        step.items[3].output.contains("Process d"),
        "item 3 output should contain 'Process d'; got: {}",
        step.items[3].output
    );

    // --- One run, status Completed ---
    assert!(!res.run_id.is_empty(), "run_id should be populated");
    let record = store.load(&res.run_id).expect("run record should exist");
    assert_eq!(record.status, RunStatus::Completed);
    assert!(record.finished_at.is_some());
    assert!(record.error_message.is_none());

    // --- host: None on all unit_checkpoints (backward compat) ---
    let mut checkpoints = store
        .read_unit_checkpoints(&res.run_id)
        .expect("unit_checkpoints.jsonl should be readable");
    checkpoints.sort_by_key(|c| c.index);

    assert_eq!(checkpoints.len(), 4, "one checkpoint per unit");
    for (i, cp) in checkpoints.iter().enumerate() {
        assert_eq!(
            cp.host, None,
            "local unit {i} should have host=None; got: {:?}",
            cp.host
        );
        assert_eq!(cp.step_id, "process");
        assert!(cp.success, "local unit {i} should succeed");
    }
}
