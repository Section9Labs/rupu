//! The runner names every placed unit attempt in the workflow run's netflow
//! roster before it dispatches it (spec
//! 2026-10-14-rupu-remote-netflow-transport-design.md §N3), so the CP's id
//! walk finds the unit's landed ledgers — or the gap where they should be —
//! even when the coordinator dies before the unit ends. A `host: local` step
//! adds no line: its ledgers are already here.

use async_trait::async_trait;
use rupu_agent::RunError;
use rupu_netflow::land::{read_roster, RosterLine};
use rupu_orchestrator::runner::{
    run_workflow, OrchestratorRunOpts, StepFactory, UnitCoverage, UnitDispatch, UnitDispatcher,
    UnitFailure, UnitOutcome, WorkspaceDelta,
};
use rupu_orchestrator::{RunStore, Workflow};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

struct PanicFactory;

#[async_trait]
impl StepFactory for PanicFactory {
    async fn launch_for_step(
        &self,
        _request: rupu_orchestrator::StepRequest,
    ) -> Result<rupu_orchestrator::StepLaunch, rupu_runtime::assembly::AssembleError> {
        panic!("placed units must not be built locally");
    }
}

/// Records every dispatch as a roster line would, and fails the FIRST
/// attempt of fan-out unit 0 so the runner retries it on the fallback host.
#[derive(Default)]
struct Recording {
    dispatched: Mutex<Vec<RosterLine>>,
    failed_once: Mutex<bool>,
}

#[async_trait]
impl UnitDispatcher for Recording {
    async fn strip_delta_coverage(&self, delta: &WorkspaceDelta) -> Result<WorkspaceDelta, String> {
        Ok(delta.clone())
    }

    async fn dispatch_unit(
        &self,
        unit: UnitDispatch,
        host: &str,
    ) -> Result<UnitOutcome, UnitFailure> {
        let is_fanout = unit.step_id == "fan";
        self.dispatched.lock().unwrap().push(RosterLine {
            unit_run_id: unit.run_id.clone(),
            host: host.to_string(),
            step_id: unit.step_id.clone(),
            index: is_fanout.then_some(unit.index),
        });
        if is_fanout && unit.index == 0 {
            let mut failed = self.failed_once.lock().unwrap();
            if !*failed {
                *failed = true;
                return Err(UnitFailure {
                    warnings: Vec::new(),
                    error: RunError::Provider("primary died".into()),
                    coverage: UnitCoverage::Stream(Vec::new()),
                });
            }
        }
        Ok(UnitOutcome {
            warnings: Vec::new(),
            output: format!("ok {}", unit.index),
            success: true,
            error: None,
            workspace_delta: None,
            coverage: UnitCoverage::Stream(Vec::new()),
        })
    }
}

const YAML: &str = r#"
name: w
steps:
  - id: here
    agent: sec
    prompt: p
    host: local
  - id: s
    agent: sec
    prompt: p
    host: host_a
  - id: fan
    agent: sec
    actions: []
    for_each: "x\ny"
    prompt: "p {{ item }}"
    distribute:
      hosts: [host_a, host_b]
"#;

fn sorted(mut lines: Vec<RosterLine>) -> Vec<RosterLine> {
    lines.sort_by(|a, b| a.unit_run_id.cmp(&b.unit_run_id));
    lines
}

#[tokio::test]
async fn every_placed_attempt_is_in_the_roster_and_a_local_step_is_not() {
    let tmp = tempfile::tempdir().unwrap();
    let runs_root = tmp.path().join("runs");
    let dispatcher = Arc::new(Recording::default());
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(YAML).unwrap(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_nf".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::new(RunStore::new(runs_root.clone()))),
        workflow_yaml: Some(YAML.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: Some(dispatcher.clone()),
        action_services: None,
        pause: None,
        naming: None,
    };
    let result = run_workflow(opts).await.expect("the workflow runs");
    assert!(!result.run_id.is_empty());

    let dispatched = dispatcher.dispatched.lock().unwrap().clone();
    // The local step, the placed step, two fan-out units and unit 0's retry.
    assert_eq!(dispatched.len(), 5, "{dispatched:?}");
    assert!(
        dispatched
            .iter()
            .any(|l| l.host == "local" && l.step_id == "here"),
        "{dispatched:?}"
    );

    let roster = read_roster(&runs_root, &result.run_id);
    let remote: Vec<RosterLine> = dispatched
        .into_iter()
        .filter(|l| l.host != "local")
        .collect();
    assert_eq!(roster.len(), 4, "{roster:?}");
    assert_eq!(sorted(roster.clone()), sorted(remote));

    // The placed step, with no index.
    assert!(
        roster
            .iter()
            .any(|l| l.step_id == "s" && l.host == "host_a" && l.index.is_none()),
        "{roster:?}"
    );
    // Unit 0 twice — the failed primary and its retry — under different ids
    // on different hosts; unit 1 once.
    let unit0: Vec<&RosterLine> = roster
        .iter()
        .filter(|l| l.step_id == "fan" && l.index == Some(0))
        .collect();
    assert_eq!(unit0.len(), 2, "{roster:?}");
    assert_ne!(unit0[0].unit_run_id, unit0[1].unit_run_id);
    assert_ne!(unit0[0].host, unit0[1].host);
    assert_eq!(
        roster
            .iter()
            .filter(|l| l.step_id == "fan" && l.index == Some(1))
            .count(),
        1,
        "{roster:?}"
    );
}

/// An in-memory run (no store) has nowhere to keep a roster and must not
/// try: the run still completes.
#[tokio::test]
async fn a_run_without_a_store_keeps_no_roster_and_still_completes() {
    let tmp = tempfile::tempdir().unwrap();
    let dispatcher = Arc::new(Recording::default());
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(YAML).unwrap(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_nf".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: None,
        workflow_yaml: Some(YAML.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: Some(dispatcher.clone()),
        action_services: None,
        pause: None,
        naming: None,
    };
    let result = run_workflow(opts).await.expect("the workflow runs");
    assert!(result.run_id.is_empty());
    assert_eq!(dispatcher.dispatched.lock().unwrap().len(), 5);
    assert!(read_roster(&tmp.path().join("runs"), "").is_empty());
}
