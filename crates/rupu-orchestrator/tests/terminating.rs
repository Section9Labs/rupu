//! Once SIGTERM has arrived (`credential_writes::terminating()`), the
//! orchestrator starts no new work: no further unit is dispatched to a host
//! and no further node is launched — the run stops as cancelled while the
//! handler drains the pending credential writes.
//!
//! Its own test binary: the flag is process-wide and never cleared, so the
//! two phases run in order inside one test — the per-unit check first (the
//! flag is raised by the first unit's dispatch), then the scheduler check
//! (the flag is already up before the run starts).

use async_trait::async_trait;
use rupu_agent::AgentRunOpts;
use rupu_orchestrator::runner::{
    run_workflow, OrchestratorRunOpts, RunWorkflowError, StepFactory, UnitCoverage, UnitDispatch,
    UnitDispatcher, UnitFailure, UnitOutcome,
};
use rupu_orchestrator::Workflow;
use rupu_providers::credential_writes;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Panics if a local step is ever built.
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
        panic!("no step may be built once the process is terminating");
    }
}

/// Records each dispatched unit and raises the termination flag on the
/// first one — SIGTERM landing while that unit runs on its host.
struct TerminatingDispatcher {
    calls: Mutex<Vec<usize>>,
}

#[async_trait]
impl UnitDispatcher for TerminatingDispatcher {
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
        self.calls.lock().unwrap().push(unit.index);
        credential_writes::request_termination();
        Ok(UnitOutcome {
            output: format!("out-{}-on-{host}", unit.index),
            success: true,
            error: None,
            workspace_delta: None,
            coverage: UnitCoverage::NotLaunched,
        })
    }
}

/// Three placed units, one at a time.
const WF_FANOUT: &str = r#"
name: terminating-fanout
steps:
  - id: process
    agent: dummy
    actions: []
    for_each: "a\nb\nc"
    prompt: "Process {{ item }}"
    max_parallel: 1
    distribute:
      hosts: [h1]
"#;

const WF_LINEAR: &str = r#"
name: terminating-linear
steps:
  - id: only
    agent: dummy
    actions: []
    prompt: "never dispatched"
"#;

fn opts(
    wf: &str,
    tmp: &std::path::Path,
    dispatcher: Option<Arc<dyn UnitDispatcher>>,
) -> OrchestratorRunOpts {
    OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(wf).unwrap(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_terminating".into(),
        workspace_path: tmp.to_path_buf(),
        transcript_dir: tmp.join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: None,
        workflow_yaml: None,
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

#[tokio::test]
async fn a_terminating_process_dispatches_no_further_unit_and_no_further_node() {
    let tmp = assert_fs::TempDir::new().unwrap();
    assert!(!credential_writes::terminating(), "fresh process");

    // Phase 1: SIGTERM lands while the first unit runs; the other two are
    // never dispatched.
    let dispatcher = Arc::new(TerminatingDispatcher {
        calls: Mutex::new(Vec::new()),
    });
    let res = run_workflow(opts(WF_FANOUT, tmp.path(), Some(dispatcher.clone()))).await;
    assert!(res.is_err(), "the run does not complete: {res:?}");
    assert_eq!(
        dispatcher.calls.lock().unwrap().len(),
        1,
        "only the unit already running when SIGTERM landed was dispatched"
    );

    // Phase 2: the flag is up before the run starts; no node is launched
    // (the factory would panic), and the run ends as cancelled.
    assert!(credential_writes::terminating());
    let res = run_workflow(opts(WF_LINEAR, tmp.path(), None)).await;
    match res {
        Err(RunWorkflowError::RunCancelled { aborted }) => assert_eq!(aborted, 0),
        other => panic!("expected RunCancelled, got {other:?}"),
    }
}
