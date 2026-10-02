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

/// The runner truncates its transcript when it starts, so `--run-id` naming
/// ANY existing run — not just the one being continued — would wipe it.
#[test]
fn continue_refuses_a_run_id_that_already_has_a_transcript() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    first_run(dir.path());
    interrupt_first_run(dir.path());
    // A second, unrelated run that finished.
    rupu(
        dir.path(),
        r#"[{"AssistantText":{"text":"other answer","stop":"end_turn"}}]"#,
    )
    .args([
        "run",
        "hello",
        "--mode",
        "bypass",
        "--run-id",
        "run_other",
        "say hi again",
    ])
    .assert()
    .success();
    let before = std::fs::read_to_string(transcript(dir.path(), "run_other")).unwrap();

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
        "run_other",
        "--continue",
        "run_first",
    ])
    .assert()
    .failure()
    .stderr(predicate::str::contains("needs a new run id"))
    .stderr(predicate::str::contains("run_other"));

    let after = std::fs::read_to_string(transcript(dir.path(), "run_other")).unwrap();
    assert_eq!(
        before, after,
        "the other run's transcript must be untouched"
    );
}

/// Mark `run_id`'s transcript as ended in failure.
fn fail_run(dir: &Path, run_id: &str) {
    let path = transcript(dir, run_id);
    let body: String = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|l| {
            let mut v: serde_json::Value = serde_json::from_str(l).unwrap();
            if v["type"] == "run_complete" {
                v["data"]["status"] = "error".into();
                v["data"]["error"] = "max turns (5) reached".into();
            }
            format!("{v}\n")
        })
        .collect();
    std::fs::write(&path, body).unwrap();
}

#[test]
fn continue_on_a_failed_run_names_it_and_the_run_it_continued() {
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
    .success();
    fail_run(dir.path(), "run_second");

    rupu(dir.path(), "[]")
        .args([
            "run",
            "hello",
            "--mode",
            "bypass",
            "--continue",
            "run_second",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "run run_second ended in failure (max turns (5) reached)",
        ))
        .stderr(predicate::str::contains("start a fresh run instead"))
        .stderr(predicate::str::contains(
            "run run_first, which it continued, may still be continued with `--continue run_first`",
        ));

    // An ordinary failed run has no such run to point at.
    rupu(
        dir.path(),
        r#"[{"AssistantText":{"text":"plain answer","stop":"end_turn"}}]"#,
    )
    .args([
        "run",
        "hello",
        "--mode",
        "bypass",
        "--run-id",
        "run_plain",
        "say hi",
    ])
    .assert()
    .success();
    fail_run(dir.path(), "run_plain");
    rupu(dir.path(), "[]")
        .args([
            "run",
            "hello",
            "--mode",
            "bypass",
            "--continue",
            "run_plain",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("run run_plain ended in failure"))
        .stderr(predicate::str::contains("which it continued").not());
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

/// `--continue` re-enters an existing run, so it takes no target, prompt or
/// clone destination. The run being continued really exists here, so each
/// case can only fail on clap's conflict — not on a missing transcript.
#[test]
fn continue_conflicts_with_a_target_prompt_or_clone_destination() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    first_run(dir.path());
    interrupt_first_run(dir.path());
    let into = dir.path().join("clone-here");

    let cases: [(&[&str], &str); 4] = [
        (&["another prompt"], "'[TARGET]'"),
        (&["--prompt", "x"], "'--prompt <PROMPT_FLAG>'"),
        (&["--tmp"], "'--tmp'"),
        (&["--into", into.to_str().unwrap()], "'--into <PATH>'"),
    ];
    for (extra, conflicting) in cases {
        rupu(dir.path(), "[]")
            .args([
                "run",
                "hello",
                "--mode",
                "bypass",
                "--continue",
                "run_first",
            ])
            .args(extra)
            .assert()
            .failure()
            .stderr(predicate::str::contains("cannot be used with"))
            .stderr(predicate::str::contains(conflicting));
    }
}
