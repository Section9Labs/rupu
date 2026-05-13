//! End-to-end tests for `rupu transcript list | show`.
//!
//! These tests mutate process-global state (`RUPU_HOME`, cwd). Hold
//! `ENV_LOCK` for the whole body of every test to serialise them within
//! this binary.

use assert_cmd::Command;
use assert_fs::prelude::*;
use chrono::Utc;
use predicates::prelude::*;
use rupu_cli::standalone_run_metadata::{
    metadata_path_for_run, write_metadata, StandaloneRunMetadata,
};
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

fn write_metadata_sidecar(
    dir: &std::path::Path,
    run_id: &str,
    session_id: Option<&str>,
) -> std::path::PathBuf {
    let path = metadata_path_for_run(dir, run_id);
    write_metadata(
        &path,
        &StandaloneRunMetadata {
            version: StandaloneRunMetadata::VERSION,
            run_id: run_id.to_string(),
            session_id: session_id.map(str::to_string),
            archived_at: None,
            workspace_path: dir.to_path_buf(),
            project_root: None,
            repo_ref: None,
            issue_ref: None,
            backend_id: "local_checkout".into(),
            worker_id: None,
            trigger_source: "run_cli".into(),
            target: None,
            workspace_strategy: None,
        },
    )
    .unwrap();
    path
}

fn rewrite_archived_at(meta_path: &std::path::Path, archived_at: &str) {
    let mut payload: serde_json::Value =
        serde_json::from_slice(&std::fs::read(meta_path).unwrap()).unwrap();
    payload["archived_at"] = serde_json::Value::String(archived_at.to_string());
    std::fs::write(meta_path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
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
async fn show_supports_jsonl_output() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("transcripts").create_dir_all().unwrap();

    let transcripts_dir = global.path().join("transcripts");
    write_transcript(&transcripts_dir, "run_jsonl123", "jsonl-agent", 88);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["--format", "jsonl", "transcript", "show", "run_jsonl123"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"type\":\"run_start\""))
        .stdout(predicate::str::contains("\"type\":\"run_complete\""));
}

#[tokio::test]
async fn show_supports_pretty_output() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("transcripts").create_dir_all().unwrap();

    let transcripts_dir = global.path().join("transcripts");
    write_transcript(&transcripts_dir, "run_pretty123", "pretty-agent", 55);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["--format", "pretty", "transcript", "show", "run_pretty123"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pretty-agent"))
        .stdout(predicate::str::contains("run started"))
        .stdout(predicate::str::contains("run complete"));
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
            "run_id,scope,title,agent,status,total_tokens,started_at\n",
        ));
}

#[tokio::test]
async fn list_supports_archived_and_all_filters() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("transcripts").create_dir_all().unwrap();

    let transcripts_dir = global.path().join("transcripts");
    write_transcript(&transcripts_dir, "run_active01", "agent-a", 10);
    write_transcript(
        &transcripts_dir.join("archive"),
        "run_archived01",
        "agent-b",
        20,
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["--format", "json", "transcript", "list", "--archived"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"run_id\": \"run_archived01\""))
        .stdout(predicate::str::contains("\"scope\": \"archived\""))
        .stdout(predicate::str::contains("run_active01").not());

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["--format", "json", "transcript", "list", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"run_id\": \"run_active01\""))
        .stdout(predicate::str::contains("\"run_id\": \"run_archived01\""));
}

#[tokio::test]
async fn archive_moves_standalone_transcript_and_show_finds_it() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("transcripts").create_dir_all().unwrap();

    let transcripts_dir = global.path().join("transcripts");
    write_transcript(&transcripts_dir, "run_archive123", "archive-agent", 61);
    write_metadata_sidecar(&transcripts_dir, "run_archive123", None);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["transcript", "archive", "run_archive123"])
        .assert()
        .success();

    assert!(!transcripts_dir.join("run_archive123.jsonl").exists());
    assert!(transcripts_dir
        .join("archive/run_archive123.jsonl")
        .is_file());
    let meta_path = metadata_path_for_run(&transcripts_dir.join("archive"), "run_archive123");
    let archived_meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    assert!(archived_meta["archived_at"].is_string());

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["--format", "json", "transcript", "show", "run_archive123"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"run_id\": \"run_archive123\""));
}

#[tokio::test]
async fn delete_requires_force_and_refuses_session_managed_transcripts() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("transcripts").create_dir_all().unwrap();

    let transcripts_dir = global.path().join("transcripts");
    write_transcript(&transcripts_dir, "run_sessionowned", "archive-agent", 61);
    write_metadata_sidecar(&transcripts_dir, "run_sessionowned", Some("ses_owned01"));

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["transcript", "archive", "run_sessionowned"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("managed by session ses_owned01"));

    write_transcript(&transcripts_dir, "run_delete123", "archive-agent", 61);
    write_metadata_sidecar(&transcripts_dir, "run_delete123", None);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["transcript", "delete", "run_delete123"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires --force"));

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["transcript", "delete", "run_delete123", "--force"])
        .assert()
        .success();

    assert!(!transcripts_dir.join("run_delete123.jsonl").exists());
    assert!(!metadata_path_for_run(&transcripts_dir, "run_delete123").exists());
}

#[tokio::test]
async fn prune_deletes_old_archived_standalone_transcripts_and_uses_config_default() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global
        .child("transcripts/archive")
        .create_dir_all()
        .unwrap();
    std::fs::write(
        global.path().join("config.toml"),
        "[storage]\narchived_transcript_retention = \"7d\"\n",
    )
    .unwrap();

    let archived_dir = global.path().join("transcripts/archive");
    write_transcript(&archived_dir, "run_old01", "archive-agent", 61);
    let old_meta = write_metadata_sidecar(&archived_dir, "run_old01", None);
    rewrite_archived_at(
        &old_meta,
        &(Utc::now() - chrono::Duration::days(10)).to_rfc3339(),
    );

    write_transcript(&archived_dir, "run_new01", "archive-agent", 61);
    let new_meta = write_metadata_sidecar(&archived_dir, "run_new01", None);
    rewrite_archived_at(
        &new_meta,
        &(Utc::now() - chrono::Duration::days(2)).to_rfc3339(),
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["--format", "json", "transcript", "prune"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"run_id\": \"run_old01\""))
        .stdout(predicate::str::contains("\"action\": \"deleted\""));

    assert!(!archived_dir.join("run_old01.jsonl").exists());
    assert!(archived_dir.join("run_new01.jsonl").exists());
}
