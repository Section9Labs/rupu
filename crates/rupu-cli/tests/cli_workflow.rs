//! End-to-end tests for `rupu workflow list | show | run`.
//!
//! These tests mutate process-global state (`RUPU_HOME`, cwd) and the
//! `RUPU_MOCK_PROVIDER_SCRIPT` env-var seam. Hold `ENV_LOCK` for the
//! whole body of every test to serialize them within this binary.

use assert_cmd::Command as AssertCommand;
use assert_fs::prelude::*;
use predicates::prelude::PredicateBooleanExt;
use std::process::Command;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

const MOCK_SCRIPT: &str = r#"
[
  { "AssistantText": { "text": "step output", "stop": "end_turn" } }
]
"#;

const WORKFLOW_YAML: &str = r#"name: hello-wf
steps:
  - id: a
    agent: echo
    actions: []
    prompt: hi
"#;

const FANOUT_WORKFLOW_YAML: &str = r#"name: fanout-wf
inputs:
  files:
    type: string
steps:
  - id: review
    agent: echo
    actions: []
    for_each: "{{ inputs.files }}"
    max_parallel: 2
    prompt: "review {{ item }}"
"#;

const WORKFLOW_SHOW_YAML: &str = r#"name: review-def
description: Review repository changes before shipping.
inputs:
  files:
    type: string
    required: true
    description: Files to inspect
contracts:
  outputs:
    report:
      from_step: summarize
      format: json
      schema: review_report
steps:
  - id: fanout
    agent: reviewer
    actions: []
    for_each: "{{ inputs.files }}"
    max_parallel: 2
    prompt: "review {{ item }}"
  - id: summarize
    agent: writer
    actions: []
    prompt: "summarize findings"
"#;

fn init_git_checkout(path: &std::path::Path, origin_url: &str) {
    let status = Command::new("git")
        .arg("init")
        .arg("-b")
        .arg("main")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
    let status = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["remote", "add", "origin", origin_url])
        .status()
        .unwrap();
    assert!(status.success());
}

/// The codename column is additive: appended LAST (named `codename`,
/// matching the JSON key), so positional CSV consumers keep reading the
/// same columns they always did.
#[tokio::test]
async fn workflow_runs_csv_appends_codename_column_last() {
    let _guard = ENV_LOCK.lock().await;
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("runs").create_dir_all().unwrap();
    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["--format", "csv", "workflow", "runs"])
        .assert()
        .success()
        .stdout(predicates::str::starts_with(
            "run_id,status,started_at,duration_seconds,expires_in_seconds,total_tokens,cost_usd,workflow,codename\n",
        ));
}

#[tokio::test]
async fn workflow_list_shows_global_and_project() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("workflows").create_dir_all().unwrap();
    global
        .child("workflows/g-only.yaml")
        .write_str(WORKFLOW_YAML)
        .unwrap();

    let project = assert_fs::TempDir::new().unwrap();
    project.child(".rupu/workflows").create_dir_all().unwrap();
    project
        .child(".rupu/workflows/p-only.yaml")
        .write_str(WORKFLOW_YAML)
        .unwrap();

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_current_dir(project.path()).unwrap();

    let exit = rupu_cli::run(vec!["rupu".into(), "workflow".into(), "list".into()]).await;

    // Reset cwd to a stable path before the project tempdir is dropped.
    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "workflow list should exit 0"
    );
}

#[tokio::test]
async fn workflow_show_prints_yaml_body() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("workflows").create_dir_all().unwrap();
    global
        .child("workflows/hello-wf.yaml")
        .write_str(WORKFLOW_YAML)
        .unwrap();

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_current_dir(tmp.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "show".into(),
        "hello-wf".into(),
    ])
    .await;

    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "workflow show should exit 0 when workflow exists"
    );
}

#[tokio::test]
async fn workflow_show_defaults_to_full_and_supports_focused_compact_views() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("workflows").create_dir_all().unwrap();
    global
        .child("workflows/review-def.yaml")
        .write_str(WORKFLOW_SHOW_YAML)
        .unwrap();

    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        // These assert on plain text. The binary honors FORCE_COLOR (correctly)
        // even through a pipe, so a developer shell exporting it would embed
        // ANSI in every assertion here.
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .current_dir(tmp.path())
        .args(["workflow", "show", "review-def", "--no-color", "--no-pager"])
        .assert()
        .success()
        .stdout(predicates::str::contains("workflow show"))
        .stdout(predicates::str::contains("·  full"))
        .stdout(predicates::str::contains("steps  ·  declared steps"))
        .stdout(predicates::str::contains("yaml  ·  raw definition"))
        .stdout(predicates::str::contains("schema: review_report"));

    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        // These assert on plain text. The binary honors FORCE_COLOR (correctly)
        // even through a pipe, so a developer shell exporting it would embed
        // ANSI in every assertion here.
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .current_dir(tmp.path())
        .args([
            "workflow",
            "show",
            "review-def",
            "--view",
            "focused",
            "--no-color",
            "--no-pager",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("·  focused"))
        .stdout(predicates::str::contains("graph  ·  workflow structure"))
        .stdout(predicates::str::contains("for_each · runtime fan-out"))
        .stdout(predicates::str::contains("inputs  ·  declared inputs").not())
        .stdout(predicates::str::contains("raw definition").not());

    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        // These assert on plain text. The binary honors FORCE_COLOR (correctly)
        // even through a pipe, so a developer shell exporting it would embed
        // ANSI in every assertion here.
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .current_dir(tmp.path())
        .args([
            "workflow",
            "show",
            "review-def",
            "--view",
            "compact",
            "--no-color",
            "--no-pager",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("·  compact"))
        .stdout(predicates::str::contains("inputs  ·  declared inputs"))
        .stdout(predicates::str::contains("outputs  ·  declared outputs"))
        .stdout(predicates::str::contains("NAME"))
        .stdout(predicates::str::contains("DETAIL"))
        .stdout(predicates::str::contains("raw definition").not());
}

#[tokio::test]
async fn workflow_show_missing_exits_nonzero() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());
    std::env::set_current_dir(tmp.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "show".into(),
        "nope".into(),
    ])
    .await;

    std::env::remove_var("RUPU_HOME");

    assert_ne!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "workflow show for missing workflow should exit nonzero"
    );
}

#[tokio::test]
async fn workflow_run_executes_one_step_via_mock() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/echo.md")
        .write_str("---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nyou echo.")
        .unwrap();
    global.child("workflows").create_dir_all().unwrap();
    global
        .child("workflows/hello-wf.yaml")
        .write_str(WORKFLOW_YAML)
        .unwrap();

    let project = assert_fs::TempDir::new().unwrap();

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT);
    std::env::set_current_dir(project.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "run".into(),
        "hello-wf".into(),
        "--mode".into(),
        "bypass".into(),
    ])
    .await;

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "workflow run should exit 0 when the mock provider succeeds"
    );

    // A transcript file should now exist under <global>/transcripts/.
    let transcripts = global.child("transcripts");
    let entries: Vec<_> = std::fs::read_dir(transcripts.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one step transcript file"
    );
    let summary = rupu_transcript::JsonlReader::summary(entries[0].path()).unwrap();
    assert_eq!(summary.status, rupu_transcript::RunStatus::Ok);

    let run_store = rupu_orchestrator::RunStore::new(global.path().join("runs"));
    let runs = run_store.list().unwrap();
    assert_eq!(runs.len(), 1, "expected exactly one persisted workflow run");
    let envelope = run_store.read_run_envelope(&runs[0].id).unwrap();
    assert_eq!(
        envelope.trigger.source,
        rupu_runtime::RunTriggerSource::WorkflowCli
    );
    assert_eq!(envelope.workflow.name, "hello-wf");
    assert_eq!(envelope.execution.permission_mode, "bypass");
    assert_eq!(runs[0].backend_id.as_deref(), Some("local_worktree"));
    assert!(runs[0].worker_id.is_some(), "expected persisted worker id");
    assert!(
        runs[0].artifact_manifest_path.is_some(),
        "expected persisted artifact manifest path"
    );
    let manifest = run_store.read_artifact_manifest(&runs[0].id).unwrap();
    assert_eq!(manifest.run_id, runs[0].id);
    assert_eq!(manifest.backend_id, "local_worktree");
    assert_eq!(manifest.worker_id, runs[0].worker_id);
    assert!(manifest
        .artifacts
        .iter()
        .any(|artifact| artifact.kind == rupu_runtime::ArtifactKind::StepTranscript));

    let worker_store = rupu_workspace::WorkerStore {
        root: global.path().join("autoflows/workers"),
    };
    let workers = worker_store.list().unwrap();
    assert_eq!(workers.len(), 1, "expected exactly one persisted worker");
    assert_eq!(workers[0].worker_id, runs[0].worker_id.clone().unwrap());
}

#[tokio::test]
async fn workflow_run_supports_focused_and_full_view_modes() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/echo.md")
        .write_str("---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nyou echo.")
        .unwrap();
    global.child("workflows").create_dir_all().unwrap();
    global
        .child("workflows/hello-wf.yaml")
        .write_str(WORKFLOW_YAML)
        .unwrap();

    let project = assert_fs::TempDir::new().unwrap();

    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        // These assert on plain text. The binary honors FORCE_COLOR (correctly)
        // even through a pipe, so a developer shell exporting it would embed
        // ANSI in every assertion here.
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .env("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT)
        .current_dir(project.path())
        .args(["workflow", "run", "hello-wf", "--mode", "bypass"])
        .assert()
        .success()
        .stdout(predicates::str::contains("assistant output"));

    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        // These assert on plain text. The binary honors FORCE_COLOR (correctly)
        // even through a pipe, so a developer shell exporting it would embed
        // ANSI in every assertion here.
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .env("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT)
        .current_dir(project.path())
        .args([
            "workflow", "run", "hello-wf", "--mode", "bypass", "--view", "full",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("step output"))
        .stdout(predicates::str::contains("assistant output").not());
}

#[tokio::test]
async fn workflow_run_focused_mode_renders_fanout_tree() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/echo.md")
        .write_str("---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nyou echo.")
        .unwrap();
    global.child("workflows").create_dir_all().unwrap();
    global
        .child("workflows/fanout-wf.yaml")
        .write_str(FANOUT_WORKFLOW_YAML)
        .unwrap();

    let project = assert_fs::TempDir::new().unwrap();

    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        // These assert on plain text. The binary honors FORCE_COLOR (correctly)
        // even through a pipe, so a developer shell exporting it would embed
        // ANSI in every assertion here.
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .env("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT)
        .current_dir(project.path())
        .args([
            "workflow",
            "run",
            "fanout-wf",
            "--mode",
            "bypass",
            "--input",
            "files=[\"a.rs\",\"b.py\"]",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("for_each"))
        .stdout(predicates::str::contains("iter[1]"))
        .stdout(predicates::str::contains("iter[2]"))
        .stdout(predicates::str::contains("assistant output  step output"));
}

#[tokio::test]
async fn workflow_show_run_supports_pretty_and_json_output() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/echo.md")
        .write_str("---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nyou echo.")
        .unwrap();
    global.child("workflows").create_dir_all().unwrap();
    global
        .child("workflows/hello-wf.yaml")
        .write_str(WORKFLOW_YAML)
        .unwrap();

    let project = assert_fs::TempDir::new().unwrap();

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT);
    std::env::set_current_dir(project.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "run".into(),
        "hello-wf".into(),
        "--mode".into(),
        "bypass".into(),
    ])
    .await;
    assert_eq!(exit, std::process::ExitCode::from(0));

    let run_store = rupu_orchestrator::RunStore::new(global.path().join("runs"));
    let runs = run_store.list().unwrap();
    assert_eq!(runs.len(), 1);
    let run_id = runs[0].id.clone();

    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        // These assert on plain text. The binary honors FORCE_COLOR (correctly)
        // even through a pipe, so a developer shell exporting it would embed
        // ANSI in every assertion here.
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .current_dir(project.path())
        .args(["--format", "pretty", "workflow", "show-run", &run_id])
        .assert()
        .success()
        .stdout(predicates::str::contains("hello-wf"))
        .stdout(predicates::str::contains("workspace"))
        .stdout(predicates::str::contains("assistant output"))
        .stdout(predicates::str::contains("usage"))
        .stdout(predicates::str::contains("PROVIDER"))
        .stdout(predicates::str::contains("OUTPUT"));

    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        // These assert on plain text. The binary honors FORCE_COLOR (correctly)
        // even through a pipe, so a developer shell exporting it would embed
        // ANSI in every assertion here.
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .current_dir(project.path())
        .args(["--format", "json", "workflow", "show-run", &run_id])
        .assert()
        .success()
        .stdout(predicates::str::contains("\"kind\": \"workflow_show_run\""))
        .stdout(predicates::str::contains(format!(
            "\"run_id\": \"{run_id}\""
        )));

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::remove_var("RUPU_HOME");
}

#[tokio::test]
async fn workflow_show_run_full_renders_fanout_timeline() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/echo.md")
        .write_str("---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nyou echo.")
        .unwrap();
    global.child("workflows").create_dir_all().unwrap();
    global
        .child("workflows/fanout-wf.yaml")
        .write_str(FANOUT_WORKFLOW_YAML)
        .unwrap();

    let project = assert_fs::TempDir::new().unwrap();

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT);
    std::env::set_current_dir(project.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "run".into(),
        "fanout-wf".into(),
        "--mode".into(),
        "bypass".into(),
        "--input".into(),
        "files=[\"a.rs\",\"b.py\"]".into(),
    ])
    .await;
    assert_eq!(exit, std::process::ExitCode::from(0));

    let run_store = rupu_orchestrator::RunStore::new(global.path().join("runs"));
    let runs = run_store.list().unwrap();
    assert_eq!(runs.len(), 1);
    let run_id = runs[0].id.clone();

    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        // These assert on plain text. The binary honors FORCE_COLOR (correctly)
        // even through a pipe, so a developer shell exporting it would embed
        // ANSI in every assertion here.
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .current_dir(project.path())
        .args([
            "--format", "pretty", "workflow", "show-run", &run_id, "--view", "full",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("for_each"))
        .stdout(predicates::str::contains("iter[1]"))
        .stdout(predicates::str::contains("iter[2]"))
        .stdout(predicates::str::contains("inputs  ·  runtime inputs"))
        .stdout(predicates::str::contains("KEY"))
        .stdout(predicates::str::contains("VALUE"))
        .stdout(predicates::str::contains("assistant output"))
        .stdout(predicates::str::contains("step output"));

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::remove_var("RUPU_HOME");
}

#[tokio::test]
async fn workflow_run_auto_tracks_current_checkout() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/echo.md")
        .write_str("---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nyou echo.")
        .unwrap();
    global.child("workflows").create_dir_all().unwrap();
    global
        .child("workflows/hello-wf.yaml")
        .write_str(WORKFLOW_YAML)
        .unwrap();

    let project = assert_fs::TempDir::new().unwrap();
    init_git_checkout(project.path(), "git@github.com:Section9Labs/rupu.git");

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT);
    std::env::set_current_dir(project.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "run".into(),
        "hello-wf".into(),
        "--mode".into(),
        "bypass".into(),
    ])
    .await;

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::remove_var("RUPU_HOME");

    assert_eq!(exit, std::process::ExitCode::from(0));

    let store = rupu_workspace::RepoRegistryStore {
        root: global.path().join("repos"),
    };
    let tracked = store
        .load("github:Section9Labs/rupu")
        .unwrap()
        .expect("repo should be auto-tracked");
    assert_eq!(tracked.repo_ref, "github:Section9Labs/rupu");
    assert_eq!(tracked.known_paths.len(), 1);
    assert_eq!(
        tracked.preferred_path,
        project.path().canonicalize().unwrap().display().to_string()
    );
}

/// Parent step dispatches `child` via `dispatch_agent`. Both agent loops
/// replay this same two-turn script (the child's own `dispatch_agent`
/// call is rejected by its empty allowlist, then it finishes).
const DISPATCH_WORKFLOW_MOCK_SCRIPT: &str = r#"
[
  { "AssistantToolUse": { "text": null, "tool_id": "call_1", "tool_name": "dispatch_agent", "tool_input": {"agent": "child", "prompt": "please do the subtask"}, "stop": "tool_use" } },
  { "AssistantText": { "text": "All done.", "stop": "end_turn" } }
]
"#;

/// Wiring: `rupu workflow run` hands ONE `RunNaming` to both the
/// orchestrator and the `CliAgentDispatcher`. A sub-agent dispatched from
/// a step is named `<step codename>><role>#1`, `DispatchStarted` carries
/// that name plus the child's provider/model, the child's own transcript
/// `RunStart` carries it, and the dispatch counter lands in the run's
/// shared `codenames.json` (proving the dispatcher used the run's
/// persisted namer, not a private in-memory one).
#[tokio::test]
async fn workflow_run_names_dispatched_sub_agent_from_the_run_namer() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/parent.md")
        .write_str(
            "---\nname: parent\nprovider: anthropic\nmodel: claude-sonnet-4-6\n\
             maxTurns: 4\ntools: [dispatch_agent]\ndispatchableAgents: [child]\n---\n\
             you dispatch a child agent to do the subtask.",
        )
        .unwrap();
    global
        .child("agents/child.md")
        .write_str(
            "---\nname: child\nprovider: anthropic\nmodel: claude-sonnet-4-6\n\
             maxTurns: 4\n---\nyou are the child agent.",
        )
        .unwrap();
    global.child("workflows").create_dir_all().unwrap();
    global
        .child("workflows/dispatch-wf.yaml")
        .write_str(
            "name: dispatch-wf\nsteps:\n  - id: a\n    agent: parent\n    actions: []\n    prompt: go\n",
        )
        .unwrap();

    let project = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", DISPATCH_WORKFLOW_MOCK_SCRIPT);
    std::env::set_current_dir(project.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "run".into(),
        "dispatch-wf".into(),
        "--mode".into(),
        "bypass".into(),
    ])
    .await;

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "workflow run should exit 0"
    );

    let run_store = rupu_orchestrator::RunStore::new(global.path().join("runs"));
    let runs = run_store.list().unwrap();
    assert_eq!(runs.len(), 1);
    let run_id = runs[0].id.clone();
    let crew = rupu_codename::crew_for(&run_id);
    let child_role = rupu_codename::role_word("child");
    let run_dir = global.path().join("runs").join(&run_id);

    let events = std::fs::read_to_string(run_dir.join("events.jsonl")).unwrap();
    let started: Vec<serde_json::Value> = events
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["type"] == "dispatch_started")
        .collect();
    assert_eq!(started.len(), 1, "one dispatch, got {started:?}");
    let child = started[0]["codename"]
        .as_str()
        .expect("DispatchStarted must carry the child's codename")
        .to_string();
    assert!(
        child.starts_with(&format!("{crew}/")) && child.ends_with(&format!(">{child_role}#1")),
        "child codename {child:?} should be <{crew}/step>{child_role}#1"
    );
    assert_eq!(started[0]["provider"], "anthropic");
    assert_eq!(started[0]["model"], "claude-sonnet-4-6");

    // The child's own transcript opens with the same name.
    let child_transcript = started[0]["transcript_path"].as_str().unwrap();
    let run_start_codename = rupu_transcript::JsonlReader::iter(child_transcript)
        .unwrap()
        .filter_map(Result::ok)
        .find_map(|ev| match ev {
            rupu_transcript::Event::RunStart { codename, .. } => Some(codename),
            _ => None,
        })
        .expect("child transcript has a RunStart");
    assert_eq!(run_start_codename.as_deref(), Some(child.as_str()));

    // The dispatch counter was persisted by the run's shared namer.
    let parent_codename = child.rsplit_once('>').unwrap().0;
    let persisted = std::fs::read_to_string(run_dir.join("codenames.json"))
        .expect("the run's codenames.json exists");
    assert!(
        persisted.contains(&format!("{parent_codename}>{child_role}")),
        "codenames.json should record the dispatch counter: {persisted}"
    );
}
