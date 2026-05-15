use assert_cmd::Command as AssertCommand;
use assert_fs::prelude::*;
use predicates::prelude::*;
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

#[tokio::test]
async fn watch_help_lists_view_option() {
    let _guard = ENV_LOCK.lock().await;

    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .args(["watch", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--view <VIEW>"))
        .stdout(predicate::str::contains("focused"))
        .stdout(predicate::str::contains("compact"))
        .stdout(predicate::str::contains("full"));
}

#[tokio::test]
async fn watch_replay_accepts_view_and_renders_output() {
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
        .current_dir(project.path())
        .args(["watch", &run_id, "--replay", "--view", "full"])
        .assert()
        .success()
        .stdout(predicate::str::contains("step output"));

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::remove_var("RUPU_HOME");
}
