use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;

fn write_agent(dir: &std::path::Path, name: &str, provider: &str, model: &str) {
    let agent_dir = dir.join(".rupu/agents");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let path = agent_dir.join(format!("{name}.md"));
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "---").unwrap();
    writeln!(f, "name: {name}").unwrap();
    writeln!(f, "provider: {provider}").unwrap();
    writeln!(f, "model: {model}").unwrap();
    writeln!(f, "---").unwrap();
    writeln!(f, "You are a hello-world agent.").unwrap();
}

const SCRIPT: &str = r#"[{"AssistantText":{"text":"hi from mock","stop":"end_turn","input_tokens":11,"output_tokens":3}}]"#;

#[test]
fn run_against_anthropic_via_mock_seam() {
    let dir = tempfile::tempdir().unwrap();
    write_agent(dir.path(), "hello", "anthropic", "claude-sonnet-4-6");
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", SCRIPT)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args(["run", "hello", "--mode", "bypass", "hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("provider: anthropic"))
        .stdout(predicate::str::contains("Total: 11 input"));
}

#[test]
fn run_against_openai_via_mock_seam() {
    let dir = tempfile::tempdir().unwrap();
    write_agent(dir.path(), "hello", "openai", "gpt-5");
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", SCRIPT)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args(["run", "hello", "--mode", "bypass", "hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("provider: openai"));
}

#[test]
fn run_against_gemini_via_mock_seam() {
    let dir = tempfile::tempdir().unwrap();
    write_agent(dir.path(), "hello", "gemini", "gemini-2.5-pro");
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", SCRIPT)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args(["run", "hello", "--mode", "bypass", "hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("provider: gemini"));
}

#[test]
fn run_against_copilot_via_mock_seam() {
    let dir = tempfile::tempdir().unwrap();
    write_agent(dir.path(), "hello", "copilot", "gpt-4o");
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", SCRIPT)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args(["run", "hello", "--mode", "bypass", "hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("provider: copilot"));
}
