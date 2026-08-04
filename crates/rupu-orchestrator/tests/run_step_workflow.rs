//! `run:` step runtime (Bench Plan 0): deterministic, non-LLM command
//! steps execute, bind their output downstream, and are gated by
//! permission mode + workspace config.
//!
//! Harness shape mirrors `tests/action_step.rs` — a real disk-backed
//! `RunStore` driving `run_workflow` through its public
//! `OrchestratorRunOpts`. No agent or provider is involved: a workflow
//! made only of `run:` steps never calls a model, which is the point.

use async_trait::async_trait;
use rupu_agent::AgentRunOpts;
use rupu_config::policy_config::WorkflowConfig;
use rupu_orchestrator::runner::{
    run_workflow, OrchestratorRunOpts, OrchestratorRunResult, RunStepPolicy, StepFactory,
};
use rupu_orchestrator::{RunStore, StepKind, Workflow};
use rupu_tools::PermissionMode;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// A factory that panics if used. A `run:` workflow must never reach for
/// an agent — if it does, that is the bug this catches.
struct NoAgentFactory;

#[async_trait]
impl StepFactory for NoAgentFactory {
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
        panic!("a run: step must never dispatch an agent");
    }
}

fn policy(mode: PermissionMode, enabled: bool, allowlist: Vec<String>) -> RunStepPolicy {
    RunStepPolicy {
        mode,
        config: WorkflowConfig {
            run_step_enabled: enabled,
            run_step_allowlist: allowlist,
        },
        workspace_root: std::env::temp_dir(),
    }
}

async fn run_with(
    yaml: &str,
    label: &str,
    policy: RunStepPolicy,
) -> Result<OrchestratorRunResult, rupu_orchestrator::runner::RunWorkflowError> {
    let wf = Workflow::parse(yaml).expect("workflow parses");
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));

    run_workflow(OrchestratorRunOpts {
        run_step: policy,
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: label.into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(NoAgentFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(store),
        workflow_yaml: Some(yaml.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    })
    .await
}

fn bypass() -> RunStepPolicy {
    policy(PermissionMode::Bypass, true, vec![])
}

#[tokio::test]
async fn run_step_executes_and_records_its_outcome() {
    let yaml = r#"
name: bench-smoke
steps:
  - id: probe
    run:
      cmd: echo
      args: ["hello"]
"#;
    let res = run_with(yaml, "ws_run_basic", bypass())
        .await
        .expect("run completes");

    let probe = &res.step_results[0];
    assert_eq!(probe.step_id, "probe");
    assert_eq!(probe.kind, StepKind::Run);
    assert!(probe.success);
    assert_eq!(probe.output.trim(), "hello");

    let outcome = probe.run_outcome.as_ref().expect("run outcome recorded");
    assert_eq!(outcome.exit_code, 0);
    assert_eq!(outcome.stdout.trim(), "hello");
}

#[tokio::test]
async fn run_step_binds_parsed_json_to_the_next_step() {
    // The benchmark shape: a scorer emits JSON, a later step reads a field.
    let yaml = r#"
name: bench-smoke
steps:
  - id: score
    run:
      cmd: echo
      args: ['{"score": 91}']
      parse: json
  - id: gate
    run:
      cmd: test
      args: ["91", "-eq", "{{ steps.score.json.score }}"]
"#;
    let res = run_with(yaml, "ws_run_json", bypass())
        .await
        .expect("run completes");

    assert!(res.step_results[0].success);
    assert!(
        res.step_results[1].success,
        "steps.score.json.score must render as 91"
    );
}

#[tokio::test]
async fn exit_code_and_stdout_are_referencable_downstream() {
    let yaml = r#"
name: bench-smoke
steps:
  - id: probe
    run:
      cmd: echo
      args: ["sentinel"]
  - id: check
    run:
      cmd: test
      args: ["0", "-eq", "{{ steps.probe.exit_code }}"]
"#;
    let res = run_with(yaml, "ws_run_fields", bypass())
        .await
        .expect("run completes");
    assert!(
        res.step_results[1].success,
        "steps.probe.exit_code must bind"
    );
}

#[tokio::test]
async fn failing_run_step_fails_the_run() {
    let yaml = r#"
name: bench-smoke
steps:
  - id: boom
    run: { cmd: "false" }
"#;
    let err = run_with(yaml, "ws_run_fail", bypass())
        .await
        .expect_err("a disallowed exit code must fail the run");
    let msg = format!("{err}");
    assert!(msg.contains("boom"), "error names the step: {msg}");
}

#[tokio::test]
async fn continue_on_error_records_the_failure_and_proceeds() {
    let yaml = r#"
name: bench-smoke
steps:
  - id: boom
    continue_on_error: true
    run: { cmd: "false" }
  - id: after
    run: { cmd: echo, args: ["still ran"] }
"#;
    let res = run_with(yaml, "ws_run_coe", bypass())
        .await
        .expect("run completes");
    assert!(!res.step_results[0].success, "the failure is recorded");
    assert!(res.step_results[1].success, "later steps still run");
}

#[tokio::test]
async fn readonly_mode_refuses_a_run_step() {
    let yaml = r#"
name: bench-smoke
steps:
  - id: probe
    run: { cmd: echo, args: ["hi"] }
"#;
    let err = run_with(
        yaml,
        "ws_run_readonly",
        policy(PermissionMode::Readonly, true, vec![]),
    )
    .await
    .expect_err("readonly must refuse");
    assert!(format!("{err}").contains("refused"));
}

#[tokio::test]
async fn disabled_config_refuses_even_under_bypass() {
    // The workspace opt-in is not overridable by --mode bypass.
    let yaml = r#"
name: bench-smoke
steps:
  - id: probe
    run: { cmd: echo, args: ["hi"] }
"#;
    let err = run_with(
        yaml,
        "ws_run_disabled",
        policy(PermissionMode::Bypass, false, vec![]),
    )
    .await
    .expect_err("a workspace that never opted in must refuse");
    let msg = format!("{err}");
    assert!(
        msg.contains("run_step_enabled"),
        "the error names the remedy: {msg}"
    );
}

#[tokio::test]
async fn non_allowlisted_executable_is_refused() {
    let yaml = r#"
name: bench-smoke
steps:
  - id: probe
    run: { cmd: echo, args: ["hi"] }
"#;
    let err = run_with(
        yaml,
        "ws_run_allowlist",
        policy(PermissionMode::Bypass, true, vec!["python3".into()]),
    )
    .await
    .expect_err("echo is not allowlisted");
    assert!(format!("{err}").contains("allowlist"));
}

#[tokio::test]
async fn a_template_rendered_arg_is_never_shell_interpreted() {
    // End-to-end version of the executor's unit test: a workflow INPUT
    // carrying shell metacharacters must reach the process as literal
    // argv text, never as a command.
    let marker = std::env::temp_dir().join("rupu_wf_pwned_marker");
    let _ = std::fs::remove_file(&marker);

    let yaml = format!(
        r#"
name: bench-smoke
steps:
  - id: echo_evil
    run:
      cmd: echo
      args: ["; touch {}"]
"#,
        marker.display()
    );

    let res = run_with(&yaml, "ws_run_injection", bypass())
        .await
        .expect("run completes");

    assert!(res.step_results[0].success);
    assert!(
        !marker.exists(),
        "a templated arg must not be shell-interpreted"
    );
}

// ---------------------------------------------------------------------------
// `for_each:` + `run:` fan-out — the shape both benchmarks depend on.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn for_each_run_fans_out_per_item_in_declared_order() {
    let yaml = r#"
name: bench-fanout
steps:
  - id: score
    for_each: '["alpha", "beta", "gamma"]'
    max_parallel: 3
    run:
      cmd: echo
      args: ["{{ item }}"]
"#;
    let res = run_with(yaml, "ws_fanout_basic", bypass())
        .await
        .expect("run completes");

    let step = &res.step_results[0];
    assert_eq!(
        step.kind,
        StepKind::Run,
        "a for_each run: step is a Run node"
    );
    assert_eq!(step.items.len(), 3);
    // Declared order regardless of finish order under max_parallel: 3.
    assert_eq!(step.items[0].output.trim(), "alpha");
    assert_eq!(step.items[1].output.trim(), "beta");
    assert_eq!(step.items[2].output.trim(), "gamma");
    assert!(step.items.iter().all(|i| i.success));
    assert!(step.success);
}

#[tokio::test]
async fn for_each_run_continue_on_error_records_failures_and_proceeds() {
    // A benchmark must not lose 199 results because unit 2 exited nonzero.
    // `sh -c` here is an EXPLICITLY authored shell invocation, which is
    // allowed — what the executor must never do is wrap an author's
    // cmd/args in a shell implicitly.
    let yaml = r#"
name: bench-fanout
steps:
  - id: score
    for_each: '["0", "1", "0"]'
    max_parallel: 1
    continue_on_error: true
    run:
      cmd: sh
      args: ["-c", "exit {{ item }}"]
"#;
    let res = run_with(yaml, "ws_fanout_coe", bypass())
        .await
        .expect("run completes");

    let step = &res.step_results[0];
    assert_eq!(step.items.len(), 3, "every unit is recorded, none dropped");
    assert!(step.items[0].success);
    assert!(!step.items[1].success, "the failing unit is recorded");
    assert!(step.items[2].success, "later units still dispatch");
    assert!(!step.success, "the step as a whole reports failure");
}

#[tokio::test]
async fn for_each_run_without_continue_on_error_fails_the_run() {
    let yaml = r#"
name: bench-fanout
steps:
  - id: score
    for_each: '["0", "1"]'
    run:
      cmd: sh
      args: ["-c", "exit {{ item }}"]
"#;
    let err = run_with(yaml, "ws_fanout_strict", bypass())
        .await
        .expect_err("a failing unit must fail the run by default");
    let msg = format!("{err}");
    assert!(msg.contains("1 of 2 units failed"), "got: {msg}");
}

#[tokio::test]
async fn for_each_run_binds_object_item_fields() {
    // The real benchmark shape: jobs.json is a list of objects.
    let yaml = r#"
name: bench-fanout
steps:
  - id: score
    for_each: '[{"id": "CB-001"}, {"id": "CB-002"}]'
    max_parallel: 2
    run:
      cmd: echo
      args: ["{{ item.id }}"]
"#;
    let res = run_with(yaml, "ws_fanout_objects", bypass())
        .await
        .expect("run completes");
    let step = &res.step_results[0];
    assert_eq!(step.items[0].output.trim(), "CB-001");
    assert_eq!(step.items[1].output.trim(), "CB-002");
}

#[tokio::test]
async fn empty_for_each_run_is_success_with_no_units() {
    let yaml = r#"
name: bench-fanout
steps:
  - id: score
    for_each: '[]'
    run: { cmd: echo, args: ["never"] }
"#;
    let res = run_with(yaml, "ws_fanout_empty", bypass())
        .await
        .expect("run completes");
    assert!(res.step_results[0].items.is_empty());
    assert!(res.step_results[0].success);
}

#[tokio::test]
async fn fanout_units_are_gated_too() {
    // The gate must apply per unit, not only to linear steps — otherwise
    // fan-out is a hole straight through the permission model.
    let yaml = r#"
name: bench-fanout
steps:
  - id: score
    for_each: '["a", "b"]'
    continue_on_error: true
    run: { cmd: echo, args: ["{{ item }}"] }
"#;
    let res = run_with(
        yaml,
        "ws_fanout_gated",
        policy(PermissionMode::Bypass, true, vec!["python3".into()]),
    )
    .await
    .expect("continue_on_error tolerates the refusals");

    let step = &res.step_results[0];
    assert_eq!(step.items.len(), 2);
    assert!(
        step.items.iter().all(|i| !i.success),
        "every unit must be refused; echo is not allowlisted"
    );
    assert!(
        step.items[0].output.contains("allowlist"),
        "the refusal reason is recorded: {}",
        step.items[0].output
    );
}
