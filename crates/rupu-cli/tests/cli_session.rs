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

fn write_session(home: &std::path::Path, session_id: &str, status: &str, active_pid: Option<u32>) {
    let dir = home.join("sessions").join(session_id);
    std::fs::create_dir_all(&dir).unwrap();
    let now = Utc::now().to_rfc3339();
    let active_run_id = if status == "running" {
        Some("run_live123".to_string())
    } else {
        None
    };
    let active_transcript_path = if status == "running" {
        Some("/tmp/repo/.rupu/transcripts/run_live123.jsonl".to_string())
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
        workspace_path: "/tmp/repo".into(),
        project_root: "/tmp/repo".into(),
        transcripts_dir: "/tmp/repo/.rupu/transcripts".into(),
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
        last_transcript_path: "/tmp/repo/.rupu/transcripts/run_prev123.jsonl".into(),
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

#[tokio::test]
async fn session_list_supports_json_output() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    write_session(&home, "ses_01", "idle", None);

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
    write_session(&home, "ses_show01", "idle", None);

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
async fn session_list_reconciles_stale_running_workers() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    write_session(&home, "ses_stale01", "running", Some(999_999));

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
