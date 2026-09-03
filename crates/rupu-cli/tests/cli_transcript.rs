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

/// `write_transcript`, with an explicit `started_at` so a test can pin the
/// newest-first ordering the listing sorts by.
fn write_transcript_started_at(
    dir: &std::path::Path,
    run_id: &str,
    agent: &str,
    started_at: chrono::DateTime<Utc>,
) -> std::path::PathBuf {
    let path = dir.join(format!("{run_id}.jsonl"));
    let mut w = JsonlWriter::create(&path).unwrap();
    w.write(&Event::RunStart {
        run_id: run_id.to_string(),
        workspace_id: "ws-test".to_string(),
        agent: agent.to_string(),
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-6".to_string(),
        started_at,
        mode: RunMode::Bypass,
        schema: None,
        system_prompt: None,
    })
    .unwrap();
    w.write(&Event::RunComplete {
        run_id: run_id.to_string(),
        status: RunStatus::Ok,
        total_tokens: 10,
        duration_ms: 100,
        error: None,
    })
    .unwrap();
    w.flush().unwrap();
    path
}

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
        schema: None,
        system_prompt: None,
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

fn write_transcript_with_assistant(
    dir: &std::path::Path,
    run_id: &str,
    agent: &str,
    assistant_content: &str,
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
        schema: None,
        system_prompt: None,
    })
    .unwrap();
    w.write(&Event::AssistantMessage {
        content: assistant_content.to_string(),
        thinking: None,
    })
    .unwrap();
    w.write(&Event::RunComplete {
        run_id: run_id.to_string(),
        status: RunStatus::Ok,
        total_tokens: 123,
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
    write_metadata_sidecar_with_pid(dir, run_id, session_id, None)
}

/// Like [`write_metadata_sidecar`] but lets the caller set the liveness
/// `pid` — used by the I4 in-flight-run tests below (`Some(current pid)` to
/// simulate a still-running owner, `Some(dead pid)` for a finished one).
fn write_metadata_sidecar_with_pid(
    dir: &std::path::Path,
    run_id: &str,
    session_id: Option<&str>,
    pid: Option<u32>,
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
            pid,
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
async fn show_pretty_supports_focused_and_full_view_modes() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("transcripts").create_dir_all().unwrap();

    let transcripts_dir = global.path().join("transcripts");
    write_transcript_with_assistant(
        &transcripts_dir,
        "run_view123",
        "view-agent",
        "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega\nOMEGA-SENTINEL",
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["--format", "pretty", "transcript", "show", "run_view123"])
        .assert()
        .success()
        .stdout(predicate::str::contains("assistant output"))
        .stdout(predicate::str::contains("OMEGA-SENTINEL").not());

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args([
            "--format",
            "pretty",
            "transcript",
            "show",
            "run_view123",
            "--view",
            "compact",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("OMEGA-SENTI"))
        .stdout(predicate::str::contains("NEL"));

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args([
            "--format",
            "pretty",
            "transcript",
            "show",
            "run_view123",
            "--view",
            "full",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("OMEGA-SENTI"))
        .stdout(predicate::str::contains("NEL"));
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

// I4 (final-review finding): `rupu run` writes `.meta.json` BEFORE the agent
// loop starts, and standalone metadata carries no status field — so without
// a liveness guard, an in-flight run is archived/deleted mid-write just like
// a finished one. Both verbs must refuse while the metadata's `pid` is alive,
// and the on-disk files must survive the refused call.
#[tokio::test]
async fn archive_and_delete_refuse_a_live_standalone_run() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("transcripts").create_dir_all().unwrap();

    let transcripts_dir = global.path().join("transcripts");
    write_transcript(&transcripts_dir, "run_live01", "archive-agent", 61);
    // This test process is alive for the whole test body, so its own pid is
    // a reliable "still running" fixture across the child `rupu` process's
    // `kill -0` check (pids are a system-wide identifier, not confined to
    // whichever process is doing the checking).
    write_metadata_sidecar_with_pid(
        &transcripts_dir,
        "run_live01",
        None,
        Some(std::process::id()),
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["transcript", "archive", "run_live01"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("still running"));

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["transcript", "delete", "run_live01", "--force"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("still running"));

    // The refused calls must not have touched the on-disk transcript/metadata.
    assert!(transcripts_dir.join("run_live01.jsonl").is_file());
    assert!(metadata_path_for_run(&transcripts_dir, "run_live01").is_file());
}

// PID-reuse escape hatch: `--ignore-liveness` explicitly opts out of the I4
// guard above — the recovery path for a recorded pid that was reused by an
// unrelated process after a reboot/wraparound. With the flag set, a "live"
// pid no longer blocks archive/delete.
#[tokio::test]
async fn ignore_liveness_overrides_a_live_pid_refusal() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("transcripts").create_dir_all().unwrap();
    let transcripts_dir = global.path().join("transcripts");

    write_transcript(&transcripts_dir, "run_live_arch", "archive-agent", 61);
    write_metadata_sidecar_with_pid(
        &transcripts_dir,
        "run_live_arch",
        None,
        Some(std::process::id()),
    );
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["transcript", "archive", "run_live_arch", "--ignore-liveness"])
        .assert()
        .success();
    assert!(!transcripts_dir.join("run_live_arch.jsonl").exists());
    assert!(transcripts_dir
        .join("archive/run_live_arch.jsonl")
        .is_file());

    write_transcript(&transcripts_dir, "run_live_del", "archive-agent", 61);
    write_metadata_sidecar_with_pid(
        &transcripts_dir,
        "run_live_del",
        None,
        Some(std::process::id()),
    );
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args([
            "transcript",
            "delete",
            "run_live_del",
            "--force",
            "--ignore-liveness",
        ])
        .assert()
        .success();
    assert!(!transcripts_dir.join("run_live_del.jsonl").exists());
    assert!(!metadata_path_for_run(&transcripts_dir, "run_live_del").exists());
}

// `--ignore-liveness` is a liveness-only override — it must NOT reach the
// separate session-ownership guard (`ensure_standalone_transcript`), which
// has no override at all: a transcript actually owned by a session must
// still be refused even with a live pid AND `--ignore-liveness` set.
#[tokio::test]
async fn ignore_liveness_does_not_bypass_session_ownership_guard() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("transcripts").create_dir_all().unwrap();
    let transcripts_dir = global.path().join("transcripts");

    write_transcript(&transcripts_dir, "run_session_live", "archive-agent", 61);
    write_metadata_sidecar_with_pid(
        &transcripts_dir,
        "run_session_live",
        Some("ses_owned02"),
        Some(std::process::id()),
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args([
            "transcript",
            "archive",
            "run_session_live",
            "--ignore-liveness",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("managed by session ses_owned02"));

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args([
            "transcript",
            "delete",
            "run_session_live",
            "--force",
            "--ignore-liveness",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("managed by session ses_owned02"));

    assert!(transcripts_dir.join("run_session_live.jsonl").is_file());
    assert!(metadata_path_for_run(&transcripts_dir, "run_session_live").is_file());
}

// `--ignore-liveness` and `--force` are independent opt-outs for `delete`:
// the former skips the liveness check, the latter is the "confirm you mean
// it" gate. Neither substitutes for the other.
#[tokio::test]
async fn delete_ignore_liveness_without_force_still_refuses() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("transcripts").create_dir_all().unwrap();
    let transcripts_dir = global.path().join("transcripts");

    write_transcript(&transcripts_dir, "run_noforce", "archive-agent", 61);
    write_metadata_sidecar_with_pid(&transcripts_dir, "run_noforce", None, Some(999_999));

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["transcript", "delete", "run_noforce", "--ignore-liveness"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires --force"));

    assert!(transcripts_dir.join("run_noforce.jsonl").is_file());
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

/// `--limit` keeps the listing's cost tied to what is rendered rather than
/// to how much history is on disk: the scan sorts on each transcript's
/// `run_start` (its first record) and only then reads the survivors end to
/// end. This pins the observable half of that — the newest N rows, in
/// order — plus the hint that says how much was left out, because silently
/// showing 50 of several thousand would read as "that is all there is".
#[tokio::test]
async fn list_limit_returns_the_newest_rows_and_reports_the_remainder() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("transcripts").create_dir_all().unwrap();
    let dir = global.path().join("transcripts");

    let t = |secs: i64| Utc::now() - chrono::Duration::seconds(secs);
    write_transcript_started_at(&dir, "run_oldest", "agent-a", t(300));
    write_transcript_started_at(&dir, "run_middle", "agent-b", t(200));
    write_transcript_started_at(&dir, "run_newest", "agent-c", t(100));
    // A `run:` step transcript, which shares this directory but is not an
    // agent transcript — it must not occupy a row or a limit slot.
    std::fs::write(
        dir.join("run_step_side_by_side.jsonl"),
        b"{\"type\":\"RunStep\",\"cmd\":\"nmap\",\"exit_code\":0}\n",
    )
    .unwrap();

    let out = Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["--format", "json", "transcript", "list", "--limit", "2"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let rows = parsed["rows"].as_array().unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r["run_id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        vec!["run_newest", "run_middle"],
        "the two newest rows, newest first"
    );

    // Table output names what it left out; JSON above carries no such prose.
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["transcript", "list", "--limit", "2"])
        .assert()
        .success()
        .stdout(predicate::str::contains("showing 2 of 3 transcripts"));
}

/// The default limit must not silently hide a small workspace's history:
/// with fewer transcripts than the limit, every row is listed and no
/// truncation hint is printed.
#[tokio::test]
async fn list_below_the_limit_shows_everything_without_a_hint() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("transcripts").create_dir_all().unwrap();
    let dir = global.path().join("transcripts");
    write_transcript_started_at(&dir, "run_only_one", "agent-a", Utc::now());

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["transcript", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("run_only_one"))
        .stdout(predicate::str::contains("showing").not());
}
