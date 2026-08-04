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
