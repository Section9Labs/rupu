//! A remote unit (`host:` / `distribute:`) runs as a standalone `rupu run` on
//! its host, so a workflow's `messaging: false` (or the cap the run itself
//! was launched under) has to travel on the `UnitDispatch` the runner hands
//! the fleet dispatcher, or the host gives the unit the default messaging
//! tools. These drive real workflows through `run_workflow` and assert what
//! each dispatched unit carried.

use async_trait::async_trait;
use rupu_agent::RunError;
use rupu_orchestrator::runner::{
    run_workflow, OrchestratorRunOpts, StepFactory, UnitCoverage, UnitDispatch, UnitDispatcher,
    UnitFailure, UnitOutcome,
};
use rupu_orchestrator::{RunStore, Workflow};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Every unit here is remote; a local dispatch is a bug. `capped` is the
/// run's own launch cap, the way `DefaultStepFactory` reports its link's.
struct CappedFactory {
    capped: bool,
}

#[async_trait]
impl StepFactory for CappedFactory {
    async fn launch_for_step(
        &self,
        _request: rupu_orchestrator::StepRequest,
    ) -> Result<rupu_orchestrator::StepLaunch, rupu_runtime::assembly::AssembleError> {
        panic!("remote units must not be built by the local step factory");
    }

    fn messaging_capped(&self) -> bool {
        self.capped
    }
}

/// `(step_id, index, host, messaging_off)` of one dispatch.
type Call = (String, usize, String, bool);

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
            unit.messaging_off,
        ));
        if self.fail_first_on.as_deref() == Some(host) {
            let mut failed = self.failed_once.lock().unwrap();
            if !*failed {
                *failed = true;
                return Err(RunError::Provider(format!("{host} is unreachable")).into());
            }
        }
        Ok(UnitOutcome {
            warnings: Vec::new(),
            output: format!("ok {}", unit.index),
            success: true,
            error: None,
            workspace_delta: None,
            coverage: UnitCoverage::NotLaunched,
        })
    }
}

async fn run(yaml: &str, capped: bool, dispatcher: Arc<Recorder>) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(yaml).expect("workflow must parse"),
        inputs: BTreeMap::new(),
        workspace_id: "ws_remote_messaging".into(),
        naming: None,
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(CappedFactory { capped }),
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
        action_services: None,
        pause: None,
    };
    run_workflow(opts).await.expect("workflow should complete");
}

fn placed(messaging: &str) -> String {
    format!(
        r#"
name: placed-messaging
{messaging}
steps:
  - id: review
    agent: sec
    prompt: p
    host: worker-1
"#
    )
}

#[tokio::test]
async fn a_placed_unit_of_an_opted_out_workflow_has_messaging_off() {
    let d = Arc::new(Recorder::new(None));
    run(&placed("messaging: false"), false, d.clone()).await;
    assert_eq!(
        d.calls(),
        vec![("review".into(), 0, "worker-1".into(), true)]
    );
}

#[tokio::test]
async fn a_placed_unit_of_a_default_workflow_keeps_its_messaging() {
    for messaging in ["", "messaging: true", "messaging:\n  tools: [msg.send]"] {
        let d = Arc::new(Recorder::new(None));
        run(&placed(messaging), false, d.clone()).await;
        assert_eq!(
            d.calls(),
            vec![("review".into(), 0, "worker-1".into(), false)],
            "{messaging:?}"
        );
    }
}

#[tokio::test]
async fn a_capped_run_places_its_units_with_messaging_off() {
    let d = Arc::new(Recorder::new(None));
    run(&placed(""), true, d.clone()).await;
    assert_eq!(
        d.calls(),
        vec![("review".into(), 0, "worker-1".into(), true)]
    );
}

#[tokio::test]
async fn every_distributed_unit_and_its_retry_carry_messaging_off() {
    let yaml = r#"
name: distributed-messaging
messaging: false
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
    run(yaml, false, d.clone()).await;
    let calls = d.calls();
    assert_eq!(
        calls.len(),
        3,
        "two units plus one retry expected: {calls:?}"
    );
    assert!(
        calls.iter().all(|c| c.3),
        "every dispatch must carry messaging off: {calls:?}"
    );
}
