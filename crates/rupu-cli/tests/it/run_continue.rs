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

/// `--continue` re-enters an existing run, whose conversation holds its
/// prompt, so it takes no new one. The run being continued really exists
/// here, so each case can only fail on the prompt.
#[test]
fn continue_takes_no_prompt() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    first_run(dir.path());
    interrupt_first_run(dir.path());

    let cases: [(&[&str], &str); 2] = [
        (&["another prompt"], "takes no prompt"),
        (
            &["--prompt", "x"],
            "cannot be used with '--prompt <PROMPT_FLAG>'",
        ),
    ];
    for (extra, refusal) in cases {
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
            .stderr(predicate::str::contains(refusal));
    }
}

fn run_start(dir: &Path, run_id: &str) -> serde_json::Value {
    events(dir, run_id)
        .into_iter()
        .find(|v| v["type"] == "run_start")
        .unwrap()["data"]
        .clone()
}

fn events(dir: &Path, run_id: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(transcript(dir, run_id))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// `rupu run <agent> --continue <id>` with no other flag restores the whole
/// command the run was launched with: the continued run records the same
/// arguments (its own run id aside) and runs with the same mode, model,
/// codename, skills, findings profile and engagement, so it sends the same
/// system prompt and is offered the same tools.
#[test]
fn a_bare_continue_restores_the_original_command() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    for skill in ["x", "y"] {
        let skill_dir = dir.path().join(".rupu/skills").join(skill);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {skill}\ndescription: The {skill} skill.\n---\n\n{skill}-BODY\n"),
        )
        .unwrap();
    }
    let script = r#"[{"AssistantText":{"text":"ran","stop":"end_turn"}}]"#;
    rupu(dir.path(), script)
        .args([
            "run",
            "hello",
            "--mode",
            "readonly",
            "--run-id",
            "run_first",
            "--model",
            "claude-opus-4-7",
            "--provider",
            "anthropic",
            "--codename",
            "cobalt-harbor/heron#1",
            "--findings-profile",
            "summary",
            "--engagement-profile",
            "network",
            "--skill",
            "y",
            "--preload-skill",
            "x",
            "--no-stream",
            "--view",
            "full",
            "say hi",
        ])
        .assert()
        .success();
    interrupt_first_run(dir.path());

    // No `--mode`: without the restored `readonly`, a non-tty `ask` run
    // would refuse to start.
    rupu(dir.path(), script)
        .args([
            "run",
            "hello",
            "--run-id",
            "run_second",
            "--continue",
            "run_first",
        ])
        .assert()
        .success();

    let first = run_start(dir.path(), "run_first");
    let second = run_start(dir.path(), "run_second");
    assert_eq!(
        first["launch_args"],
        serde_json::json!([
            "hello",
            "--run-id=run_first",
            "--mode=readonly",
            "--view=full",
            "--codename=cobalt-harbor/heron#1",
            "--findings-profile=summary",
            "--model=claude-opus-4-7",
            "--provider=anthropic",
            "--engagement-profile=network",
            "--skill=y",
            "--preload-skill=x",
            "--no-stream",
        ])
    );
    let mut expected = first["launch_args"].clone();
    expected[1] = "--run-id=run_second".into();
    assert_eq!(second["launch_args"], expected);
    for key in [
        "mode",
        "model",
        "codename",
        "launch_skills",
        "launch_preload_skills",
    ] {
        assert_eq!(second[key], first[key], "{key}");
    }
    assert_eq!(second["system_prompt"], first["system_prompt"]);
    assert!(second["system_prompt"].as_str().unwrap().contains("x-BODY"));
    let tools = |run_id| {
        events(dir.path(), run_id)
            .into_iter()
            .find(|v| v["type"] == "tool_grant")
            .unwrap()["data"]
            .clone()
    };
    let granted = tools("run_second");
    assert_eq!(granted, tools("run_first"));
    assert!(
        granted.to_string().contains("assets.mark"),
        "the network engagement offers assets.mark: {granted}"
    );

    // A flag given on the continue replaces only its own setting.
    let path = transcript(dir.path(), "run_second");
    let kept: String = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .take_while(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["type"] != "turn_start"
        })
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&path, kept).unwrap();
    rupu(dir.path(), script)
        .args([
            "run",
            "hello",
            "--run-id",
            "run_third",
            "--continue",
            "run_second",
            "--findings-profile",
            "full",
        ])
        .assert()
        .success();
    let third = run_start(dir.path(), "run_third");
    let mut expected = first["launch_args"].clone();
    expected[1] = "--run-id=run_third".into();
    expected[5] = "--findings-profile=full".into();
    assert_eq!(third["launch_args"], expected);
    assert_eq!(third["mode"], first["mode"]);
}

/// A clone destination given on the continue is accepted; the run has no
/// repo target, so it changes nothing.
#[test]
fn continue_accepts_a_clone_destination() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    first_run(dir.path());
    interrupt_first_run(dir.path());
    let into = dir.path().join("clone-here");
    let cases: [&[&str]; 2] = [&["--tmp"], &["--into", into.to_str().unwrap()]];
    for (n, extra) in cases.into_iter().enumerate() {
        rupu(
            dir.path(),
            r#"[{"AssistantText":{"text":"continued","stop":"end_turn"}}]"#,
        )
        .args([
            "run",
            "hello",
            "--run-id",
            &format!("run_dest_{n}"),
            "--continue",
            "run_first",
        ])
        .args(extra)
        .assert()
        .success();
    }
}

/// A run whose provider never built (no credentials) records its command
/// too, so continuing it — on another provider, say, once one is set up —
/// restores its flags: here `--mode bypass`, without which a non-tty run
/// would refuse to start in `ask` mode.
#[test]
fn continue_restores_the_command_of_a_run_that_never_started() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    let mut first = Command::cargo_bin("rupu").unwrap();
    first
        .current_dir(dir.path())
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .env_remove("RUPU_MOCK_PROVIDER_SCRIPT")
        .env_remove("RUPU_AUTH_FILE")
        .env_remove("ANTHROPIC_API_KEY")
        .args([
            "run",
            "hello",
            "--mode",
            "bypass",
            "--run-id",
            "run_unstarted",
            "say hi",
        ])
        .assert()
        .failure();
    assert_eq!(
        run_start(dir.path(), "run_unstarted")["launch_args"],
        serde_json::json!(["hello", "--run-id=run_unstarted", "--mode=bypass"])
    );

    rupu(
        dir.path(),
        r#"[{"AssistantText":{"text":"continued answer","stop":"end_turn"}}]"#,
    )
    .args([
        "run",
        "hello",
        "--run-id",
        "run_after",
        "--continue",
        "run_unstarted",
        "--provider",
        "anthropic",
    ])
    .assert()
    .success();
    assert_eq!(run_start(dir.path(), "run_after")["mode"], "bypass");
}
