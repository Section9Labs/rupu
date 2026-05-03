use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn login_api_key_with_inline_key_succeeds() {
    Command::cargo_bin("rupu")
        .unwrap()
        .args([
            "auth",
            "login",
            "--provider",
            "anthropic",
            "--mode",
            "api-key",
            "--key",
            "sk-test-flag-only",
        ])
        .assert()
        .success();
}

#[test]
fn login_sso_without_browser_errors_on_headless_linux() {
    // Only meaningful on Linux without DISPLAY; skipped elsewhere.
    if std::env::var_os("DISPLAY").is_some() || cfg!(not(target_os = "linux")) {
        return;
    }
    let mut cmd = Command::cargo_bin("rupu").unwrap();
    cmd.env_remove("DISPLAY").env_remove("BROWSER").args([
        "auth",
        "login",
        "--provider",
        "anthropic",
        "--mode",
        "sso",
    ]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("requires a desktop browser"));
}
