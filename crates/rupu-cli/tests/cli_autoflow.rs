//! End-to-end and direct runtime tests for `rupu autoflow ...`.
//!
//! These tests mutate process-global state (`RUPU_HOME`,
//! `RUPU_MOCK_PROVIDER_SCRIPT`, cwd). Hold `ENV_LOCK` for the full
//! body.

use assert_fs::prelude::*;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

const MOCK_SCRIPT: &str = r#"
[
  { "AssistantText": { "text": "autoflow output", "stop": "end_turn" } }
]
"#;

fn write_agent_and_workflow(
    project: &assert_fs::TempDir,
    workflow_name: &str,
    workflow_yaml: &str,
) {
    project.child(".rupu/agents").create_dir_all().unwrap();
    project
        .child(".rupu/agents/echo.md")
        .write_str("---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nyou echo.")
        .unwrap();
    project.child(".rupu/workflows").create_dir_all().unwrap();
    project
        .child(format!(".rupu/workflows/{workflow_name}.yaml"))
        .write_str(workflow_yaml)
        .unwrap();
}

#[tokio::test]
async fn explicit_context_runs_in_permissive_mode_and_fails_in_strict_mode() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.create_dir_all().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    let workflow_yaml = r#"name: auto-wf
autoflow:
  enabled: true
steps:
  - id: a
    agent: echo
    actions: []
    prompt: "missing={{ issue.missing }} title={{ issue.title }}"
"#;
    write_agent_and_workflow(&project, "auto-wf", workflow_yaml);

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT);

    let ctx = rupu_cli::cmd::workflow::ExplicitWorkflowRunContext {
        project_root: Some(project.path().to_path_buf()),
        workspace_path: project.path().to_path_buf(),
        workspace_id: "ws_auto".into(),
        inputs: Vec::new(),
        mode: "bypass".into(),
        event: None,
        issue: Some(serde_json::json!({
            "title": "Fix bug",
            "number": 42
        })),
        issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
        system_prompt_suffix: None,
        attach_ui: false,
        use_canvas: false,
        run_id_override: Some("run_explicit_permissive".into()),
        strict_templates: false,
    };
    let summary = rupu_cli::cmd::workflow::run_with_explicit_context("auto-wf", ctx)
        .await
        .expect("permissive run should succeed");
    assert_eq!(summary.run_id, "run_explicit_permissive");

    let strict_ctx = rupu_cli::cmd::workflow::ExplicitWorkflowRunContext {
        project_root: Some(project.path().to_path_buf()),
        workspace_path: project.path().to_path_buf(),
        workspace_id: "ws_auto".into(),
        inputs: Vec::new(),
        mode: "bypass".into(),
        event: None,
        issue: Some(serde_json::json!({
            "title": "Fix bug",
            "number": 42
        })),
        issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
        system_prompt_suffix: None,
        attach_ui: false,
        use_canvas: false,
        run_id_override: Some("run_explicit_strict".into()),
        strict_templates: true,
    };
    let err = rupu_cli::cmd::workflow::run_with_explicit_context("auto-wf", strict_ctx)
        .await
        .unwrap_err();

    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::remove_var("RUPU_HOME");

    assert!(err.to_string().contains("render step a"));
}

#[tokio::test]
async fn autoflow_run_rejects_non_issue_target() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.create_dir_all().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    write_agent_and_workflow(
        &project,
        "auto-wf",
        r#"name: auto-wf
autoflow:
  enabled: true
steps:
  - id: a
    agent: echo
    actions: []
    prompt: hi
"#,
    );

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_current_dir(project.path()).unwrap();
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "autoflow".into(),
        "run".into(),
        "auto-wf".into(),
        "github:Section9Labs/rupu".into(),
    ])
    .await;

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_HOME");

    assert_ne!(exit, std::process::ExitCode::from(0));
}

#[tokio::test]
async fn autoflow_release_deletes_claim() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let claim_store = rupu_workspace::AutoflowClaimStore {
        root: home.join("autoflows/claims"),
    };
    let issue_ref = "github:Section9Labs/rupu/issues/42";
    claim_store
        .save(&rupu_workspace::AutoflowClaimRecord {
            issue_ref: issue_ref.into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            workflow: "auto-wf".into(),
            status: rupu_workspace::ClaimStatus::Claimed,
            worktree_path: None,
            branch: None,
            last_run_id: None,
            last_error: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
        .unwrap();

    std::env::set_var("RUPU_HOME", &home);
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "autoflow".into(),
        "release".into(),
        issue_ref.into(),
    ])
    .await;
    std::env::remove_var("RUPU_HOME");

    assert_eq!(exit, std::process::ExitCode::from(0));
    assert!(claim_store.load(issue_ref).unwrap().is_none());
}
