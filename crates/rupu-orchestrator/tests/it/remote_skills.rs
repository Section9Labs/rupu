//! A remote unit (`host:` / `distribute:`) never reaches the step factory,
//! so its step's `skills:` / `preload_skills:` travel on the `UnitDispatch`
//! the runner hands the fleet dispatcher. These drive real workflows through
//! `run_workflow` and assert what each dispatched unit carried.

use async_trait::async_trait;
use rupu_agent::RunError;
use rupu_orchestrator::runner::{
    run_workflow, OrchestratorRunOpts, StepFactory, UnitCoverage, UnitDispatch, UnitDispatcher,
    UnitFailure, UnitOutcome,
};
use rupu_orchestrator::{RunStore, Workflow};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Every unit in these workflows is remote; a local dispatch is a bug.
struct PanicFactory;

#[async_trait]
impl StepFactory for PanicFactory {
    async fn launch_for_step(
        &self,
        _request: rupu_orchestrator::StepRequest,
    ) -> Result<rupu_orchestrator::StepLaunch, rupu_runtime::assembly::AssembleError> {
        panic!("remote units must not be built by the local step factory");
    }
}

/// `(step_id, index, host, skills, preload_skills)` of one dispatch.
type Call = (String, usize, String, Vec<String>, Vec<String>);

/// Records a [`Call`] per dispatch. Fails the first dispatch to any host in
/// `fail_first_on`, to exercise the fan-out retry path.
struct SkillsRecorder {
    calls: Mutex<Vec<Call>>,
    fail_first_on: Option<String>,
    failed_once: Mutex<bool>,
}

impl SkillsRecorder {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail_first_on: None,
            failed_once: Mutex::new(false),
        }
    }

    fn failing_first_on(host: &str) -> Self {
        Self {
            fail_first_on: Some(host.to_string()),
            ..Self::new()
        }
    }

    fn calls(&self) -> Vec<Call> {
        let mut c = self.calls.lock().unwrap().clone();
        c.sort_by(|a, b| (&a.0, a.1, &a.2).cmp(&(&b.0, b.1, &b.2)));
        c
    }
}

#[async_trait]
impl UnitDispatcher for SkillsRecorder {
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
            unit.skills.clone(),
            unit.preload_skills.clone(),
        ));
        if self.fail_first_on.as_deref() == Some(host) {
            let mut failed = self.failed_once.lock().unwrap();
            if !*failed {
                *failed = true;
                return Err(RunError::Provider(format!("{host} refused the launch")).into());
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

async fn run(yaml: &str, dispatcher: Arc<SkillsRecorder>) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(yaml).expect("workflow must parse"),
        inputs: BTreeMap::new(),
        workspace_id: "ws_remote_skills".into(),
        naming: None,
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
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

fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(ToString::to_string).collect()
}

#[tokio::test]
async fn placed_steps_carry_the_steps_skills() {
    let yaml = r#"
name: placed-skills
steps:
  - id: review
    agent: sec
    prompt: p
    host: worker-1
    skills: [pdf, "project:notes"]
    preload_skills: [house-style]
"#;
    let d = Arc::new(SkillsRecorder::new());
    run(yaml, d.clone()).await;
    assert_eq!(
        d.calls(),
        vec![(
            "review".into(),
            0,
            "worker-1".into(),
            v(&["pdf", "project:notes"]),
            v(&["house-style"]),
        )]
    );
}

#[tokio::test]
async fn a_placed_step_without_skills_sends_none() {
    let yaml = r#"
name: placed-no-skills
steps:
  - id: plain
    agent: sec
    prompt: p
    host: worker-1
"#;
    let d = Arc::new(SkillsRecorder::new());
    run(yaml, d.clone()).await;
    assert_eq!(
        d.calls(),
        vec![("plain".into(), 0, "worker-1".into(), vec![], vec![])]
    );
}

#[tokio::test]
async fn every_distributed_unit_carries_the_steps_skills() {
    let yaml = r#"
name: distributed-skills
steps:
  - id: fan
    agent: sec
    actions: []
    for_each: "a\nb\nc"
    prompt: "check {{ item }}"
    max_parallel: 3
    distribute:
      hosts: [h1, h2, h3]
    skills: [pdf]
    preload_skills: [house-style]
"#;
    let d = Arc::new(SkillsRecorder::new());
    run(yaml, d.clone()).await;
    assert_eq!(
        d.calls(),
        vec![
            (
                "fan".into(),
                0,
                "h1".into(),
                v(&["pdf"]),
                v(&["house-style"])
            ),
            (
                "fan".into(),
                1,
                "h2".into(),
                v(&["pdf"]),
                v(&["house-style"])
            ),
            (
                "fan".into(),
                2,
                "h3".into(),
                v(&["pdf"]),
                v(&["house-style"])
            ),
        ]
    );
}

#[tokio::test]
async fn a_distributed_units_retry_on_the_fallback_host_keeps_the_skills() {
    // A connector whose peer can't take the skills refuses the launch; the
    // unit is retried on the next host, and that retry carries the same
    // skills rather than running with the agent's alone.
    let yaml = r#"
name: distributed-retry
steps:
  - id: fan
    agent: sec
    actions: []
    for_each: "only"
    prompt: "check {{ item }}"
    distribute:
      hosts: [old-host, new-host]
    skills: [pdf]
    preload_skills: [house-style]
"#;
    let d = Arc::new(SkillsRecorder::failing_first_on("old-host"));
    run(yaml, d.clone()).await;
    assert_eq!(
        d.calls(),
        vec![
            (
                "fan".into(),
                0,
                "new-host".into(),
                v(&["pdf"]),
                v(&["house-style"])
            ),
            (
                "fan".into(),
                0,
                "old-host".into(),
                v(&["pdf"]),
                v(&["house-style"])
            ),
        ]
    );
}
