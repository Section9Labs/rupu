use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;

fn make_hello_agent(dir: &std::path::Path) {
    let agent_dir = dir.join(".rupu/agents");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let agent_path = agent_dir.join("hello.md");
    let mut f = std::fs::File::create(&agent_path).unwrap();
    writeln!(f, "---").unwrap();
    writeln!(f, "name: hello").unwrap();
    writeln!(f, "provider: anthropic").unwrap();
    writeln!(f, "model: claude-sonnet-4-6").unwrap();
    writeln!(f, "---").unwrap();
    writeln!(f, "You are a hello-world agent.").unwrap();
}

#[test]
fn run_line_stream_header_shows_agent_provider_model() {
    let dir = tempfile::tempdir().unwrap();
    make_hello_agent(dir.path());

    let script = r#"[{"AssistantText":{"text":"hi","stop":"end_turn"}}]"#;

    // Line-stream header: `▶ hello  (anthropic · claude-sonnet-4-6)  run_XXX`
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args(["run", "hello", "--mode", "bypass", "say hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello"))
        .stdout(predicate::str::contains("anthropic"))
        .stdout(predicate::str::contains("claude-sonnet-4-6"));
}

#[test]
fn run_line_stream_shows_token_count() {
    let dir = tempfile::tempdir().unwrap();
    make_hello_agent(dir.path());

    // Script with explicit token counts: 42 input, 7 output (total 49).
    let script = r#"[{"AssistantText":{"text":"hi","stop":"end_turn","input_tokens":42,"output_tokens":7}}]"#;

    // Line-stream step footer: `✓ <run_id>  Xs · 49t`
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args(["run", "hello", "--mode", "bypass", "say hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("49t"));
}

#[test]
fn run_line_stream_shows_assistant_text() {
    let dir = tempfile::tempdir().unwrap();
    make_hello_agent(dir.path());

    let script = r#"[{"AssistantText":{"text":"hello world","stop":"end_turn"}}]"#;

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args(["run", "hello", "--mode", "bypass", "say hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello world"));
}
