//! Every linear / `for_each` attempt writes one `attempts.jsonl` row at the
//! moment it STARTS (Task 3 of recover-on-interrupt plan 2). The ledger is
//! what `rupu workflow resume` reads to find the transcript to continue, so
//! each row must name the right step / unit / host and point at the transcript
//! the attempt actually writes.
//!
//! Harness: a real disk-backed `RunStore` plus `run_workflow` driven through
//! its public `OrchestratorRunOpts` (same shape as `placed_step_e2e.rs` and
//! `distributed_fanout_e2e.rs`). Local runs use a `MockProvider`; placed runs
//! use a fake `UnitDispatcher` so no agent loop is needed.

use async_trait::async_trait;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::{AgentRunOpts, RunError};
use rupu_orchestrator::runner::{
    run_workflow, OrchestratorRunOpts, StepFactory, UnitDispatch, UnitDispatcher, UnitOutcome,
};
use rupu_orchestrator::runs::AttemptRecord;
use rupu_orchestrator::{RunStore, Workflow};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Hands every agent its own single-turn `MockProvider`, so a fan-out's units
/// (and a linear step) each finish on their own.
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
        workspace_path: PathBuf,
        transcript_path: PathBuf,
        on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: format!("step {step_id} echo: {rendered_prompt}"),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        AgentRunOpts {
            seed_source: None,
            agent_name: agent_name.to_string(),
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
            step_id: step_id.to_string(),
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
}

/// Panics if a local agent is ever built — every attempt in the placed tests
/// goes through the `UnitDispatcher`.
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
        panic!("PanicFactory: placed attempts must not build a local agent");
    }
}

/// Echoes a canned output, but fails the FIRST dispatch of `fail_index` (when
/// set) so the runner retries that unit on the fallback host under a fresh run
/// id. Records every `(step_id, unit index, host, run id)` it was handed.
struct FlakyDispatcher {
    fail_index: Option<usize>,
    calls: Mutex<Vec<(String, usize, String, String)>>,
}

impl FlakyDispatcher {
    fn new(fail_index: Option<usize>) -> Self {
        Self {
            fail_index,
            calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl UnitDispatcher for FlakyDispatcher {
    async fn dispatch_unit(&self, unit: UnitDispatch, host: &str) -> Result<UnitOutcome, RunError> {
        let first_try = {
            let mut calls = self.calls.lock().unwrap();
            let first = !calls
                .iter()
                .any(|(s, i, _, _)| *s == unit.step_id && *i == unit.index);
            calls.push((
                unit.step_id.clone(),
                unit.index,
                host.to_string(),
                unit.run_id.clone(),
            ));
            first
        };
        if self.fail_index == Some(unit.index) && first_try {
            return Err(RunError::Provider("host down".into()));
        }
        Ok(UnitOutcome {
            output: format!("out-{}-on-{host}", unit.index),
            success: true,
            error: None,
            workspace_delta: None,
        })
    }
}

fn opts_for(
    tmp: &std::path::Path,
    yaml: &str,
    factory: Arc<dyn StepFactory>,
    store: &Arc<RunStore>,
    dispatcher: Option<Arc<dyn UnitDispatcher>>,
) -> OrchestratorRunOpts {
    OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(yaml).unwrap(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_attempts".into(),
        workspace_path: tmp.to_path_buf(),
        transcript_dir: tmp.join("transcripts"),
        factory,
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(store)),
        workflow_yaml: Some(yaml.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: dispatcher,
        action_dispatcher: None,
        pause: None,
        naming: None,
    }
}

fn units_of<'a>(atts: &'a [AttemptRecord], step_id: &str) -> Vec<&'a AttemptRecord> {
    atts.iter().filter(|a| a.step_id == step_id).collect()
}

const WF_LOCAL: &str = r#"
name: attempts-local
steps:
  - id: lin
    agent: worker
    actions: []
    prompt: "do the linear thing"
  - id: each
    agent: worker
    actions: []
    for_each: "a\nb"
    prompt: "Process {{ item }}"
    max_parallel: 2
"#;

#[tokio::test]
async fn local_linear_and_fanout_attempts_are_recorded_with_their_transcripts() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let res = run_workflow(opts_for(
        tmp.path(),
        WF_LOCAL,
        Arc::new(EchoFactory),
        &store,
        None,
    ))
    .await
    .expect("local run completes");

    let atts = store.read_attempts(&res.run_id).unwrap();
    assert_eq!(
        atts.len(),
        3,
        "one row per linear step + one per fan-out unit, got {atts:?}"
    );

    let fe = units_of(&atts, "each");
    assert_eq!(fe.len(), 2);
    assert_eq!(
        fe.iter()
            .filter_map(|a| a.unit_index)
            .collect::<BTreeSet<_>>(),
        [0usize, 1].into_iter().collect()
    );

    let lin = units_of(&atts, "lin");
    assert_eq!(lin.len(), 1);
    assert_eq!(lin[0].unit_index, None);
    assert_eq!(lin[0].sub_id, None);

    for a in &atts {
        assert_eq!(a.v, 1);
        assert_eq!(a.host, None, "a local attempt has no host: {a:?}");
        assert_eq!(a.continued_from, None, "a fresh attempt continues nothing");
        assert!(
            a.transcript_path
                .ends_with(format!("{}.jsonl", a.agent_run_id)),
            "transcript path must be <agent_run_id>.jsonl: {a:?}"
        );
        // The ledger row is written at start, but by the time the run
        // finished the transcript it names must be the one on disk.
        assert!(
            a.transcript_path.exists(),
            "the attempt's transcript must exist: {a:?}"
        );
    }

    // Each row's run id is the one the step result recorded — the ledger
    // names the SAME run the rest of the run state does.
    let sr_lin = res
        .step_results
        .iter()
        .find(|s| s.step_id == "lin")
        .unwrap();
    assert_eq!(lin[0].agent_run_id, sr_lin.run_id);
    let sr_each = res
        .step_results
        .iter()
        .find(|s| s.step_id == "each")
        .unwrap();
    for item in &sr_each.items {
        let row = fe
            .iter()
            .find(|a| a.unit_index == Some(item.index))
            .expect("a row per unit");
        assert_eq!(row.agent_run_id, item.run_id);
        assert_eq!(row.transcript_path, item.transcript_path);
    }
}

const WF_PLACED_LINEAR: &str = r#"
name: attempts-placed-linear
steps:
  - id: gather
    agent: worker
    actions: []
    host: edge-1
    prompt: "gather things"
"#;

#[tokio::test]
async fn placed_linear_attempt_records_its_host() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let dispatcher = Arc::new(FlakyDispatcher::new(None));
    let res = run_workflow(opts_for(
        tmp.path(),
        WF_PLACED_LINEAR,
        Arc::new(PanicFactory),
        &store,
        Some(dispatcher.clone() as Arc<dyn UnitDispatcher>),
    ))
    .await
    .expect("placed run completes");

    let atts = store.read_attempts(&res.run_id).unwrap();
    assert_eq!(atts.len(), 1, "got {atts:?}");
    let a = &atts[0];
    assert_eq!(a.step_id, "gather");
    assert_eq!(a.unit_index, None);
    assert_eq!(a.host.as_deref(), Some("edge-1"));
    // The ledger names the run id the dispatcher was actually handed.
    let calls = dispatcher.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(a.agent_run_id, calls[0].3);
    assert!(a
        .transcript_path
        .ends_with(format!("{}.jsonl", a.agent_run_id)));
}

const WF_PLACED_FANOUT: &str = r#"
name: attempts-placed-fanout
steps:
  - id: process
    agent: worker
    actions: []
    for_each: "a\nb\nc\nd"
    prompt: "Process {{ item }}"
    max_parallel: 4
    distribute:
      hosts: [h1, h2]
"#;

#[tokio::test]
async fn placed_fanout_retry_on_the_fallback_host_gets_its_own_row() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    // Unit 1 is placed on h2; its first dispatch fails, so it retries on h1
    // (the next host) under a freshly minted run id.
    let dispatcher = Arc::new(FlakyDispatcher::new(Some(1)));
    let res = run_workflow(opts_for(
        tmp.path(),
        WF_PLACED_FANOUT,
        Arc::new(PanicFactory),
        &store,
        Some(dispatcher.clone() as Arc<dyn UnitDispatcher>),
    ))
    .await
    .expect("the retry recovers the unit");

    let atts = store.read_attempts(&res.run_id).unwrap();
    assert_eq!(
        atts.len(),
        5,
        "4 first attempts + 1 retry row for unit 1, got {atts:?}"
    );
    for a in &atts {
        assert_eq!(a.step_id, "process");
        assert!(a.unit_index.is_some(), "fan-out rows carry a unit index");
        assert!(a
            .transcript_path
            .ends_with(format!("{}.jsonl", a.agent_run_id)));
    }

    // Round-robin: 0→h1, 1→h2, 2→h1, 3→h2.
    let host_of = |idx: usize| -> Vec<(Option<String>, String)> {
        atts.iter()
            .filter(|a| a.unit_index == Some(idx))
            .map(|a| (a.host.clone(), a.agent_run_id.clone()))
            .collect()
    };
    assert_eq!(host_of(0).len(), 1);
    assert_eq!(host_of(0)[0].0.as_deref(), Some("h1"));
    assert_eq!(host_of(2).len(), 1);
    assert_eq!(host_of(2)[0].0.as_deref(), Some("h1"));
    assert_eq!(host_of(3).len(), 1);
    assert_eq!(host_of(3)[0].0.as_deref(), Some("h2"));

    let unit1 = host_of(1);
    assert_eq!(unit1.len(), 2, "primary + retry, got {unit1:?}");
    let hosts: BTreeSet<_> = unit1.iter().filter_map(|(h, _)| h.clone()).collect();
    assert_eq!(
        hosts,
        ["h1".to_string(), "h2".to_string()].into_iter().collect(),
        "the retry row names the fallback host"
    );
    assert_ne!(unit1[0].1, unit1[1].1, "the retry mints a fresh run id");

    // Both rows' run ids are the ones the dispatcher received for unit 1.
    let dispatched: BTreeSet<_> = dispatcher
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, i, _, _)| *i == 1)
        .map(|(_, _, host, run_id)| (host.clone(), run_id.clone()))
        .collect();
    let recorded: BTreeSet<_> = unit1
        .iter()
        .map(|(h, r)| (h.clone().unwrap(), r.clone()))
        .collect();
    assert_eq!(recorded, dispatched);
}
