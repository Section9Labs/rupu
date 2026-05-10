//! End-to-end tests for `rupu workflow list | show | run`.
//!
//! These tests mutate process-global state (`RUPU_HOME`, cwd) and the
//! `RUPU_MOCK_PROVIDER_SCRIPT` env-var seam. Hold `ENV_LOCK` for the
//! whole body of every test to serialize them within this binary.

use assert_fs::prelude::*;
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
