use assert_fs::prelude::*;
use std::process::Command;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

const MOCK_SCRIPT: &str = r#"
[
  { "AssistantText": { "text": "Hello from the mock provider.", "stop": "end_turn" } }
]
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

#[tokio::test(flavor = "multi_thread")]
async fn rupu_run_writes_transcript_under_mock_provider() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.create_dir_all().unwrap();
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/echo.md")
        .write_str(
            "---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 1\n---\nyou echo.",
        )
        .unwrap();

    let project = assert_fs::TempDir::new().unwrap();

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT);
    // Force PWD to project so workspace discovery uses it.
    std::env::set_current_dir(project.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "run".into(),
        "echo".into(),
        "--mode".into(),
        "bypass".into(),
        "say hi".into(),
    ])
    .await;

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "rupu run should exit 0 when the mock provider succeeds"
    );

    // Find the transcript file written under <global>/transcripts/<run_id>.jsonl
    let transcripts = global.child("transcripts");
    let entries: Vec<_> = std::fs::read_dir(transcripts.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1, "expected exactly one transcript file");
    let summary = rupu_transcript::JsonlReader::summary(entries[0].path()).unwrap();
    assert_eq!(summary.status, rupu_transcript::RunStatus::Ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn rupu_run_unknown_agent_exits_nonzero() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    let exit = rupu_cli::run(vec!["rupu".into(), "run".into(), "nonexistent".into()]).await;
    std::env::remove_var("RUPU_HOME");
    assert_ne!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "unknown agent should not exit 0"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rupu_run_auto_tracks_current_checkout() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.create_dir_all().unwrap();
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/echo.md")
        .write_str(
            "---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 1\n---\nyou echo.",
        )
        .unwrap();

    let project = assert_fs::TempDir::new().unwrap();
    init_git_checkout(project.path(), "git@github.com:Section9Labs/rupu.git");

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT);
    std::env::set_current_dir(project.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "run".into(),
        "echo".into(),
        "--mode".into(),
        "bypass".into(),
        "say hi".into(),
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
