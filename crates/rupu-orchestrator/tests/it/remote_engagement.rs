//! A remote unit (`host:` / `distribute:`) never reaches the step factory, so
//! the run's engagement (`rupu workflow run --engagement-profile`) has to
//! travel on the `UnitDispatch` the runner hands the fleet dispatcher — or
//! the host runs the unit on the `code` path. These drive real workflows
//! through `run_workflow` and assert what each dispatched unit carried.

use async_trait::async_trait;
use rupu_agent::{AgentRunOpts, RunError};
use rupu_orchestrator::runner::{
    run_workflow, OrchestratorRunOpts, StepFactory, UnitCoverage, UnitDispatch, UnitDispatcher,
    UnitFailure, UnitOutcome,
};
use rupu_orchestrator::{RunStore, Workflow};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Every unit here is remote; a local dispatch is a bug. Reports the run's
/// engagement the way `DefaultStepFactory` does.
struct EngagedFactory(Vec<String>);

#[async_trait]
impl StepFactory for EngagedFactory {
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
        panic!("remote units must not be built by the local step factory");
    }

    fn engagement_profiles(&self) -> Vec<String> {
        self.0.clone()
    }
}

/// `(step_id, index, host, engagement_profiles)` of one dispatch.
type Call = (String, usize, String, Vec<String>);

/// Records a [`Call`] per dispatch; fails the first dispatch to
/// `fail_first_on` (a host that refuses the engagement) to exercise the
/// fan-out retry.
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
            unit.engagement_profiles.clone(),
        ));
        if self.fail_first_on.as_deref() == Some(host) {
            let mut failed = self.failed_once.lock().unwrap();
            if !*failed {
                *failed = true;
                return Err(RunError::Provider(format!(
                    "{host} does not support engagement profiles"
                ))
                .into());
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

async fn run(yaml: &str, engagement: &[&str], dispatcher: Arc<Recorder>) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(yaml).expect("workflow must parse"),
        inputs: BTreeMap::new(),
        workspace_id: "ws_remote_engagement".into(),
        naming: None,
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(EngagedFactory(
            engagement.iter().map(|s| s.to_string()).collect(),
        )),
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

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn a_placed_step_carries_the_runs_engagement() {
    let yaml = r#"
name: placed-engagement
steps:
  - id: recon
    agent: recon
    prompt: p
    host: worker-1
"#;
    let d = Arc::new(Recorder::new(None));
    run(yaml, &["network", "web"], d.clone()).await;
    assert_eq!(
        d.calls(),
        vec![(
            "recon".into(),
            0,
            "worker-1".into(),
            ids(&["network", "web"])
        )]
    );
}

#[tokio::test]
async fn every_distributed_unit_and_its_retry_carry_the_engagement() {
    // The first host refuses (an older peer); the unit's retry on the next
    // host must carry the same engagement, not fall back to `code`.
    let yaml = r#"
name: distributed-engagement
steps:
  - id: fan
    agent: service-analyst
    actions: []
    for_each: "a\nb"
    prompt: "check {{ item }}"
    max_parallel: 1
    distribute:
      hosts: [old-host, new-host]
"#;
    let d = Arc::new(Recorder::new(Some("old-host")));
    run(yaml, &["network"], d.clone()).await;
    let calls = d.calls();
    assert!(
        calls.len() >= 3,
        "two units plus one retry expected: {calls:?}"
    );
    assert!(
        calls.iter().all(|c| c.3 == ids(&["network"])),
        "every dispatch must carry the engagement: {calls:?}"
    );
}

#[tokio::test]
async fn without_an_engagement_a_placed_unit_carries_none() {
    let yaml = r#"
name: placed-code
steps:
  - id: review
    agent: sec
    prompt: p
    host: worker-1
"#;
    let d = Arc::new(Recorder::new(None));
    run(yaml, &[], d.clone()).await;
    assert_eq!(
        d.calls(),
        vec![("review".into(), 0, "worker-1".into(), Vec::new())]
    );
}
