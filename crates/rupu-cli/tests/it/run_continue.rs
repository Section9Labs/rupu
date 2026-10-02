//! `rupu run <agent> --continue <agent_run_id>` (recover-on-interrupt spec §1).

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::{Path, PathBuf};

fn make_agent(dir: &Path, name: &str) {
    let agents = dir.join(".rupu/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join(format!("{name}.md")),
        format!(
            "---\nname: {name}\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nYou are a test agent.\n"
        ),
    )
    .unwrap();
}

fn rupu(dir: &Path, script: &str) -> Command {
    let mut cmd = Command::cargo_bin("rupu").unwrap();
    cmd.current_dir(dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.join(".rupu"));
    cmd
}

fn transcript(dir: &Path, run_id: &str) -> PathBuf {
    dir.join(".rupu/transcripts")
        .join(format!("{run_id}.jsonl"))
}

/// Run `hello` once to completion as `run_first`.
fn first_run(dir: &Path) {
    rupu(
        dir,
        r#"[{"AssistantText":{"text":"first answer","stop":"end_turn"}}]"#,
    )
    .args([
        "run",
        "hello",
        "--mode",
        "bypass",
        "--run-id",
        "run_first",
        "say hi",
    ])
    .assert()
    .success();
    assert!(transcript(dir, "run_first").is_file());
}

/// Cut `run_first`'s transcript back to before its first turn — what a
/// runner killed while waiting on the model leaves behind.
fn interrupt_first_run(dir: &Path) {
    let path = transcript(dir, "run_first");
    let kept: String = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .take_while(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            v["type"] != "turn_start"
        })
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&path, kept).unwrap();
}

#[test]
fn continue_resumes_an_interrupted_run_seeded_by_reference() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    first_run(dir.path());
    interrupt_first_run(dir.path());

    rupu(
        dir.path(),
        r#"[{"AssistantText":{"text":"continued answer","stop":"end_turn"}}]"#,
    )
    .args([
        "run",
        "hello",
        "--mode",
        "bypass",
        "--run-id",
        "run_second",
        "--continue",
        "run_first",
    ])
    .assert()
    .success()
    .stdout(predicate::str::contains("continued answer"));

    let seed = std::fs::read_to_string(transcript(dir.path(), "run_second"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|v| v["type"] == "seed")
        .expect("the continued run seeds from the interrupted one");
    assert!(seed["data"]["source_transcript"]
        .as_str()
        .unwrap()
        .ends_with("run_first.jsonl"));
}

#[test]
fn continue_on_a_finished_run_prints_its_answer_without_a_model_call() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    first_run(dir.path());

    // An empty script: any model call would fail the run.
    rupu(dir.path(), "[]")
        .args([
            "run",
            "hello",
            "--mode",
            "bypass",
            "--run-id",
            "run_second",
            "--continue",
            "run_first",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("first answer"));
    assert!(!transcript(dir.path(), "run_second").exists());
}

#[test]
fn continue_refuses_a_run_of_a_different_agent() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    make_agent(dir.path(), "other");
    first_run(dir.path());
    interrupt_first_run(dir.path());

    rupu(dir.path(), "[]")
        .args([
            "run",
            "other",
            "--mode",
            "bypass",
            "--continue",
            "run_first",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("was agent `hello`"));
}

#[test]
fn continue_refuses_to_reuse_the_continued_runs_id() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    first_run(dir.path());
    interrupt_first_run(dir.path());
    let before = std::fs::read_to_string(transcript(dir.path(), "run_first")).unwrap();

    // An empty script: any model call would fail the run.
    rupu(dir.path(), "[]")
        .args([
            "run",
            "hello",
            "--mode",
            "bypass",
            "--run-id",
            "run_first",
            "--continue",
            "run_first",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("needs a new run id"));

    let after = std::fs::read_to_string(transcript(dir.path(), "run_first")).unwrap();
    assert_eq!(
        before, after,
        "the interrupted transcript must be untouched"
    );
}

#[test]
fn continue_refuses_an_unknown_run() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    rupu(dir.path(), "[]")
        .args([
            "run",
            "hello",
            "--mode",
            "bypass",
            "--continue",
            "run_missing",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("can't be read"));
}

#[test]
fn continue_does_not_take_a_prompt() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    rupu(dir.path(), "[]")
        .args(["run", "hello", "--continue", "run_first", "another prompt"])
        .assert()
        .failure();
}
