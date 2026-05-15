//! End-to-end tests for `rupu session ...` surfaces backed by persisted session state.
//!
//! These tests mutate process-global state (`RUPU_HOME`, cwd). Hold
//! `ENV_LOCK` for the whole body of every test to serialize them.

use assert_cmd::Command;
use chrono::Utc;
use predicates::prelude::*;
use serde::Serialize;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

#[derive(Serialize)]
struct TestRun {
    run_id: String,
    prompt: String,
    transcript_path: String,
    started_at: String,
    completed_at: Option<String>,
    status: Option<String>,
    total_tokens_in: u64,
    total_tokens_out: u64,
    duration_ms: u64,
    pid: Option<u32>,
    error: Option<String>,
}

#[derive(Serialize)]
struct TestSession {
    version: u32,
    session_id: String,
    agent_name: String,
    description: String,
    provider_name: String,
    auth_mode: String,
    model: String,
    agent_system_prompt: String,
    agent_tools: Vec<String>,
    max_turns: u32,
    permission_mode: String,
    no_stream: bool,
    workspace_id: String,
    workspace_path: String,
    project_root: String,
    transcripts_dir: String,
    repo_ref: String,
    issue_ref: String,
    target: String,
    workspace_strategy: String,
    created_at: String,
    updated_at: String,
    status: String,
    active_run_id: Option<String>,
    active_transcript_path: Option<String>,
    active_pid: Option<u32>,
    last_run_id: String,
    last_transcript_path: String,
    last_error: Option<String>,
    total_turns: u32,
    total_tokens_in: u64,
    total_tokens_out: u64,
    message_history: Vec<serde_json::Value>,
    runs: Vec<TestRun>,
}

fn write_session(
    home: &std::path::Path,
    session_id: &str,
    status: &str,
    active_pid: Option<u32>,
    archived: bool,
) {
    let repo_root = home.parent().unwrap().join("repo");
    let transcripts_dir = repo_root.join(".rupu/transcripts");
    std::fs::create_dir_all(&transcripts_dir).unwrap();
    let archived_transcripts_dir = transcripts_dir.join("archive");
    std::fs::create_dir_all(&archived_transcripts_dir).unwrap();
    let root_dir = if archived {
        "sessions-archive"
    } else {
        "sessions"
    };
    let dir = home.join(root_dir).join(session_id);
    std::fs::create_dir_all(&dir).unwrap();
    let now = Utc::now().to_rfc3339();
    let prev_transcript_path = if archived {
        archived_transcripts_dir.join("run_prev123.jsonl")
    } else {
        transcripts_dir.join("run_prev123.jsonl")
    };
    std::fs::write(&prev_transcript_path, "{}\n").unwrap();
    let live_transcript_path = if archived {
        archived_transcripts_dir.join("run_live123.jsonl")
    } else {
        transcripts_dir.join("run_live123.jsonl")
    };
    if status == "running" {
        std::fs::write(&live_transcript_path, "{}\n").unwrap();
    }
    let active_run_id = if status == "running" {
        Some("run_live123".to_string())
    } else {
        None
    };
    let active_transcript_path = if status == "running" {
        Some(live_transcript_path.display().to_string())
    } else {
        None
    };
    let payload = TestSession {
        version: 1,
        session_id: session_id.to_string(),
        agent_name: "issue-reader".into(),
        description: "Persistent issue reader".into(),
        provider_name: "anthropic".into(),
        auth_mode: "api-key".into(),
        model: "claude-sonnet-4-6".into(),
        agent_system_prompt: "You are a test agent.".into(),
        agent_tools: vec!["read_file".into()],
        max_turns: 50,
        permission_mode: "bypass".into(),
        no_stream: false,
        workspace_id: "ws_test".into(),
        workspace_path: repo_root.display().to_string(),
        project_root: repo_root.display().to_string(),
        transcripts_dir: transcripts_dir.display().to_string(),
        repo_ref: "github:Section9Labs/rupu".into(),
        issue_ref: "github:Section9Labs/rupu/issues/42".into(),
        target: "github:Section9Labs/rupu/issues/42".into(),
        workspace_strategy: "direct_checkout".into(),
        created_at: now.clone(),
        updated_at: now.clone(),
        status: status.into(),
        active_run_id,
        active_transcript_path,
        active_pid,
        last_run_id: "run_prev123".into(),
        last_transcript_path: prev_transcript_path.display().to_string(),
        last_error: None,
        total_turns: 3,
        total_tokens_in: 120,
        total_tokens_out: 45,
        message_history: Vec::new(),
        runs: vec![TestRun {
            run_id: "run_prev123".into(),
            prompt: "Summarize the issue.".into(),
            transcript_path: "/tmp/repo/.rupu/transcripts/run_prev123.jsonl".into(),
            started_at: now.clone(),
            completed_at: Some(now),
            status: Some("ok".into()),
            total_tokens_in: 100,
            total_tokens_out: 40,
            duration_ms: 1234,
            pid: None,
            error: None,
        }],
    };
    std::fs::write(
        dir.join("session.json"),
        serde_json::to_vec_pretty(&payload).unwrap(),
    )
    .unwrap();
}

fn rewrite_session_updated_at(
    home: &std::path::Path,
    root: &str,
    session_id: &str,
    updated_at: &str,
) {
    let path = home.join(root).join(session_id).join("session.json");
    let mut payload: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    payload["updated_at"] = serde_json::Value::String(updated_at.to_string());
    std::fs::write(&path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
}

#[tokio::test]
async fn session_list_supports_json_output() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    write_session(&home, "ses_01", "idle", None, false);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["--format", "json", "session", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"session_list\""))
        .stdout(predicate::str::contains("\"session_id\": \"ses_01\""));
}

#[tokio::test]
async fn session_show_supports_json_output() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    write_session(&home, "ses_show01", "idle", None, false);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["--format", "json", "session", "show", "ses_show01"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"session_show\""))
        .stdout(predicate::str::contains("\"agent\": \"issue-reader\""))
        .stdout(predicate::str::contains("\"session_id\": \"ses_show01\""));
}

#[tokio::test]
async fn session_show_supports_focused_compact_and_full_views() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    write_session(&home, "ses_show_view01", "idle", None, false);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["session", "show", "ses_show_view01"])
        .assert()
        .success()
        .stdout(predicate::str::contains("session show"))
        .stdout(predicate::str::contains("runs  ·  recent turns"))
        .stdout(predicate::str::contains("transcript /tmp/repo/.rupu/transcripts/run_prev123.jsonl").not());

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["session", "show", "ses_show_view01", "--view", "compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains("·  compact"))
        .stdout(predicate::str::contains("transcript /tmp/repo/.rupu/transcripts/run_prev123.jsonl"));

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["session", "show", "ses_show_view01", "--view", "full"])
        .assert()
        .success()
        .stdout(predicate::str::contains("·  full"))
        .stdout(predicate::str::contains("completed "))
        .stdout(predicate::str::contains("1234ms"));
}

#[tokio::test]
async fn session_list_reconciles_stale_running_workers() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    write_session(&home, "ses_stale01", "running", Some(999_999), false);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["--format", "json", "session", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\": \"failed\""));

    let persisted: serde_json::Value = serde_json::from_slice(
        &std::fs::read(home.join("sessions/ses_stale01/session.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(persisted["status"], "failed");
    assert!(persisted["active_run_id"].is_null());
}

#[tokio::test]
async fn session_archive_restore_round_trip_moves_owned_transcripts() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    write_session(&home, "ses_archive01", "idle", None, false);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["session", "archive", "ses_archive01"])
        .assert()
        .success();

    assert!(home.join("sessions/ses_archive01").is_dir() == false);
    assert!(home
        .join("sessions-archive/ses_archive01/session.json")
        .is_file());
    assert!(tmp
        .path()
        .join("repo/.rupu/transcripts/archive/run_prev123.jsonl")
        .is_file());

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["--format", "json", "session", "list", "--archived"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"session_id\": \"ses_archive01\"",
        ))
        .stdout(predicate::str::contains("\"scope\": \"archived\""));

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["session", "restore", "ses_archive01"])
        .assert()
        .success();

    assert!(home.join("sessions/ses_archive01/session.json").is_file());
    assert!(tmp
        .path()
        .join("repo/.rupu/transcripts/run_prev123.jsonl")
        .is_file());
}

#[tokio::test]
async fn session_delete_requires_force_and_removes_files() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    write_session(&home, "ses_delete01", "idle", None, false);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["session", "delete", "ses_delete01"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires --force"));

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["session", "delete", "ses_delete01", "--force"])
        .assert()
        .success();

    assert!(!home.join("sessions/ses_delete01").exists());
    assert!(!tmp
        .path()
        .join("repo/.rupu/transcripts/run_prev123.jsonl")
        .exists());
}

#[tokio::test]
async fn session_delete_refuses_running_worker() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    write_session(
        &home,
        "ses_running01",
        "running",
        Some(std::process::id()),
        false,
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["session", "delete", "ses_running01", "--force"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot delete session"));
}

#[tokio::test]
async fn session_prune_removes_archived_sessions_older_than_cutoff() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    write_session(&home, "ses_old01", "idle", None, true);
    write_session(&home, "ses_new01", "idle", None, true);
    rewrite_session_updated_at(
        &home,
        "sessions-archive",
        "ses_old01",
        &(Utc::now() - chrono::Duration::days(45)).to_rfc3339(),
    );
    rewrite_session_updated_at(
        &home,
        "sessions-archive",
        "ses_new01",
        &(Utc::now() - chrono::Duration::days(2)).to_rfc3339(),
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args([
            "--format",
            "json",
            "session",
            "prune",
            "--older-than",
            "30d",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"session_id\": \"ses_old01\""))
        .stdout(predicate::str::contains("\"action\": \"deleted\""));

    assert!(!home.join("sessions-archive/ses_old01").exists());
    assert!(home.join("sessions-archive/ses_new01").exists());
}

#[tokio::test]
async fn session_prune_dry_run_uses_config_default() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("config.toml"),
        "[storage]\narchived_session_retention = \"7d\"\n",
    )
    .unwrap();
    write_session(&home, "ses_cfg01", "idle", None, true);
    rewrite_session_updated_at(
        &home,
        "sessions-archive",
        "ses_cfg01",
        &(Utc::now() - chrono::Duration::days(10)).to_rfc3339(),
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["--format", "json", "session", "prune", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"session_id\": \"ses_cfg01\""))
        .stdout(predicate::str::contains("\"action\": \"would_delete\""));

    assert!(home.join("sessions-archive/ses_cfg01").exists());
}
