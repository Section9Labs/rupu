use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn status_renders_two_column_header() {
    Command::cargo_bin("rupu")
        .unwrap()
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PROVIDER"))
        .stdout(predicate::str::contains("API-KEY"))
        .stdout(predicate::str::contains("SSO"));
}

#[test]
fn status_lists_all_four_providers() {
    Command::cargo_bin("rupu")
        .unwrap()
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("anthropic"))
        .stdout(predicate::str::contains("openai"))
        .stdout(predicate::str::contains("gemini"))
        .stdout(predicate::str::contains("copilot"));
}
