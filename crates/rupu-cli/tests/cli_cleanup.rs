//! End-to-end tests for `rupu cleanup`.
//!
//! These tests mutate process-global state (`RUPU_HOME`, cwd). Hold
//! `ENV_LOCK` for the whole body of every test.

use assert_cmd::Command;
use chrono::{Duration, Utc};
use predicates::prelude::*;
use rupu_cli::standalone_run_metadata::{
    metadata_path_for_run, write_metadata, StandaloneRunMetadata,
};
use rupu_transcript::{Event, JsonlWriter, RunMode, RunStatus};
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

fn write_archived_session(home: &std::path::Path, session_id: &str, updated_at: &str) {
    let repo_root = home.parent().unwrap().join("repo");
    let transcripts_dir = repo_root.join(".rupu/transcripts");
    let archived_transcripts_dir = transcripts_dir.join("archive");
    std::fs::create_dir_all(&archived_transcripts_dir).unwrap();

    let transcript_path = archived_transcripts_dir.join("run_session_owned01.jsonl");
    std::fs::write(&transcript_path, "{}\n").unwrap();

    let dir = home.join("sessions-archive").join(session_id);
    std::fs::create_dir_all(&dir).unwrap();
    let payload = TestSession {
        version: 1,
        session_id: session_id.to_string(),
        agent_name: "writer".into(),
        description: "Archived session".into(),
        provider_name: "anthropic".into(),
        auth_mode: "api-key".into(),
        model: "claude-sonnet-4-6".into(),
        agent_system_prompt: "You are a test agent.".into(),
        agent_tools: vec!["read_file".into()],
        max_turns: 50,
        permission_mode: "readonly".into(),
        no_stream: false,
        workspace_id: "ws_test".into(),
        workspace_path: repo_root.display().to_string(),
        project_root: repo_root.display().to_string(),
        transcripts_dir: transcripts_dir.display().to_string(),
        repo_ref: "github:Section9Labs/rupu".into(),
        issue_ref: "github:Section9Labs/rupu/issues/42".into(),
        target: "github:Section9Labs/rupu/issues/42".into(),
        workspace_strategy: "direct_checkout".into(),
        created_at: updated_at.to_string(),
        updated_at: updated_at.to_string(),
        status: "idle".into(),
        active_run_id: None,
        active_transcript_path: None,
        active_pid: None,
        last_run_id: "run_session_owned01".into(),
        last_transcript_path: transcript_path.display().to_string(),
        last_error: None,
        total_turns: 1,
        total_tokens_in: 10,
        total_tokens_out: 5,
        message_history: Vec::new(),
        runs: vec![TestRun {
            run_id: "run_session_owned01".into(),
            prompt: "Summarize".into(),
            transcript_path: transcript_path.display().to_string(),
            started_at: updated_at.to_string(),
            completed_at: Some(updated_at.to_string()),
            status: Some("ok".into()),
            total_tokens_in: 10,
            total_tokens_out: 5,
            duration_ms: 100,
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

fn write_archived_standalone_transcript(
    home: &std::path::Path,
    run_id: &str,
    archived_at: &str,
) -> std::path::PathBuf {
    let dir = home.join("transcripts").join("archive");
    std::fs::create_dir_all(&dir).unwrap();
    let transcript_path = dir.join(format!("{run_id}.jsonl"));
    let mut w = JsonlWriter::create(&transcript_path).unwrap();
    w.write(&Event::RunStart {
        run_id: run_id.to_string(),
        workspace_id: "ws-test".to_string(),
        agent: "writer".to_string(),
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-6".to_string(),
        started_at: Utc::now(),
        mode: RunMode::Readonly,
    })
    .unwrap();
    w.write(&Event::RunComplete {
        run_id: run_id.to_string(),
        status: RunStatus::Ok,
        total_tokens: 12,
        duration_ms: 100,
        error: None,
    })
    .unwrap();
    w.flush().unwrap();

    let metadata_path = metadata_path_for_run(&dir, run_id);
    write_metadata(
        &metadata_path,
        &StandaloneRunMetadata {
            version: StandaloneRunMetadata::VERSION,
            run_id: run_id.to_string(),
            session_id: None,
            archived_at: Some(archived_at.to_string()),
            workspace_path: dir.clone(),
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
    transcript_path
}

#[tokio::test]
async fn cleanup_dry_run_reports_sessions_and_transcripts() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let old = (Utc::now() - Duration::days(40)).to_rfc3339();
    write_archived_session(&home, "ses_cleanup01", &old);
    write_archived_standalone_transcript(&home, "run_cleanup01", &old);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args([
            "--format",
            "json",
            "cleanup",
            "--dry-run",
            "--older-than",
            "0s",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"cleanup\""))
        .stdout(predicate::str::contains("\"id\": \"ses_cleanup01\""))
        .stdout(predicate::str::contains("\"id\": \"run_cleanup01\""))
        .stdout(predicate::str::contains("\"action\": \"would_delete\""));
}

#[tokio::test]
async fn cleanup_sessions_only_removes_archived_sessions() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let old = (Utc::now() - Duration::days(40)).to_rfc3339();
    write_archived_session(&home, "ses_cleanup02", &old);
    let transcript = write_archived_standalone_transcript(&home, "run_cleanup02", &old);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["cleanup", "--sessions", "--older-than", "0s"])
        .assert()
        .success();

    assert!(!home.join("sessions-archive").join("ses_cleanup02").exists());
    assert!(transcript.exists());
}

#[tokio::test]
async fn cleanup_transcripts_only_removes_archived_transcripts() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let old = (Utc::now() - Duration::days(40)).to_rfc3339();
    write_archived_session(&home, "ses_cleanup03", &old);
    let transcript = write_archived_standalone_transcript(&home, "run_cleanup03", &old);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["cleanup", "--transcripts", "--older-than", "0s"])
        .assert()
        .success();

    assert!(home.join("sessions-archive").join("ses_cleanup03").exists());
    assert!(!transcript.exists());
}

#[tokio::test]
async fn cleanup_stats_reports_resource_inventory() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let old = (Utc::now() - Duration::days(40)).to_rfc3339();
    write_archived_session(&home, "ses_cleanup04", &old);
    write_archived_standalone_transcript(&home, "run_cleanup04", &old);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["--format", "json", "cleanup", "--stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"cleanup_stats\""))
        .stdout(predicate::str::contains("\"scope\": \"archived\""))
        .stdout(predicate::str::contains("\"scope\": \"global_archived\""))
        .stdout(predicate::str::contains("\"count\": 1"));
}
