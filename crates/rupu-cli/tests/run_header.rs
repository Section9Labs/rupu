use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;

#[test]
fn run_footer_prints_token_totals() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join(".rupu/agents");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let agent_path = agent_dir.join("hello.md");
    let mut f = std::fs::File::create(&agent_path).unwrap();
    writeln!(f, "---").unwrap();
    writeln!(f, "name: hello").unwrap();
    writeln!(f, "provider: anthropic").unwrap();
    writeln!(f, "model: claude-sonnet-4-6").unwrap();
    writeln!(f, "---").unwrap();
    writeln!(f, "You are a hello-world agent.").unwrap();
    drop(f);

    // Script with explicit token counts: 42 input, 7 output.
    let script = r#"[{"AssistantText":{"text":"hi","stop":"end_turn","input_tokens":42,"output_tokens":7}}]"#;

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args(["run", "hello", "--mode", "bypass", "say hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Total: 42 input"))
        .stdout(predicate::str::contains("7 output"));
}

#[test]
fn run_header_prints_provider_model_line() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join(".rupu/agents");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let agent_path = agent_dir.join("hello.md");
    let mut f = std::fs::File::create(&agent_path).unwrap();
    writeln!(f, "---").unwrap();
    writeln!(f, "name: hello").unwrap();
    writeln!(f, "provider: anthropic").unwrap();
    writeln!(f, "model: claude-sonnet-4-6").unwrap();
    writeln!(f, "---").unwrap();
    writeln!(f, "You are a hello-world agent.").unwrap();
    drop(f);

    // Mock provider script: one assistant turn that ends.
    let script = r#"[{"AssistantText":{"text":"hi","stop":"end_turn"}}]"#;

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .args(["run", "hello", "--mode", "bypass", "say hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("agent: hello"))
        .stdout(predicate::str::contains("provider: anthropic"))
        .stdout(predicate::str::contains("model: claude-sonnet-4-6"));
}
