//! End-to-end and direct runtime tests for `rupu autoflow ...`.
//!
//! These tests mutate process-global state (`RUPU_HOME`,
//! `RUPU_MOCK_PROVIDER_SCRIPT`, cwd). Hold `ENV_LOCK` for the full
//! body.

use assert_cmd::Command;
use assert_fs::prelude::*;
use predicates::prelude::*;
use rupu_workspace::RepoRegistryStore;
use std::collections::BTreeMap;
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
        .args(["config", "commit.gpgsign", "false"])
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

fn track_repo(home: &assert_fs::fixture::ChildPath, repo_ref: &str, path: &std::path::Path) {
    let store = RepoRegistryStore {
        root: home.path().join("repos"),
    };
    store.upsert(repo_ref, path, None, None).unwrap();
}

fn enqueue_issue_wake(
    home: &std::path::Path,
    repo_ref: &str,
    issue_ref: &str,
    event_id: &str,
    payload: Option<serde_json::Value>,
) -> rupu_runtime::WakeRecord {
    rupu_runtime::WakeStore::new(home.join("autoflows/wakes"))
        .enqueue(rupu_runtime::WakeEnqueueRequest {
            source: rupu_runtime::WakeSource::Manual,
            repo_ref: repo_ref.into(),
            entity: rupu_runtime::WakeEntity {
                kind: rupu_runtime::WakeEntityKind::Issue,
                ref_text: issue_ref.into(),
            },
            event: rupu_runtime::WakeEvent {
                id: event_id.into(),
                delivery_id: None,
                dedupe_key: None,
            },
            payload,
            received_at: chrono::Utc::now().to_rfc3339(),
            not_before: chrono::Utc::now().to_rfc3339(),
        })
        .unwrap()
}

fn sample_run_record(id: &str, issue_ref: &str) -> rupu_orchestrator::RunRecord {
    rupu_orchestrator::RunRecord {
        id: id.into(),
        workflow_name: "issue-supervisor-dispatch".into(),
        status: rupu_orchestrator::RunStatus::Pending,
        inputs: BTreeMap::new(),
        event: None,
        workspace_id: "ws_1".into(),
        workspace_path: std::path::PathBuf::from("/tmp/proj"),
        transcript_dir: std::path::PathBuf::from("/tmp/proj/.rupu/transcripts"),
        started_at: chrono::Utc::now(),
        finished_at: None,
        error_message: None,
        awaiting_step_id: None,
        approval_prompt: None,
        awaiting_since: None,
        expires_at: None,
        issue_ref: Some(issue_ref.into()),
        issue: None,
        parent_run_id: None,
        backend_id: Some("local_worktree".into()),
        worker_id: Some("worker_local_test_cli".into()),
        artifact_manifest_path: None,
        source_wake_id: None,
    }
}

#[test]
fn autoflow_list_shows_only_enabled_workflows() {
    let _guard = ENV_LOCK.blocking_lock();

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    home.create_dir_all().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    write_agent_and_workflow(
        &project,
        "controller",
        r#"name: controller
autoflow:
  enabled: true
  source: jira:acme.atlassian.net/ENG
  priority: 100
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
    );
    write_agent_and_workflow(
        &project,
        "manual-only",
        r#"name: manual-only
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .current_dir(project.path())
        .args(["autoflow", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("NAME"))
        .stdout(predicate::str::contains("controller"))
        .stdout(predicate::str::contains("project"))
        .stdout(predicate::str::contains("issue"))
        .stdout(predicate::str::contains("jira:acme.atlassian.net/ENG"))
        .stdout(predicate::str::contains("100"))
        .stdout(predicate::str::contains("manual-only").not());
}

#[test]
fn autoflow_structured_outputs_cover_list_claims_status_and_wakes() {
    let _guard = ENV_LOCK.blocking_lock();

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    home.create_dir_all().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    init_git_checkout(project.path(), "git@github.com:Section9Labs/rupu.git");
    write_agent_and_workflow(
        &project,
        "controller",
        r#"name: controller
autoflow:
  enabled: true
  priority: 100
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
    );
    track_repo(&home, "github:Section9Labs/rupu", project.path());
    let issue_ref = "github:Section9Labs/rupu/issues/42";
    let claim_store = rupu_workspace::AutoflowClaimStore {
        root: home.path().join("autoflows/claims"),
    };
    claim_store
        .save(&rupu_workspace::AutoflowClaimRecord {
            issue_ref: issue_ref.into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: Some("github:Section9Labs/rupu".into()),
            issue_display_ref: Some("ENG-42".into()),
            issue_title: Some("Tracker-native claim".into()),
            issue_url: Some("https://linear.app/acme/issue/ENG-42".into()),
            issue_state_name: Some("In Review".into()),
            issue_tracker: Some("linear".into()),
            workflow: "controller".into(),
            status: rupu_workspace::ClaimStatus::AwaitExternal,
            worktree_path: Some("/tmp/rupu/issue-42".into()),
            branch: Some("rupu/issue-42".into()),
            last_run_id: Some("run_usage".into()),
            last_error: None,
            last_summary: Some("waiting for review".into()),
            pr_url: Some("https://github.com/Section9Labs/rupu/pull/1".into()),
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: Some((chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339()),
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
    enqueue_issue_wake(
        home.path(),
        "github:Section9Labs/rupu",
        issue_ref,
        "github.issue.opened",
        Some(serde_json::json!({ "payload": { "issue": { "number": 42 } } })),
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .current_dir(project.path())
        .args([
            "--format",
            "json",
            "autoflow",
            "list",
            "--repo",
            "github:Section9Labs/rupu",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"autoflow_list\""))
        .stdout(predicate::str::contains("\"name\": \"controller\""));

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .args([
            "--format",
            "json",
            "autoflow",
            "claims",
            "--repo",
            "github:Section9Labs/rupu",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"autoflow_claims\""))
        .stdout(predicate::str::contains(
            "\"issue\": \"github:Section9Labs/rupu/issues/42\"",
        ))
        .stdout(predicate::str::contains("\"issue_display\": \"ENG-42\""))
        .stdout(predicate::str::contains(
            "\"source\": \"github:Section9Labs/rupu\"",
        ))
        .stdout(predicate::str::contains("\"state\": \"In Review\""))
        .stdout(predicate::str::contains(
            "\"repo\": \"github:Section9Labs/rupu\"",
        ))
        .stdout(predicate::str::contains("\"branch\": \"rupu/issue-42\""))
        .stdout(predicate::str::contains("\"workflow\": \"controller\""));

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .args([
            "--format",
            "json",
            "autoflow",
            "status",
            "--repo",
            "github:Section9Labs/rupu",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"autoflow_status\""))
        .stdout(predicate::str::contains("\"contested\""))
        .stdout(predicate::str::contains("\"issue_display\": \"ENG-42\""))
        .stdout(predicate::str::contains(
            "\"source\": \"github:Section9Labs/rupu\"",
        ))
        .stdout(predicate::str::contains("\"state\": \"In Review\""))
        .stdout(predicate::str::contains(
            "\"repo\": \"github:Section9Labs/rupu\"",
        ))
        .stdout(predicate::str::contains("\"await_external\""));

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .args([
            "--format",
            "csv",
            "autoflow",
            "wakes",
            "--repo",
            "github:Section9Labs/rupu",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "wake_id,state,source,event,entity,not_before,repo",
        ))
        .stdout(predicate::str::contains("github.issue.opened"))
        .stdout(predicate::str::contains(
            "github:Section9Labs/rupu/issues/42",
        ));
}

#[test]
fn autoflow_show_prints_resolved_metadata_and_body() {
    let _guard = ENV_LOCK.blocking_lock();

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    home.create_dir_all().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    write_agent_and_workflow(
        &project,
        "controller",
        r#"name: controller
autoflow:
  enabled: true
  source: linear:72b2a2dc-6f4f-4423-9d34-24b5bd10634a
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["autoflow", "bug"]
    labels_any: ["urgent", "p1"]
    labels_none: ["blocked"]
    limit: 25
  wake_on:
    - github.issue.labeled
    - github.pull_request.closed
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}-{{ inputs.phase }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .current_dir(project.path())
        .args(["autoflow", "show", "controller"])
        .assert()
        .success()
        .stdout(predicate::str::contains("priority: 100"))
        .stdout(predicate::str::contains("entity: issue"))
        .stdout(predicate::str::contains(
            "source: linear:72b2a2dc-6f4f-4423-9d34-24b5bd10634a",
        ))
        .stdout(predicate::str::contains("workspace: worktree"))
        .stdout(predicate::str::contains(
            "workspace branch: rupu/issue-{{ issue.number }}-{{ inputs.phase }}",
        ))
        .stdout(predicate::str::contains("reconcile_every: 10m"))
        .stdout(predicate::str::contains(
            "wake_on: github.issue.labeled,github.pull_request.closed",
        ))
        .stdout(predicate::str::contains("claim ttl: 3h"))
        .stdout(predicate::str::contains("outcome output: result"))
        .stdout(predicate::str::contains("selector states: open"))
        .stdout(predicate::str::contains(
            "selector labels_all: autoflow,bug",
        ))
        .stdout(predicate::str::contains("selector labels_any: urgent,p1"))
        .stdout(predicate::str::contains("selector labels_none: blocked"))
        .stdout(predicate::str::contains("selector limit: 25"))
        .stdout(predicate::str::contains("---"))
        .stdout(predicate::str::contains("name: controller"));
}

#[test]
fn autoflow_list_includes_tracked_repo_workflows_outside_project() {
    let _guard = ENV_LOCK.blocking_lock();

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    home.create_dir_all().unwrap();
    let outside = tmp.child("outside");
    outside.create_dir_all().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    init_git_checkout(project.path(), "git@github.com:Section9Labs/rupu.git");
    write_agent_and_workflow(
        &project,
        "controller",
        r#"name: controller
autoflow:
  enabled: true
  priority: 100
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
    );
    track_repo(&home, "github:Section9Labs/rupu", project.path());

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .current_dir(outside.path())
        .args(["autoflow", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("controller"))
        .stdout(predicate::str::contains("github:Section9Labs/rupu"));
}

#[test]
fn autoflow_list_filters_to_one_repo() {
    let _guard = ENV_LOCK.blocking_lock();

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    home.create_dir_all().unwrap();
    let outside = tmp.child("outside");
    outside.create_dir_all().unwrap();
    let repo_a = assert_fs::TempDir::new().unwrap();
    let repo_b = assert_fs::TempDir::new().unwrap();
    init_git_checkout(repo_a.path(), "git@github.com:Section9Labs/rupu.git");
    init_git_checkout(repo_b.path(), "git@github.com:Section9Labs/okegu.git");
    write_agent_and_workflow(
        &repo_a,
        "controller",
        r#"name: controller
autoflow:
  enabled: true
  priority: 100
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
    );
    write_agent_and_workflow(
        &repo_b,
        "controller",
        r#"name: controller
autoflow:
  enabled: true
  priority: 50
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
    );
    track_repo(&home, "github:Section9Labs/rupu", repo_a.path());
    track_repo(&home, "github:Section9Labs/okegu", repo_b.path());

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .current_dir(outside.path())
        .args(["autoflow", "list", "--repo", "github:Section9Labs/rupu"])
        .assert()
        .success()
        .stdout(predicate::str::contains("github:Section9Labs/rupu"))
        .stdout(predicate::str::contains("github:Section9Labs/okegu").not())
        .stdout(predicate::str::contains("100"))
        .stdout(predicate::str::contains("50").not());
}

#[test]
fn autoflow_show_resolves_tracked_repo_outside_project() {
    let _guard = ENV_LOCK.blocking_lock();

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    home.create_dir_all().unwrap();
    let outside = tmp.child("outside");
    outside.create_dir_all().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    init_git_checkout(project.path(), "git@github.com:Section9Labs/rupu.git");
    write_agent_and_workflow(
        &project,
        "controller",
        r#"name: controller
autoflow:
  enabled: true
  priority: 100
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
    );
    track_repo(&home, "github:Section9Labs/rupu", project.path());

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .current_dir(outside.path())
        .args([
            "autoflow",
            "show",
            "controller",
            "--repo",
            "github:Section9Labs/rupu",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("scope: project"))
        .stdout(predicate::str::contains("repo: github:Section9Labs/rupu"))
        .stdout(predicate::str::contains("preferred checkout:"))
        .stdout(predicate::str::contains("name: controller"));
}

#[test]
fn autoflow_show_errors_when_name_is_ambiguous_across_tracked_repos() {
    let _guard = ENV_LOCK.blocking_lock();

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    home.create_dir_all().unwrap();
    let outside = tmp.child("outside");
    outside.create_dir_all().unwrap();
    let repo_a = assert_fs::TempDir::new().unwrap();
    let repo_b = assert_fs::TempDir::new().unwrap();
    init_git_checkout(repo_a.path(), "git@github.com:Section9Labs/rupu.git");
    init_git_checkout(repo_b.path(), "git@github.com:Section9Labs/okegu.git");
    for repo in [&repo_a, &repo_b] {
        write_agent_and_workflow(
            repo,
            "controller",
            r#"name: controller
autoflow:
  enabled: true
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
        );
    }
    track_repo(&home, "github:Section9Labs/rupu", repo_a.path());
    track_repo(&home, "github:Section9Labs/okegu", repo_b.path());

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .current_dir(outside.path())
        .args(["autoflow", "show", "controller"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "multiple autoflows named `controller` are visible",
        ))
        .stderr(predicate::str::contains(
            "pass `--repo <platform>:<owner>/<repo>` to disambiguate",
        ))
        .stderr(predicate::str::contains("github:Section9Labs/rupu"))
        .stderr(predicate::str::contains("github:Section9Labs/okegu"));
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
        invocation_source: rupu_runtime::RunTriggerSource::Autoflow,
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
        run_envelope_template: None,
        worker: None,
    };
    let summary = rupu_cli::cmd::workflow::run_with_explicit_context("auto-wf", ctx)
        .await
        .expect("permissive run should succeed");
    assert_eq!(summary.run_id, "run_explicit_permissive");
    assert_eq!(summary.backend_id.as_deref(), Some("local_worktree"));
    assert!(summary.worker_id.is_some(), "expected worker id in summary");
    assert!(
        summary.artifact_manifest_path.is_some(),
        "expected artifact manifest path in summary"
    );
    let envelope = rupu_orchestrator::RunStore::new(global.path().join("runs"))
        .read_run_envelope(&summary.run_id)
        .expect("run envelope should persist");
    assert_eq!(
        envelope.trigger.source,
        rupu_runtime::RunTriggerSource::Autoflow
    );
    assert_eq!(
        envelope
            .context
            .as_ref()
            .and_then(|context| context.issue_ref.as_deref()),
        Some("github:Section9Labs/rupu/issues/42")
    );
    assert!(envelope
        .context
        .as_ref()
        .is_some_and(|context| context.issue_present));
    assert_eq!(
        envelope
            .worker
            .as_ref()
            .and_then(|worker| worker.assigned_worker_id.as_deref()),
        summary.worker_id.as_deref()
    );
    let run_store = rupu_orchestrator::RunStore::new(global.path().join("runs"));
    let manifest = run_store
        .read_artifact_manifest(&summary.run_id)
        .expect("artifact manifest should persist");
    assert_eq!(manifest.backend_id, "local_worktree");
    assert_eq!(manifest.worker_id, summary.worker_id);
    assert!(manifest
        .artifacts
        .iter()
        .any(|artifact| artifact.kind == rupu_runtime::ArtifactKind::StepTranscript));

    let strict_ctx = rupu_cli::cmd::workflow::ExplicitWorkflowRunContext {
        project_root: Some(project.path().to_path_buf()),
        workspace_path: project.path().to_path_buf(),
        workspace_id: "ws_auto".into(),
        inputs: Vec::new(),
        mode: "bypass".into(),
        invocation_source: rupu_runtime::RunTriggerSource::Autoflow,
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
        run_envelope_template: None,
        worker: None,
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
async fn autoflow_run_rejects_ask_mode_override() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    home.create_dir_all().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    init_git_checkout(project.path(), "git@github.com:Section9Labs/rupu.git");
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
    track_repo(&home, "github:Section9Labs/rupu", project.path());

    std::env::set_var("RUPU_HOME", home.path());
    std::env::set_current_dir(project.path()).unwrap();
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "autoflow".into(),
        "run".into(),
        "auto-wf".into(),
        "github:Section9Labs/rupu/issues/42".into(),
        "--mode".into(),
        "ask".into(),
    ])
    .await;

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_HOME");

    assert_ne!(exit, std::process::ExitCode::from(0));
}

#[tokio::test]
async fn autoflow_run_rejects_ask_mode_from_config() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    home.create_dir_all().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    init_git_checkout(project.path(), "git@github.com:Section9Labs/rupu.git");
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
    project
        .child(".rupu/config.toml")
        .write_str("[autoflow]\npermission_mode = \"ask\"\n")
        .unwrap();
    track_repo(&home, "github:Section9Labs/rupu", project.path());

    std::env::set_var("RUPU_HOME", home.path());
    std::env::set_current_dir(project.path()).unwrap();
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "autoflow".into(),
        "run".into(),
        "auto-wf".into(),
        "github:Section9Labs/rupu/issues/42".into(),
    ])
    .await;

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_HOME");

    assert_ne!(exit, std::process::ExitCode::from(0));
}

#[tokio::test]
async fn autoflow_run_rejects_unknown_mode_override() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    home.create_dir_all().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    init_git_checkout(project.path(), "git@github.com:Section9Labs/rupu.git");
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
    track_repo(&home, "github:Section9Labs/rupu", project.path());

    std::env::set_var("RUPU_HOME", home.path());
    std::env::set_current_dir(project.path()).unwrap();
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "autoflow".into(),
        "run".into(),
        "auto-wf".into(),
        "github:Section9Labs/rupu/issues/42".into(),
        "--mode".into(),
        "admin".into(),
    ])
    .await;

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_HOME");

    assert_ne!(exit, std::process::ExitCode::from(0));
}

#[tokio::test]
async fn autoflow_run_rejects_unknown_mode_from_config() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    home.create_dir_all().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    init_git_checkout(project.path(), "git@github.com:Section9Labs/rupu.git");
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
    project
        .child(".rupu/config.toml")
        .write_str("[autoflow]\npermission_mode = \"admin\"\n")
        .unwrap();
    track_repo(&home, "github:Section9Labs/rupu", project.path());

    std::env::set_var("RUPU_HOME", home.path());
    std::env::set_current_dir(project.path()).unwrap();
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "autoflow".into(),
        "run".into(),
        "auto-wf".into(),
        "github:Section9Labs/rupu/issues/42".into(),
    ])
    .await;

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_HOME");

    assert_ne!(exit, std::process::ExitCode::from(0));
}

#[tokio::test]
async fn autoflow_run_rejects_existing_owned_claim() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    home.create_dir_all().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    init_git_checkout(project.path(), "git@github.com:Section9Labs/rupu.git");
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
    track_repo(&home, "github:Section9Labs/rupu", project.path());
    let claim_store = rupu_workspace::AutoflowClaimStore {
        root: home.path().join("autoflows/claims"),
    };
    claim_store
        .save(&rupu_workspace::AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu/issues/42".into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "controller".into(),
            status: rupu_workspace::ClaimStatus::AwaitExternal,
            worktree_path: Some("/tmp/rupu/issue-42".into()),
            branch: None,
            last_run_id: None,
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: Some((chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339()),
            pending_dispatch: None,
            contenders: vec![],
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
        .unwrap();

    std::env::set_var("RUPU_HOME", home.path());
    std::env::set_current_dir(project.path()).unwrap();
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "autoflow".into(),
        "run".into(),
        "auto-wf".into(),
        "github:Section9Labs/rupu/issues/42".into(),
    ])
    .await;

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_HOME");

    assert_ne!(exit, std::process::ExitCode::from(0));
}

#[tokio::test]
async fn autoflow_run_rejects_blocked_claim_until_release() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    home.create_dir_all().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    init_git_checkout(project.path(), "git@github.com:Section9Labs/rupu.git");
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
    track_repo(&home, "github:Section9Labs/rupu", project.path());
    let claim_store = rupu_workspace::AutoflowClaimStore {
        root: home.path().join("autoflows/claims"),
    };
    claim_store
        .save(&rupu_workspace::AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu/issues/42".into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "controller".into(),
            status: rupu_workspace::ClaimStatus::Blocked,
            worktree_path: Some("/tmp/rupu/issue-42".into()),
            branch: None,
            last_run_id: None,
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: Some("2000-01-01T00:00:00Z".into()),
            pending_dispatch: None,
            contenders: vec![],
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
        .unwrap();

    std::env::set_var("RUPU_HOME", home.path());
    std::env::set_current_dir(project.path()).unwrap();
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "autoflow".into(),
        "run".into(),
        "auto-wf".into(),
        "github:Section9Labs/rupu/issues/42".into(),
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
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "auto-wf".into(),
            status: rupu_workspace::ClaimStatus::Claimed,
            worktree_path: Some(worktree.path.display().to_string()),
            branch: None,
            last_run_id: None,
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
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
            source_ref: Some("linear:eng-team".into()),
            issue_display_ref: Some("ENG-42".into()),
            issue_title: Some("Tracker-native claim".into()),
            issue_url: Some("https://linear.app/acme/issue/ENG-42".into()),
            issue_state_name: Some("In Review".into()),
            issue_tracker: Some("linear".into()),
            workflow: "controller".into(),
            status: rupu_workspace::ClaimStatus::Claimed,
            worktree_path: Some("/tmp/rupu/issue-42".into()),
            branch: Some("rupu/eng-42".into()),
            last_run_id: Some("run_123".into()),
            last_error: None,
            last_summary: Some("Draft PR opened and ready for review".into()),
            pr_url: Some("https://github.com/Section9Labs/rupu/pull/42".into()),
            artifacts: Some(serde_json::json!({
                "review_packet": "docs/reviews/issue-42.json"
            })),
            artifact_manifest_path: Some("/tmp/runs/run_123/artifact_manifest.json".into()),
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
        .stdout(predicate::str::contains("Source"))
        .stdout(predicate::str::contains("State"))
        .stdout(predicate::str::contains("Repo"))
        .stdout(predicate::str::contains("Branch"))
        .stdout(predicate::str::contains("Priority"))
        .stdout(predicate::str::contains("PR"))
        .stdout(predicate::str::contains("Summary"))
        .stdout(predicate::str::contains("Contenders"))
        .stdout(predicate::str::contains("ENG-42"))
        .stdout(predicate::str::contains("linear:eng-team"))
        .stdout(predicate::str::contains("In Review"))
        .stdout(predicate::str::contains("controller"))
        .stdout(predicate::str::contains("github:Section9Labs/rupu"))
        .stdout(predicate::str::contains("rupu/eng-42"))
        .stdout(predicate::str::contains(
            "https://github.com/Section9Labs/rupu/pull/42",
        ))
        .stdout(predicate::str::contains(
            "Draft PR opened and ready for review",
        ))
        .stdout(predicate::str::contains("*controller[100]"))
        .stdout(predicate::str::contains("phase-ready[50]"));
}

#[test]
fn autoflow_claims_filters_to_one_repo() {
    let _guard = ENV_LOCK.blocking_lock();

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let claim_store = rupu_workspace::AutoflowClaimStore {
        root: home.join("autoflows/claims"),
    };
    for (repo_ref, issue_ref) in [
        (
            "github:Section9Labs/rupu",
            "github:Section9Labs/rupu/issues/42",
        ),
        (
            "github:Section9Labs/okegu",
            "github:Section9Labs/okegu/issues/9",
        ),
    ] {
        claim_store
            .save(&rupu_workspace::AutoflowClaimRecord {
                issue_ref: issue_ref.into(),
                repo_ref: repo_ref.into(),
                source_ref: None,
                issue_display_ref: None,
                issue_title: None,
                issue_url: None,
                issue_state_name: None,
                issue_tracker: None,
                workflow: "controller".into(),
                status: rupu_workspace::ClaimStatus::Claimed,
                worktree_path: Some("/tmp/rupu/issue".into()),
                branch: None,
                last_run_id: None,
                last_error: None,
                last_summary: None,
                pr_url: None,
                artifacts: None,
                artifact_manifest_path: None,
                next_retry_at: None,
                claim_owner: None,
                lease_expires_at: None,
                pending_dispatch: None,
                contenders: vec![rupu_workspace::AutoflowContender {
                    workflow: "controller".into(),
                    priority: 100,
                    scope: Some("project".into()),
                    selected: true,
                }],
                updated_at: chrono::Utc::now().to_rfc3339(),
            })
            .unwrap();
    }

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args(["autoflow", "claims", "--repo", "github:Section9Labs/rupu"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "github:Section9Labs/rupu/issues/42",
        ))
        .stdout(predicate::str::contains("github:Section9Labs/okegu/issues/9").not());
}

#[test]
fn autoflow_status_summarizes_counts_and_contested_issues() {
    let _guard = ENV_LOCK.blocking_lock();

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let claim_store = rupu_workspace::AutoflowClaimStore {
        root: home.join("autoflows/claims"),
    };
    claim_store
        .save(&rupu_workspace::AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu/issues/42".into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: Some("linear:eng-team".into()),
            issue_display_ref: Some("ENG-42".into()),
            issue_title: Some("Tracker-native claim".into()),
            issue_url: Some("https://linear.app/acme/issue/ENG-42".into()),
            issue_state_name: Some("In Review".into()),
            issue_tracker: Some("linear".into()),
            workflow: "controller".into(),
            status: rupu_workspace::ClaimStatus::Claimed,
            worktree_path: Some("/tmp/rupu/issue-42".into()),
            branch: Some("rupu/eng-42".into()),
            last_run_id: Some("run_123".into()),
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
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
    claim_store
        .save(&rupu_workspace::AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu/issues/43".into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "controller".into(),
            status: rupu_workspace::ClaimStatus::AwaitHuman,
            worktree_path: Some("/tmp/rupu/issue-43".into()),
            branch: None,
            last_run_id: Some("run_124".into()),
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![rupu_workspace::AutoflowContender {
                workflow: "controller".into(),
                priority: 100,
                scope: Some("project".into()),
                selected: true,
            }],
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
        .unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args(["autoflow", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("STATUS"))
        .stdout(predicate::str::contains("claimed"))
        .stdout(predicate::str::contains("await_human"))
        .stdout(predicate::str::contains("contested issues:"))
        .stdout(predicate::str::contains("ENG-42"))
        .stdout(predicate::str::contains("linear:eng-team"))
        .stdout(predicate::str::contains("In Review"))
        .stdout(predicate::str::contains("github:Section9Labs/rupu"))
        .stdout(predicate::str::contains("*controller[100]"))
        .stdout(predicate::str::contains("phase-ready[50]"));
}

#[test]
fn autoflow_status_filters_to_one_repo() {
    let _guard = ENV_LOCK.blocking_lock();

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let claim_store = rupu_workspace::AutoflowClaimStore {
        root: home.join("autoflows/claims"),
    };
    claim_store
        .save(&rupu_workspace::AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu/issues/42".into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "controller".into(),
            status: rupu_workspace::ClaimStatus::Claimed,
            worktree_path: Some("/tmp/rupu/issue-42".into()),
            branch: None,
            last_run_id: None,
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![],
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
        .unwrap();
    claim_store
        .save(&rupu_workspace::AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/okegu/issues/9".into(),
            repo_ref: "github:Section9Labs/okegu".into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "controller".into(),
            status: rupu_workspace::ClaimStatus::AwaitHuman,
            worktree_path: Some("/tmp/rupu/issue-9".into()),
            branch: None,
            last_run_id: None,
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![],
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
        .unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args(["autoflow", "status", "--repo", "github:Section9Labs/rupu"])
        .assert()
        .success()
        .stdout(predicate::str::contains("claimed"))
        .stdout(predicate::str::contains("await_human").not())
        .stdout(predicate::str::contains("github:Section9Labs/okegu/issues/9").not());
}

#[test]
fn autoflow_wakes_lists_queued_and_processed_rows() {
    let _guard = ENV_LOCK.blocking_lock();

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let repo_ref = "github:Section9Labs/rupu";
    let issue_ref = "github:Section9Labs/rupu/issues/42";
    let queued = enqueue_issue_wake(&home, repo_ref, issue_ref, "github.issue.opened", None);
    let processed = enqueue_issue_wake(
        &home,
        repo_ref,
        issue_ref,
        "github.issue.labeled",
        Some(serde_json::json!({ "payload": { "issue": { "number": 42 } } })),
    );
    rupu_runtime::WakeStore::new(home.join("autoflows/wakes"))
        .mark_processed(&processed.wake_id)
        .unwrap();
    enqueue_issue_wake(
        &home,
        "github:Section9Labs/okegu",
        "github:Section9Labs/okegu/issues/9",
        "github.issue.opened",
        None,
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args(["autoflow", "wakes", "--repo", repo_ref])
        .assert()
        .success()
        .stdout(predicate::str::contains("Wake"))
        .stdout(predicate::str::contains(&queued.wake_id))
        .stdout(predicate::str::contains(&processed.wake_id))
        .stdout(predicate::str::contains("queued"))
        .stdout(predicate::str::contains("processed"))
        .stdout(predicate::str::contains("github:Section9Labs/okegu/issues/9").not());
}

#[test]
fn autoflow_explain_prints_claim_run_and_wake_context() {
    let _guard = ENV_LOCK.blocking_lock();

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let issue_ref = "github:Section9Labs/rupu/issues/42";
    let repo_ref = "github:Section9Labs/rupu";
    let source_wake = enqueue_issue_wake(&home, repo_ref, issue_ref, "github.issue.labeled", None);
    rupu_runtime::WakeStore::new(home.join("autoflows/wakes"))
        .mark_processed(&source_wake.wake_id)
        .unwrap();
    enqueue_issue_wake(
        &home,
        repo_ref,
        issue_ref,
        "autoflow.dispatch.pending",
        None,
    );

    let run_store = rupu_orchestrator::RunStore::new(home.join("runs"));
    let mut run = sample_run_record("run_123", issue_ref);
    run.status = rupu_orchestrator::RunStatus::AwaitingApproval;
    run.awaiting_step_id = Some("review".into());
    run.expires_at = Some(chrono::Utc::now() + chrono::Duration::minutes(30));
    run.source_wake_id = Some(source_wake.wake_id.clone());
    run_store
        .create(run, "name: issue-supervisor-dispatch\nsteps: []\n")
        .unwrap();

    let claim_store = rupu_workspace::AutoflowClaimStore {
        root: home.join("autoflows/claims"),
    };
    claim_store
        .save(&rupu_workspace::AutoflowClaimRecord {
            issue_ref: issue_ref.into(),
            repo_ref: repo_ref.into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "issue-supervisor-dispatch".into(),
            status: rupu_workspace::ClaimStatus::AwaitHuman,
            worktree_path: Some("/tmp/rupu/issue-42".into()),
            branch: Some("rupu/issue-42".into()),
            last_run_id: Some("run_123".into()),
            last_error: None,
            last_summary: Some("waiting for review".into()),
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: Some("worker_local_test_serve".into()),
            lease_expires_at: Some((chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339()),
            pending_dispatch: Some(rupu_workspace::PendingDispatch {
                workflow: "phase-delivery-cycle".into(),
                target: issue_ref.into(),
                inputs: BTreeMap::from([("phase".into(), "phase-1".into())]),
            }),
            contenders: vec![
                rupu_workspace::AutoflowContender {
                    workflow: "issue-supervisor-dispatch".into(),
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
        .args(["autoflow", "explain", issue_ref])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "issue: github:Section9Labs/rupu/issues/42",
        ))
        .stdout(predicate::str::contains(
            "workflow: issue-supervisor-dispatch",
        ))
        .stdout(predicate::str::contains("last run: run_123"))
        .stdout(predicate::str::contains(
            "last run status: awaiting_approval",
        ))
        .stdout(predicate::str::contains("source wake:"))
        .stdout(predicate::str::contains("approval gate: step=review"))
        .stdout(predicate::str::contains(
            "pending dispatch: phase-delivery-cycle",
        ))
        .stdout(predicate::str::contains("queued wakes:"))
        .stdout(predicate::str::contains("recent processed wake:"));
}

#[test]
fn autoflow_explain_supports_tracker_native_issue_refs() {
    let _guard = ENV_LOCK.blocking_lock();

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let claim_store = rupu_workspace::AutoflowClaimStore {
        root: home.join("autoflows/claims"),
    };
    let issue_ref = "linear:eng-team/issues/42";
    claim_store
        .save(&rupu_workspace::AutoflowClaimRecord {
            issue_ref: issue_ref.into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: Some("linear:eng-team".into()),
            issue_display_ref: Some("ENG-42".into()),
            issue_title: Some("Tracker-native claim".into()),
            issue_url: Some("https://linear.app/acme/issue/ENG-42".into()),
            issue_state_name: Some("In Progress".into()),
            issue_tracker: Some("linear".into()),
            workflow: "tracker-controller".into(),
            status: rupu_workspace::ClaimStatus::AwaitExternal,
            worktree_path: Some("/tmp/rupu/eng-42".into()),
            branch: Some("rupu/eng-42".into()),
            last_run_id: None,
            last_error: None,
            last_summary: Some("waiting for tracker transition".into()),
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![rupu_workspace::AutoflowContender {
                workflow: "tracker-controller".into(),
                priority: 100,
                scope: Some("source".into()),
                selected: true,
            }],
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
        .unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args(["autoflow", "explain", issue_ref])
        .assert()
        .success()
        .stdout(predicate::str::contains("issue: linear:eng-team/issues/42"))
        .stdout(predicate::str::contains("repo: github:Section9Labs/rupu"))
        .stdout(predicate::str::contains("issue display: ENG-42"))
        .stdout(predicate::str::contains("issue tracker: linear"))
        .stdout(predicate::str::contains("issue state: In Progress"))
        .stdout(predicate::str::contains("source: linear:eng-team"))
        .stdout(predicate::str::contains(
            "issue url: https://linear.app/acme/issue/ENG-42",
        ))
        .stdout(predicate::str::contains("workflow: tracker-controller"));
}

#[test]
fn autoflow_explain_resolves_unclaimed_tracker_native_issue_from_visible_source() {
    let _guard = ENV_LOCK.blocking_lock();

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    home.create_dir_all().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    init_git_checkout(project.path(), "git@github.com:Section9Labs/rupu.git");
    write_agent_and_workflow(
        &project,
        "tracker-controller",
        r#"name: tracker-controller
autoflow:
  enabled: true
  source: linear:eng-team
  priority: 100
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
    );
    project
        .child(".rupu/config.toml")
        .write_str(
            r#"[autoflow]
enabled = true
repo = "github:Section9Labs/rupu"
"#,
        )
        .unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .current_dir(project.path())
        .args(["autoflow", "explain", "linear:eng-team/issues/42"])
        .assert()
        .success()
        .stdout(predicate::str::contains("issue: linear:eng-team/issues/42"))
        .stdout(predicate::str::contains("repo: github:Section9Labs/rupu"))
        .stdout(predicate::str::contains("status: unclaimed"))
        .stdout(predicate::str::contains("source: linear:eng-team"))
        .stdout(predicate::str::contains(
            "candidate workflows: tracker-controller",
        ));
}

#[test]
fn autoflow_doctor_reports_state_problems() {
    let _guard = ENV_LOCK.blocking_lock();

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let issue_ref = "github:Section9Labs/rupu/issues/42";
    let repo_ref = "github:Section9Labs/missing";
    let wake = enqueue_issue_wake(
        &home,
        repo_ref,
        issue_ref,
        "github.issue.opened",
        Some(serde_json::json!({ "payload": { "issue": { "number": 42 } } })),
    );
    let payload_path = home
        .join("autoflows/wakes/payloads")
        .join(format!("{}.json", wake.wake_id));
    std::fs::remove_file(&payload_path).unwrap();

    let claim_store = rupu_workspace::AutoflowClaimStore {
        root: home.join("autoflows/claims"),
    };
    claim_store
        .save(&rupu_workspace::AutoflowClaimRecord {
            issue_ref: issue_ref.into(),
            repo_ref: repo_ref.into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "issue-supervisor-dispatch".into(),
            status: rupu_workspace::ClaimStatus::Claimed,
            worktree_path: Some("/tmp/does-not-exist".into()),
            branch: Some("rupu/issue-42".into()),
            last_run_id: Some("run_missing".into()),
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: Some("/tmp/manifest-missing.json".into()),
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![],
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
        .unwrap();
    let issue_dir = home
        .join("autoflows/claims")
        .join(rupu_workspace::autoflow_claim_store::issue_key(issue_ref));
    std::fs::create_dir_all(&issue_dir).unwrap();
    std::fs::write(
        issue_dir.join(".lock"),
        toml::to_string(&rupu_workspace::ActiveLockRecord {
            owner: "worker_local_test_serve".into(),
            acquired_at: chrono::Utc::now().to_rfc3339(),
            lease_expires_at: Some((chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339()),
        })
        .unwrap(),
    )
    .unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args(["autoflow", "doctor"])
        .assert()
        .success()
        .stdout(predicate::str::contains("missing_repo_binding"))
        .stdout(predicate::str::contains("missing_worktree"))
        .stdout(predicate::str::contains("artifact_missing"))
        .stdout(predicate::str::contains("missing_run"))
        .stdout(predicate::str::contains("invalid_wake_payload"))
        .stdout(predicate::str::contains("stale_lock"));
}

#[test]
fn autoflow_repair_rebuilds_worktree_and_drops_invalid_wake() {
    let _guard = ENV_LOCK.blocking_lock();

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
    let issue_ref = "github:Section9Labs/rupu/issues/42";
    let expected_worktree = rupu_workspace::issue_worktree_path(
        &home.join("autoflows/worktrees"),
        "github:Section9Labs/rupu",
        issue_ref,
    );
    let wake = enqueue_issue_wake(
        &home,
        "github:Section9Labs/rupu",
        issue_ref,
        "github.issue.opened",
        Some(serde_json::json!({ "payload": { "issue": { "number": 42 } } })),
    );
    std::fs::remove_file(
        home.join("autoflows/wakes/payloads")
            .join(format!("{}.json", wake.wake_id)),
    )
    .unwrap();

    let claim_store = rupu_workspace::AutoflowClaimStore {
        root: home.join("autoflows/claims"),
    };
    claim_store
        .save(&rupu_workspace::AutoflowClaimRecord {
            issue_ref: issue_ref.into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "issue-supervisor-dispatch".into(),
            status: rupu_workspace::ClaimStatus::Claimed,
            worktree_path: Some(expected_worktree.display().to_string()),
            branch: Some("rupu/issue-42".into()),
            last_run_id: None,
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![],
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
        .unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args(["autoflow", "repair", issue_ref])
        .assert()
        .success()
        .stdout(predicate::str::contains("deleted invalid queued wake"))
        .stdout(predicate::str::contains("rebuilt worktree"));

    let claim = claim_store.load(issue_ref).unwrap().unwrap();
    assert!(std::path::Path::new(claim.worktree_path.as_deref().unwrap()).exists());
    assert!(rupu_runtime::WakeStore::new(home.join("autoflows/wakes"))
        .list_queued()
        .unwrap()
        .is_empty());
}

#[test]
fn autoflow_requeue_enqueues_manual_wake() {
    let _guard = ENV_LOCK.blocking_lock();

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let issue_ref = "github:Section9Labs/rupu/issues/42";

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args([
            "autoflow",
            "requeue",
            issue_ref,
            "--event",
            "github.issue.reopened",
            "--not-before",
            "10m",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("queued wake_"));

    let queued = rupu_runtime::WakeStore::new(home.join("autoflows/wakes"))
        .list_queued()
        .unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].source, rupu_runtime::WakeSource::Manual);
    assert_eq!(queued[0].event.id, "github.issue.reopened");
    assert_eq!(queued[0].entity.ref_text, issue_ref);
}
