//! End-to-end tests for the line-stream output path.
//!
//! These tests exercise `rupu run` via the mock provider and assert on
//! the streaming vertical-timeline output produced by `LineStreamPrinter`.
//! All color codes are stripped because the test process does not have
//! a TTY (owo-colors degrades automatically on non-TTY stdout).

use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;

fn make_agent(dir: &std::path::Path, name: &str) {
    let agent_dir = dir.join(".rupu/agents");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let agent_path = agent_dir.join(format!("{name}.md"));
    let mut f = std::fs::File::create(&agent_path).unwrap();
    writeln!(f, "---").unwrap();
    writeln!(f, "name: {name}").unwrap();
    writeln!(f, "provider: anthropic").unwrap();
    writeln!(f, "model: claude-sonnet-4-6").unwrap();
    writeln!(f, "---").unwrap();
    writeln!(f, "You are a test agent.").unwrap();
}

/// `rupu run` in line-stream mode emits the agent name and provider in
/// the header (`▶ <agent>  (<provider> · <model>)  <run_id>`).
#[test]
fn run_line_stream_header_format() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "test-agent");

    let script = r#"[{"AssistantText":{"text":"done","stop":"end_turn"}}]"#;

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args(["run", "test-agent", "--mode", "bypass", "go"])
        .assert()
        .success()
        // Header glyph.
        .stdout(predicate::str::contains("▶"))
        // Agent name.
        .stdout(predicate::str::contains("test-agent"))
        // Provider.
        .stdout(predicate::str::contains("anthropic"))
        // Model.
        .stdout(predicate::str::contains("claude-sonnet-4-6"));
}

/// Line-stream mode shows assistant content inline.
#[test]
fn run_line_stream_shows_assistant_content() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "reply-agent");

    let script = r#"[{"AssistantText":{"text":"hello from agent","stop":"end_turn"}}]"#;

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args(["run", "reply-agent", "--mode", "bypass", "go"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello from agent"));
}

/// Token count appears in the step footer (`Xs · <N> tokens`).
#[test]
fn run_line_stream_token_count_in_footer() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "token-agent");

    // 30 input + 5 output = 35 total.
    let script =
        r#"[{"AssistantText":{"text":"x","stop":"end_turn","input_tokens":30,"output_tokens":5}}]"#;

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args(["run", "token-agent", "--mode", "bypass", "go"])
        .assert()
        .success()
        // Footer shows total token count.
        .stdout(predicate::str::contains("35 tokens"));
}

/// `rupu run` prints no alt-screen escape sequences.
/// The simplest check is that the output contains printable ASCII /
/// Unicode only (no ESC char 0x1B) when stdout is non-TTY.
#[test]
fn run_line_stream_no_ansi_on_pipe() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "clean-agent");

    let script = r#"[{"AssistantText":{"text":"plain","stop":"end_turn"}}]"#;

    let output = Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args(["run", "clean-agent", "--mode", "bypass", "go"])
        .output()
        .unwrap();

    assert!(output.status.success(), "rupu run failed");
    // No ESC byte (\x1b) in stdout when non-TTY.
    assert!(
        !output.stdout.contains(&0x1b),
        "unexpected ANSI escape in stdout"
    );
}

/// The step-start spinner and completion marker appear in output.
#[test]
fn run_line_stream_step_glyphs() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "glyph-agent");

    let script = r#"[{"AssistantText":{"text":"ok","stop":"end_turn"}}]"#;

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args(["run", "glyph-agent", "--mode", "bypass", "go"])
        .assert()
        .success()
        // Running glyph in step_start line.
        .stdout(predicate::str::contains("◐"))
        // Done glyph in step_done line.
        .stdout(predicate::str::contains("✓"));
}

/// Transcript path footer is still printed after the run.
#[test]
fn run_line_stream_transcript_footer() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "tx-agent");

    let script = r#"[{"AssistantText":{"text":"ok","stop":"end_turn"}}]"#;

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args(["run", "tx-agent", "--mode", "bypass", "go"])
        .assert()
        .success()
        .stdout(predicate::str::contains("transcript:"));
}

#[test]
fn run_line_stream_supports_focused_and_full_view_modes() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "view-agent");

    let script = r#"[{"AssistantText":{"text":"OMEGA-SENTINEL from agent","stop":"end_turn"}}]"#;

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args([
            "run",
            "view-agent",
            "--mode",
            "bypass",
            "--view",
            "focused",
            "go",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("assistant output"));

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args([
            "run",
            "view-agent",
            "--mode",
            "bypass",
            "--view",
            "full",
            "go",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("OMEGA-SENTINEL from agent"));
}
