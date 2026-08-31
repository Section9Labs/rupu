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
        .stdout(predicate::str::contains("(anthropic · claude-sonnet-4-6)"))
        .stdout(predicate::str::contains("hi from mock"));
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
        .stdout(predicate::str::contains("(openai · gpt-5)"));
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
        .stdout(predicate::str::contains("(gemini · gemini-2.5-pro)"));
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
        .stdout(predicate::str::contains("(copilot · gpt-4o)"));
}

// ── Named (multi-account) providers ──────────────────────────────────────────
//
// `rupu auth login --account anthropic-work --kind anthropic` declares
// `[providers.anthropic-work] kind = "anthropic"` in the global config.toml —
// that table IS the storage `rupu auth status`'s KIND column reads back
// (`cmd/auth.rs::resolve_declared_kind` -> `provider_factory::resolve_kind`).
// An agent pinned to such an account must run against its vendor kind, not
// bail as an undeclared provider.

/// Write a global `config.toml` declaring one named account.
fn write_named_account(home: &std::path::Path, account: &str, kind: &str) {
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(
        home.join("config.toml"),
        format!("[providers.{account}]\nkind = \"{kind}\"\n"),
    )
    .unwrap();
}

#[test]
fn run_against_a_named_anthropic_account_via_mock_seam() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".rupu");
    write_named_account(&home, "anthropic-work", "anthropic");
    write_agent(dir.path(), "hello", "anthropic-work", "claude-sonnet-4-6");
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", SCRIPT)
        .env("RUPU_HOME", &home)
        .args(["run", "hello", "--mode", "bypass", "hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "(anthropic-work · claude-sonnet-4-6)",
        ))
        .stdout(predicate::str::contains("hi from mock"));
}

#[test]
fn run_against_a_named_openai_account_via_mock_seam() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".rupu");
    write_named_account(&home, "openai-work", "openai");
    write_agent(dir.path(), "hello", "openai-work", "gpt-5");
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", SCRIPT)
        .env("RUPU_HOME", &home)
        .args(["run", "hello", "--mode", "bypass", "hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("(openai-work · gpt-5)"));
}

/// A declared openai-compatible endpoint keeps working exactly as before —
/// its `kind` ("openai-compatible") is not a dispatchable builtin, so it must
/// still pass the gate on the strength of its `[providers.<name>]` block.
#[test]
fn run_against_an_openai_compatible_account_via_mock_seam() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".rupu");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("config.toml"),
        "[providers.oracle]\nkind = \"openai-compatible\"\n\
         base_url = \"http://127.0.0.1:9\"\ndefault_model = \"glm\"\n",
    )
    .unwrap();
    write_agent(dir.path(), "hello", "oracle", "glm");
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", SCRIPT)
        .env("RUPU_HOME", &home)
        .args(["run", "hello", "--mode", "bypass", "hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("(oracle · glm)"));
}

/// A name that is neither a builtin, nor a declared account, nor an
/// openai-compatible endpoint still fails — and the message must name BOTH
/// remedies, since either could be what the user forgot.
#[test]
fn run_against_an_undeclared_provider_still_errors_actionably() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".rupu");
    std::fs::create_dir_all(&home).unwrap();
    write_agent(dir.path(), "hello", "typo-provider", "some-model");
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", SCRIPT)
        .env("RUPU_HOME", &home)
        .args(["run", "hello", "--mode", "bypass", "hi"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("typo-provider"))
        // remedy 1: declare the account's vendor kind
        .stderr(predicate::str::contains("rupu auth login"))
        // remedy 2: declare an openai-compatible endpoint
        .stderr(predicate::str::contains("openai-compatible"));
}
