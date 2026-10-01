//! Engagement profiles at the workflow level (engagement-profiles Plan 2,
//! Task 9).
//!
//! * A remote unit (`host:` / `distribute:`) never reaches the step factory
//!   and `UnitDispatch` carries no engagement, so a non-`code` selection
//!   cannot be delivered to it. The runner REFUSES it (fail-closed) instead of
//!   dispatching a unit that would silently run on the native `code` path.
//! * An `action: findings.record` step's engagement is resolved through the
//!   factory and failures surface as a failed step, never a silent downgrade.

use async_trait::async_trait;
use rupu_agent::{AgentRunOpts, RunError};
use rupu_mcp::{McpPermission, ToolDispatcher};
use rupu_orchestrator::runner::{
    run_workflow, OrchestratorRunOpts, StepFactory, UnitDispatch, UnitDispatcher, UnitOutcome,
};
use rupu_orchestrator::{RunStore, RunWorkflowError, Step, Workflow};
use rupu_scm::Registry;
use rupu_tools::PermissionMode;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Every unit in the remote workflows is remote; a local dispatch is a bug.
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
        panic!("this workflow has no local agent step");
    }
}

/// A factory with a scripted answer for an action step's engagement.
struct ActionEngagementFactory {
    answer: Result<Option<Arc<rupu_coverage::profile::ActiveSet>>, String>,
}

#[async_trait]
impl StepFactory for ActionEngagementFactory {
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
        panic!("this workflow has no agent step");
    }

    fn action_engagement(
        &self,
        _step: &Step,
    ) -> Result<Option<Arc<rupu_coverage::profile::ActiveSet>>, String> {
        self.answer.clone()
    }
}

/// `(step_id, index, host)` of every unit that actually reached the dispatcher.
#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<(String, usize, String)>>,
}

impl Recorder {
    fn calls(&self) -> Vec<(String, usize, String)> {
        let mut c = self.calls.lock().unwrap().clone();
        c.sort();
        c
    }
}

#[async_trait]
impl UnitDispatcher for Recorder {
    async fn dispatch_unit(&self, unit: UnitDispatch, host: &str) -> Result<UnitOutcome, RunError> {
        self.calls
            .lock()
            .unwrap()
            .push((unit.step_id.clone(), unit.index, host.to_string()));
        Ok(UnitOutcome {
            output: format!("ok {}", unit.index),
            success: true,
            error: None,
            workspace_delta: None,
        })
    }
}

fn opts(
    yaml: &str,
    factory: Arc<dyn StepFactory>,
    unit_dispatcher: Option<Arc<dyn UnitDispatcher>>,
    action_dispatcher: Option<Arc<ToolDispatcher>>,
) -> (OrchestratorRunOpts, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(yaml).expect("workflow must parse"),
        inputs: BTreeMap::new(),
        workspace_id: "ws_engagement".into(),
        naming: None,
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory,
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(store),
        workflow_yaml: Some(yaml.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher,
        action_dispatcher,
        pause: None,
    };
    (opts, tmp)
}

fn remote_run(yaml: &str, d: &Arc<Recorder>) -> (OrchestratorRunOpts, tempfile::TempDir) {
    opts(
        yaml,
        Arc::new(PanicFactory),
        Some(d.clone() as Arc<dyn UnitDispatcher>),
        None,
    )
}

// ── remote guard ────────────────────────────────────────────────────────

#[tokio::test]
async fn a_placed_step_under_a_workflow_engagement_is_refused() {
    let yaml = r#"
name: placed-engagement
defaults:
  engagement_profiles: [binary]
steps:
  - id: placed
    agent: sec
    prompt: p
    host: worker-1
"#;
    let d = Arc::new(Recorder::default());
    let (o, _tmp) = remote_run(yaml, &d);
    let err = run_workflow(o)
        .await
        .expect_err("a non-default engagement cannot be delivered to a remote host");
    let msg = err.to_string();
    assert!(msg.contains("engagement"), "{msg}");
    assert!(msg.contains("binary"), "names the selection: {msg}");
    assert!(msg.contains("placed"), "names the step: {msg}");
    assert!(d.calls().is_empty(), "nothing may be dispatched: {msg}");
}

#[tokio::test]
async fn a_placed_step_with_its_own_engagement_is_refused_too() {
    let yaml = r#"
name: placed-step-engagement
steps:
  - id: placed
    agent: sec
    prompt: p
    host: worker-1
    engagement_profiles: [binary]
"#;
    let d = Arc::new(Recorder::default());
    let (o, _tmp) = remote_run(yaml, &d);
    let err = run_workflow(o).await.expect_err("refused");
    assert!(err.to_string().contains("engagement"), "{err}");
    assert!(d.calls().is_empty());
}

#[tokio::test]
async fn a_refused_placed_step_honors_continue_on_error() {
    let yaml = r#"
name: placed-continue
defaults:
  engagement_profiles: [binary]
steps:
  - id: placed
    agent: sec
    prompt: p
    host: worker-1
    continue_on_error: true
  - id: after
    agent: sec
    prompt: p
    host: worker-1
    engagement_profiles: [code]
"#;
    let d = Arc::new(Recorder::default());
    let (o, _tmp) = remote_run(yaml, &d);
    let res = run_workflow(o)
        .await
        .expect("continue_on_error tolerates it");
    assert!(
        !res.step_results[0].success,
        "the refused step is a failure"
    );
    assert!(
        res.step_results[0].output.contains("engagement"),
        "{}",
        res.step_results[0].output
    );
    // The step that narrowed back to `code` ran and was dispatched.
    assert!(res.step_results[1].success);
    assert_eq!(d.calls(), vec![("after".into(), 0, "worker-1".into())]);
}

#[tokio::test]
async fn distributed_units_under_an_engagement_are_all_refused() {
    let yaml = r#"
name: distributed-engagement
defaults:
  engagement_profiles: [binary]
steps:
  - id: fan
    agent: sec
    actions: []
    for_each: "a\nb\nc"
    prompt: "check {{ item }}"
    distribute:
      hosts: [h1, h2]
"#;
    let d = Arc::new(Recorder::default());
    let (o, _tmp) = remote_run(yaml, &d);
    let err = run_workflow(o).await.expect_err("refused");
    let msg = err.to_string();
    assert!(
        msg.contains("engagement") && msg.contains("binary"),
        "{msg}"
    );
    assert!(
        d.calls().is_empty(),
        "no unit may reach a host, retry included: {msg}"
    );
}

#[tokio::test]
async fn code_or_no_engagement_dispatches_remote_units_unchanged() {
    let yaml = r#"
name: placed-native
defaults:
  engagement_profiles: [binary]
steps:
  - id: narrowed
    agent: sec
    prompt: p
    host: worker-1
    engagement_profiles: [code]
  - id: fan
    agent: sec
    actions: []
    for_each: "a\nb"
    prompt: "check {{ item }}"
    distribute:
      hosts: [h1]
    engagement_profiles: [code]
"#;
    let d = Arc::new(Recorder::default());
    let (o, _tmp) = remote_run(yaml, &d);
    run_workflow(o)
        .await
        .expect("a step narrowed to `code` is the default and runs remotely");
    assert_eq!(
        d.calls(),
        vec![
            ("fan".into(), 0, "h1".into()),
            ("fan".into(), 1, "h1".into()),
            ("narrowed".into(), 0, "worker-1".into()),
        ]
    );

    let plain = r#"
name: placed-plain
steps:
  - id: placed
    agent: sec
    prompt: p
    host: worker-1
"#;
    let d = Arc::new(Recorder::default());
    let (o, _tmp) = remote_run(plain, &d);
    run_workflow(o).await.expect("no engagement at all");
    assert_eq!(d.calls(), vec![("placed".into(), 0, "worker-1".into())]);
}

// ── action steps ────────────────────────────────────────────────────────

const WF_ACTION: &str = r#"
name: action-engagement
defaults:
  engagement_profiles: [binary]
steps:
  - id: record
    action: findings.record
    findings_profile: summary
    with:
      scope: host
      target_ref: "gateway.internal.example"
      summary: "Admin console reachable without authentication"
      severity: high
      rationale: "GET /admin returned 200 with no session."
"#;

fn action_dispatcher(workspace: &std::path::Path) -> Arc<ToolDispatcher> {
    Arc::new(
        ToolDispatcher::new(
            Arc::new(Registry::empty()),
            McpPermission::new(PermissionMode::Bypass, vec!["*".into()]),
        )
        .with_findings(rupu_mcp::FindingsContext {
            workspace_path: workspace.to_path_buf(),
            scope_name: "action-engagement".into(),
            run_id: "run_action_engagement".into(),
            model: "mock-1".into(),
            surface: rupu_coverage::Surface::Workflow,
            options: rupu_coverage::FindingWriteOptions::default(),
            codename: None,
            provider: None,
        }),
    )
}

#[tokio::test]
async fn an_action_steps_unresolvable_engagement_fails_the_step() {
    let tmp_ws = tempfile::tempdir().unwrap();
    let factory = Arc::new(ActionEngagementFactory {
        answer: Err("step `record`: engagement_profiles [web] widens the run set".into()),
    });
    let (o, _tmp) = opts(
        WF_ACTION,
        factory,
        None,
        Some(action_dispatcher(tmp_ws.path())),
    );
    let err = run_workflow(o)
        .await
        .expect_err("an unresolvable engagement must not record a finding");
    assert!(
        matches!(err, RunWorkflowError::Action { .. }),
        "surfaces as a failed action step: {err:?}"
    );
    assert!(err.to_string().contains("widens"), "{err}");
    // Nothing was recorded.
    let paths = rupu_coverage::CoveragePaths::new(
        tmp_ws.path(),
        &rupu_coverage::target_id(tmp_ws.path(), "action-engagement"),
    );
    assert!(rupu_coverage::read_findings(&paths).unwrap().is_empty());
}

#[tokio::test]
async fn an_action_step_with_no_engagement_records_as_before() {
    let tmp_ws = tempfile::tempdir().unwrap();
    let factory = Arc::new(ActionEngagementFactory { answer: Ok(None) });
    let (o, _tmp) = opts(
        WF_ACTION,
        factory,
        None,
        Some(action_dispatcher(tmp_ws.path())),
    );
    let res = run_workflow(o).await.expect("native path records");
    assert!(
        res.step_results[0].success,
        "{}",
        res.step_results[0].output
    );
    assert!(res.step_results[0].output.starts_with("finding_id: fnd_"));
}

#[tokio::test]
async fn an_action_step_under_a_real_engagement_is_not_recorded_on_the_native_path() {
    // The `findings.record` dispatcher cannot yet carry a per-step engagement
    // set (MCP-side delivery is the next task in this plan). Until it does,
    // recording under a non-default engagement must FAIL rather than quietly
    // file the finding under the native `code` rules.
    let set = rupu_coverage::profile::builtin_registry()
        .unwrap()
        .active_set(&["binary".to_string()])
        .unwrap();
    let tmp_ws = tempfile::tempdir().unwrap();
    let factory = Arc::new(ActionEngagementFactory {
        answer: Ok(Some(Arc::new(set))),
    });
    let (o, _tmp) = opts(
        WF_ACTION,
        factory,
        None,
        Some(action_dispatcher(tmp_ws.path())),
    );
    let err = run_workflow(o).await.expect_err("refused, not downgraded");
    assert!(err.to_string().contains("engagement"), "{err}");
    let paths = rupu_coverage::CoveragePaths::new(
        tmp_ws.path(),
        &rupu_coverage::target_id(tmp_ws.path(), "action-engagement"),
    );
    assert!(rupu_coverage::read_findings(&paths).unwrap().is_empty());
}
