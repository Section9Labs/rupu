//! End-to-end tests for `rupu transcript list | show`.
//!
//! These tests mutate process-global state (`RUPU_HOME`, cwd). Hold
//! `ENV_LOCK` for the whole body of every test to serialise them within
//! this binary.

use assert_cmd::Command;
use assert_fs::prelude::*;
use chrono::Utc;
use predicates::prelude::*;
use rupu_transcript::{Event, JsonlWriter, RunMode, RunStatus};
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

/// Write a minimal but valid two-event transcript (RunStart + RunComplete)
/// to `dir/<run_id>.jsonl`.
fn write_transcript(
    dir: &std::path::Path,
    run_id: &str,
    agent: &str,
    total_tokens: u64,
) -> std::path::PathBuf {
    let path = dir.join(format!("{run_id}.jsonl"));
    let mut w = JsonlWriter::create(&path).unwrap();
    w.write(&Event::RunStart {
        run_id: run_id.to_string(),
        workspace_id: "ws-test".to_string(),
        agent: agent.to_string(),
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-6".to_string(),
        started_at: Utc::now(),
        mode: RunMode::Bypass,
    })
    .unwrap();
    w.write(&Event::RunComplete {
        run_id: run_id.to_string(),
        status: RunStatus::Ok,
        total_tokens,
        duration_ms: 100,
        error: None,
    })
    .unwrap();
    w.flush().unwrap();
    path
}

#[tokio::test]
async fn list_shows_recent_transcripts() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("transcripts").create_dir_all().unwrap();

    let transcripts_dir = global.path().join("transcripts");
    write_transcript(&transcripts_dir, "run_aaa111", "agent-a", 100);
    write_transcript(&transcripts_dir, "run_bbb222", "agent-b", 200);

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_current_dir(tmp.path()).unwrap();

    let exit = rupu_cli::run(vec!["rupu".into(), "transcript".into(), "list".into()]).await;

    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "transcript list should exit 0"
    );
}

#[tokio::test]
async fn show_prints_run_events() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("transcripts").create_dir_all().unwrap();

    let transcripts_dir = global.path().join("transcripts");
    write_transcript(&transcripts_dir, "run_show999", "test-agent", 42);

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_current_dir(tmp.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "transcript".into(),
        "show".into(),
        "run_show999".into(),
    ])
    .await;

    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "transcript show should exit 0 when run_id exists"
    );
}

#[tokio::test]
async fn show_missing_run_id_exits_nonzero() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());
    std::env::set_current_dir(tmp.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "transcript".into(),
        "show".into(),
        "run_does_not_exist".into(),
    ])
    .await;

    std::env::remove_var("RUPU_HOME");

    assert_ne!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "transcript show for missing run_id should exit nonzero"
    );
}

#[tokio::test]
async fn show_supports_json_output() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("transcripts").create_dir_all().unwrap();

    let transcripts_dir = global.path().join("transcripts");
    write_transcript(&transcripts_dir, "run_json123", "json-agent", 77);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["--format", "json", "transcript", "show", "run_json123"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"transcript_show\""))
        .stdout(predicate::str::contains("\"run_id\": \"run_json123\""))
        .stdout(predicate::str::contains("\"events\""));
}

#[tokio::test]
async fn list_csv_with_no_rows_emits_headers() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("transcripts").create_dir_all().unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["--format", "csv", "transcript", "list"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(
            "run_id,title,agent,status,total_tokens,started_at\n",
        ));
}
