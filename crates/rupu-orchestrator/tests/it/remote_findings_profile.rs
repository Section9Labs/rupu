//! A remote unit (`host:` / `distribute:`) never reaches the step factory,
//! so its findings profile has to travel on the `UnitDispatch` the runner
//! hands the fleet dispatcher. These drive real workflows through
//! `run_workflow` and assert what each dispatched unit carried:
//! step `findings_profile` → workflow `defaults.findings_profile` → `None`
//! (the host resolves the agent's `findingsProfile` from its own agent file).

use async_trait::async_trait;
use rupu_agent::{AgentRunOpts, RunError};
use rupu_coverage::FindingProfile::{self, Full, Summary};
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
}

/// `(step_id, index, host, findings_profile)` of one dispatch.
type Call = (String, usize, String, Option<FindingProfile>);

/// Records a [`Call`] per dispatch. Fails the first dispatch to any host in
/// `fail_first_on`, to exercise the fan-out retry path.
struct ProfileRecorder {
    calls: Mutex<Vec<Call>>,
    fail_first_on: Option<String>,
    failed_once: Mutex<bool>,
}

impl ProfileRecorder {
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
impl UnitDispatcher for ProfileRecorder {
    async fn dispatch_unit(
        &self,
        unit: UnitDispatch,
        host: &str,
    ) -> Result<UnitOutcome, UnitFailure> {
        self.calls.lock().unwrap().push((
            unit.step_id.clone(),
            unit.index,
            host.to_string(),
            unit.findings_profile,
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

async fn run(yaml: &str, dispatcher: Arc<ProfileRecorder>) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(yaml).expect("workflow must parse"),
        inputs: BTreeMap::new(),
        workspace_id: "ws_remote_profile".into(),
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
        action_dispatcher: None,
        pause: None,
    };
    run_workflow(opts).await.expect("workflow should complete");
}

#[tokio::test]
async fn placed_steps_carry_the_step_then_default_profile() {
    let yaml = r#"
name: placed-profiles
defaults:
  findings_profile: summary
steps:
  - id: inherits
    agent: sec
    prompt: p
    host: worker-1
  - id: overrides
    agent: sec
    prompt: p
    host: worker-2
    findings_profile: full
"#;
    let d = Arc::new(ProfileRecorder::new());
    run(yaml, d.clone()).await;
    assert_eq!(
        d.calls(),
        vec![
            ("inherits".into(), 0, "worker-1".into(), Some(Summary)),
            ("overrides".into(), 0, "worker-2".into(), Some(Full)),
        ]
    );
}

#[tokio::test]
async fn a_placed_step_with_no_profile_leaves_it_to_the_host() {
    // Neither the step nor the workflow sets one, so the coordinator must NOT
    // resolve to the built-in `full` — that would override the agent's
    // `findingsProfile` on the host, which only the host can read.
    let yaml = r#"
name: placed-no-profile
steps:
  - id: agent_decides
    agent: sec
    prompt: p
    host: worker-1
"#;
    let d = Arc::new(ProfileRecorder::new());
    run(yaml, d.clone()).await;
    assert_eq!(
        d.calls(),
        vec![("agent_decides".into(), 0, "worker-1".into(), None)]
    );
}

#[tokio::test]
async fn every_distributed_unit_carries_the_steps_profile() {
    let yaml = r#"
name: distributed-profiles
defaults:
  findings_profile: full
steps:
  - id: fan
    agent: sec
    actions: []
    for_each: "a\nb\nc"
    prompt: "check {{ item }}"
    max_parallel: 3
    distribute:
      hosts: [h1, h2]
    findings_profile: summary
"#;
    let d = Arc::new(ProfileRecorder::new());
    run(yaml, d.clone()).await;
    assert_eq!(
        d.calls(),
        vec![
            ("fan".into(), 0, "h1".into(), Some(Summary)),
            ("fan".into(), 1, "h2".into(), Some(Summary)),
            ("fan".into(), 2, "h1".into(), Some(Summary)),
        ]
    );
}

#[tokio::test]
async fn a_distributed_units_retry_on_the_fallback_host_keeps_the_profile() {
    // A connector that can't honour the profile refuses the launch; the unit
    // is then retried on the next host, and that retry must carry the same
    // profile rather than fall back to the host's agent file.
    let yaml = r#"
name: distributed-retry
defaults:
  findings_profile: summary
steps:
  - id: fan
    agent: sec
    actions: []
    for_each: "only"
    prompt: "check {{ item }}"
    distribute:
      hosts: [old-host, new-host]
"#;
    let d = Arc::new(ProfileRecorder::failing_first_on("old-host"));
    run(yaml, d.clone()).await;
    assert_eq!(
        d.calls(),
        vec![
            ("fan".into(), 0, "new-host".into(), Some(Summary)),
            ("fan".into(), 0, "old-host".into(), Some(Summary)),
        ]
    );
}
