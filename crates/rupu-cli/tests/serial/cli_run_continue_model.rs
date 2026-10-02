//! `rupu run` records a run that ends badly as failed and exits non-zero, and
//! `rupu run --continue <id> --model <model>` picks a failed run up on another
//! model (spec 2026-10-01 response-outcomes §8).

use crate::ENV_LOCK;
use assert_cmd::Command;
use predicates::prelude::*;
use std::path::{Path, PathBuf};

/// The agent's own model refuses; `mock-2` answers.
const MOCK_SCRIPT: &str = r#"
{
  "turns": [
    { "Reply": {
        "content": [{ "type": "text", "text": "I won't do that." }],
        "stop": { "reason": "refusal", "wire": { "provider": "anthropic", "value": "refusal" } }
    } }
  ],
  "models": {
    "mock-2": [
      { "AssistantText": { "text": "Done on mock-2.", "stop": "end_turn" } }
    ]
  }
}
"#;

fn make_agent(dir: &Path, name: &str, extra: &str) {
    let agents = dir.join(".rupu/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join(format!("{name}.md")),
        format!(
            "---\nname: {name}\nprovider: anthropic\nmodel: claude-sonnet-4-6\n{extra}---\nyou answer.\n"
        ),
    )
    .unwrap();
}

fn rupu(dir: &Path, script: &str) -> Command {
    let mut cmd = Command::cargo_bin("rupu").unwrap();
    cmd.current_dir(dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.join(".rupu"))
        .write_stdin("");
    cmd
}

fn transcript(dir: &Path, run_id: &str) -> PathBuf {
    dir.join(".rupu/transcripts")
        .join(format!("{run_id}.jsonl"))
}

fn events(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn run_record(dir: &Path, run_id: &str) -> rupu_orchestrator::RunRecord {
    rupu_orchestrator::RunStore::new(dir.join(".rupu/runs"))
        .load(run_id)
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_run_fails_and_continues_on_another_model() {
    let _guard = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "guarded", "maxTurns: 3\n");

    // Run 1: the refusal has no fallback chain to go to.
    rupu(dir.path(), MOCK_SCRIPT)
        .args([
            "run",
            "guarded",
            "--mode",
            "bypass",
            "--run-id",
            "run_refused",
            "do the thing",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("refused"))
        .stderr(predicate::str::contains("--continue run_refused --model"));
    let rec = run_record(dir.path(), "run_refused");
    assert_eq!(rec.status, rupu_orchestrator::RunStatus::Failed);
    assert!(
        rec.error_message
            .as_deref()
            .is_some_and(|e| e.starts_with("refused")),
        "{:?}",
        rec.error_message
    );
    assert_eq!(
        rec.cause.as_ref().map(|c| c.class.as_str()),
        Some("refusal")
    );
    assert_eq!(rec.final_output, None);

    // Without --model, a failed run is still not continued.
    rupu(dir.path(), MOCK_SCRIPT)
        .args([
            "run",
            "guarded",
            "--mode",
            "bypass",
            "--continue",
            "run_refused",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("ended in failure"));

    // Run 2: continue it on mock-2.
    rupu(dir.path(), MOCK_SCRIPT)
        .args([
            "run",
            "guarded",
            "--mode",
            "bypass",
            "--run-id",
            "run_retry",
            "--continue",
            "run_refused",
            "--model",
            "mock-2",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Done on mock-2."));

    let second = events(&transcript(dir.path(), "run_retry"));
    let seed = second
        .iter()
        .find(|v| v["type"] == "seed")
        .expect("the continued run seeds from the failed one");
    let source = seed["data"]["source_transcript"].as_str().unwrap();
    assert!(source.ends_with("/run_refused.jsonl"), "{source}");
    let note = second
        .iter()
        .find(|v| v["type"] == "user_message")
        .and_then(|v| v["data"]["content"].as_str())
        .expect("the continued run sends a note")
        .to_string();
    assert!(
        note.contains("A previous attempt at this task stopped (refused"),
        "{note}"
    );
    assert!(note.contains("anthropic/mock-2"), "{note}");
    let start = second
        .iter()
        .find(|v| v["type"] == "run_start")
        .expect("run_start");
    assert_eq!(start["data"]["model"], "mock-2");
    assert_eq!(
        run_record(dir.path(), "run_retry").status,
        rupu_orchestrator::RunStatus::Completed
    );
}

/// A max-turns bust returns `Ok` from the runner with a failed status: the
/// run is recorded failed and the command exits non-zero.
#[tokio::test(flavor = "multi_thread")]
async fn a_max_turns_bust_fails_the_command() {
    let _guard = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "looper", "maxTurns: 1\n");
    std::fs::write(dir.path().join("notes.txt"), "alpha\n").unwrap();
    let script = r#"[
      { "AssistantToolUse": {
          "text": null, "tool_id": "call_1", "tool_name": "read_file",
          "tool_input": { "path": "notes.txt" }, "stop": "tool_use"
      } }
    ]"#;
    rupu(dir.path(), script)
        .args([
            "run",
            "looper",
            "--mode",
            "bypass",
            "--run-id",
            "run_bust",
            "read the notes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("max turns (1) reached"));
    let rec = run_record(dir.path(), "run_bust");
    assert_eq!(rec.status, rupu_orchestrator::RunStatus::Failed);
    assert_eq!(rec.error_message.as_deref(), Some("max turns (1) reached"));
    assert_eq!(rec.cause, None);
}

/// `--model` and `--provider` override the agent for an ordinary run too.
#[tokio::test(flavor = "multi_thread")]
async fn model_and_provider_flags_override_the_agent() {
    let _guard = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "guarded", "");
    rupu(dir.path(), MOCK_SCRIPT)
        .args([
            "run",
            "guarded",
            "--mode",
            "bypass",
            "--run-id",
            "run_override",
            "--provider",
            "openai",
            "--model",
            "mock-2",
            "do the thing",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Done on mock-2."));
    let start = events(&transcript(dir.path(), "run_override"))
        .into_iter()
        .find(|v| v["type"] == "run_start")
        .expect("run_start");
    assert_eq!(start["data"]["model"], "mock-2");
    assert_eq!(start["data"]["provider"], "openai");
}
