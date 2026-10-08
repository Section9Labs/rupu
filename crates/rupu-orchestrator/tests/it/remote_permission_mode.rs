//! A remote unit (`host:` / `distribute:`) never reaches the step factory, so
//! the run's permission mode (`rupu workflow run --mode`) has to travel on
//! the `UnitDispatch` the runner hands the fleet dispatcher — or the host
//! runs the unit under its own `permission_mode` default, which can be
//! `bypass` under a `--mode readonly` workflow. These drive real workflows
//! through `run_workflow` and assert what each dispatched unit carried.

use async_trait::async_trait;
use rupu_agent::RunError;
use rupu_orchestrator::runner::{
    run_workflow, OrchestratorRunOpts, StepFactory, UnitCoverage, UnitDispatch, UnitDispatcher,
    UnitFailure, UnitOutcome,
};
use rupu_orchestrator::{RunStore, Workflow};
use rupu_tools::PermissionMode;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Every unit here is remote; a local dispatch is a bug. Reports the run's
/// mode the way `DefaultStepFactory` does (`None` = a factory that tracks
/// no single mode).
struct ModeFactory(Option<&'static str>);

#[async_trait]
impl StepFactory for ModeFactory {
    async fn launch_for_step(
        &self,
        _request: rupu_orchestrator::StepRequest,
    ) -> Result<rupu_orchestrator::StepLaunch, rupu_runtime::assembly::AssembleError> {
        panic!("remote units must not be built by the local step factory");
    }

    fn permission_mode(&self) -> Option<&str> {
        self.0
    }
}

/// `(step_id, index, host, mode)` of one dispatch.
type Call = (String, usize, String, Option<PermissionMode>);

/// Records a [`Call`] per dispatch; fails the first dispatch to
/// `fail_first_on` to exercise the fan-out retry.
struct Recorder {
    calls: Mutex<Vec<Call>>,
    fail_first_on: Option<String>,
    failed_once: Mutex<bool>,
}

impl Recorder {
    fn new(fail_first_on: Option<&str>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail_first_on: fail_first_on.map(str::to_string),
            failed_once: Mutex::new(false),
        }
    }

    fn calls(&self) -> Vec<Call> {
        let mut c = self.calls.lock().unwrap().clone();
        c.sort_by(|a, b| (&a.0, a.1, &a.2).cmp(&(&b.0, b.1, &b.2)));
        c
    }
}

#[async_trait]
impl UnitDispatcher for Recorder {
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
        self.calls.lock().unwrap().push((
            unit.step_id.clone(),
            unit.index,
            host.to_string(),
            unit.mode,
        ));
        if self.fail_first_on.as_deref() == Some(host) {
            let mut failed = self.failed_once.lock().unwrap();
            if !*failed {
                *failed = true;
                return Err(RunError::Provider(format!("{host} is unreachable")).into());
            }
        }
        Ok(UnitOutcome {
            output: format!("ok {}", unit.index),
            success: true,
            error: None,
            workspace_delta: None,
            coverage: UnitCoverage::NotLaunched,
        })
    }
}

async fn run(yaml: &str, mode: Option<&'static str>, dispatcher: Arc<Recorder>) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(yaml).expect("workflow must parse"),
        inputs: BTreeMap::new(),
        workspace_id: "ws_remote_mode".into(),
        naming: None,
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(ModeFactory(mode)),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(store),
        workflow_yaml: Some(yaml.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: Some(dispatcher),
        action_dispatcher: None,
        pause: None,
    };
    run_workflow(opts).await.expect("workflow should complete");
}

const PLACED: &str = r#"
name: placed-mode
steps:
  - id: review
    agent: sec
    prompt: p
    host: worker-1
"#;

#[tokio::test]
async fn a_placed_step_carries_the_runs_mode() {
    for (mode, expected) in [
        ("readonly", PermissionMode::Readonly),
        ("bypass", PermissionMode::Bypass),
    ] {
        let d = Arc::new(Recorder::new(None));
        run(PLACED, Some(mode), d.clone()).await;
        assert_eq!(
            d.calls(),
            vec![("review".into(), 0, "worker-1".into(), Some(expected))],
            "mode {mode}"
        );
    }
}

#[tokio::test]
async fn an_ask_run_places_its_units_at_bypass() {
    // A local workflow step runs `ask` unattended — no operator to prompt, so
    // it allows writes (ISSUES.md I-78). A detached `rupu run --mode ask`
    // instead refuses to start without a tty, so the unit gets the mode its
    // local twin actually runs at.
    let d = Arc::new(Recorder::new(None));
    run(PLACED, Some("ask"), d.clone()).await;
    assert_eq!(
        d.calls(),
        vec![(
            "review".into(),
            0,
            "worker-1".into(),
            Some(PermissionMode::Bypass)
        )]
    );
}

#[tokio::test]
async fn every_distributed_unit_and_its_retry_carry_the_mode() {
    let yaml = r#"
name: distributed-mode
steps:
  - id: fan
    agent: service-analyst
    actions: []
    for_each: "a\nb"
    prompt: "check {{ item }}"
    max_parallel: 1
    distribute:
      hosts: [down-host, up-host]
"#;
    let d = Arc::new(Recorder::new(Some("down-host")));
    run(yaml, Some("readonly"), d.clone()).await;
    let calls = d.calls();
    assert!(
        calls.len() >= 3,
        "two units plus one retry expected: {calls:?}"
    );
    assert!(
        calls.iter().all(|c| c.3 == Some(PermissionMode::Readonly)),
        "every dispatch must carry the run's mode: {calls:?}"
    );
}

#[tokio::test]
async fn a_factory_without_a_mode_places_units_with_none() {
    let d = Arc::new(Recorder::new(None));
    run(PLACED, None, d.clone()).await;
    assert_eq!(
        d.calls(),
        vec![("review".into(), 0, "worker-1".into(), None)]
    );
}
