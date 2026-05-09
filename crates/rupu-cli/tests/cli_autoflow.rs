//! End-to-end and direct runtime tests for `rupu autoflow ...`.
//!
//! These tests mutate process-global state (`RUPU_HOME`,
//! `RUPU_MOCK_PROVIDER_SCRIPT`, cwd). Hold `ENV_LOCK` for the full
//! body.

use assert_cmd::Command;
use assert_fs::prelude::*;
use predicates::prelude::*;
use std::process::Command as ProcessCommand;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

const MOCK_SCRIPT: &str = r#"
[
  { "AssistantText": { "text": "autoflow output", "stop": "end_turn" } }
]
"#;

fn init_git_checkout(path: &std::path::Path, origin_url: &str) {
    std::fs::create_dir_all(path).unwrap();
    assert!(ProcessCommand::new("git")
        .arg("init")
        .arg("-b")
        .arg("main")
        .arg(path)
        .status()
        .unwrap()
        .success());
    assert!(ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(["config", "user.email", "test@example.com"])
        .status()
        .unwrap()
        .success());
    assert!(ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(["config", "user.name", "Test User"])
        .status()
        .unwrap()
        .success());
    assert!(ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(["remote", "add", "origin", origin_url])
        .status()
        .unwrap()
        .success());
    std::fs::write(path.join("README.md"), "hello\n").unwrap();
    assert!(ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(["add", "README.md"])
        .status()
        .unwrap()
        .success());
    assert!(ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(["commit", "-m", "init"])
        .status()
        .unwrap()
        .success());
}

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
    let project = tmp.path().join("repo");
    init_git_checkout(&project, "git@github.com:Section9Labs/rupu.git");
    let repo_store = rupu_workspace::RepoRegistryStore {
        root: home.join("repos"),
    };
    repo_store
        .upsert(
            "github:Section9Labs/rupu",
            &project,
            Some("git@github.com:Section9Labs/rupu.git"),
            Some("main"),
        )
        .unwrap();
    let worktree = rupu_workspace::ensure_issue_worktree(
        &project,
        &home.join("autoflows/worktrees"),
        "github:Section9Labs/rupu",
        "github:Section9Labs/rupu/issues/42",
        "rupu/issue-42",
        Some("HEAD"),
    )
    .unwrap();
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
            worktree_path: Some(worktree.path.display().to_string()),
            branch: None,
            last_run_id: None,
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![],
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
    assert!(!worktree.path.exists());
}

#[test]
fn autoflow_claims_shows_contenders_and_selected_priority() {
    let _guard = ENV_LOCK.blocking_lock();

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
            workflow: "controller".into(),
            status: rupu_workspace::ClaimStatus::Claimed,
            worktree_path: Some("/tmp/rupu/issue-42".into()),
            branch: None,
            last_run_id: Some("run_123".into()),
            last_error: None,
            last_summary: Some("Draft PR opened and ready for review".into()),
            pr_url: Some("https://github.com/Section9Labs/rupu/pull/42".into()),
            artifacts: Some(serde_json::json!({
                "review_packet": "docs/reviews/issue-42.json"
            })),
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![
                rupu_workspace::AutoflowContender {
                    workflow: "controller".into(),
                    priority: 100,
                    scope: Some("project".into()),
                    selected: true,
                },
                rupu_workspace::AutoflowContender {
                    workflow: "phase-ready".into(),
                    priority: 50,
                    scope: Some("project".into()),
                    selected: false,
                },
            ],
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
        .unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args(["autoflow", "claims"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Priority"))
        .stdout(predicate::str::contains("PR"))
        .stdout(predicate::str::contains("Summary"))
        .stdout(predicate::str::contains("Contenders"))
        .stdout(predicate::str::contains("controller"))
        .stdout(predicate::str::contains(
            "https://github.com/Section9Labs/rupu/pull/42",
        ))
        .stdout(predicate::str::contains(
            "Draft PR opened and ready for review",
        ))
        .stdout(predicate::str::contains("*controller[100]"))
        .stdout(predicate::str::contains("phase-ready[50]"));
}
