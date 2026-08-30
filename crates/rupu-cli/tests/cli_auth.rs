//! End-to-end tests for `rupu auth login | logout | status`.
//!
//! These tests mutate process-global state (`RUPU_HOME`). Hold
//! `ENV_LOCK` for the whole body of every test to serialise them within
//! this binary.
//!
//! To avoid touching the real OS keychain the tests pre-populate the
//! backend probe-cache at `<RUPU_HOME>/cache/auth-backend.json` with
//! `"json_file"` before invoking the CLI. `select_backend` reads the
//! cache and skips the probe, routing all credential operations to the
//! chmod-600 JSON file inside the tempdir.

use assert_cmd::Command;
use predicates::prelude::*;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

/// Pre-populate the backend cache so the CLI never probes the real
/// OS keychain.  `BackendChoice::JsonFile` serialises as `"json_file"`.
fn force_json_backend(tmp: &assert_fs::TempDir) {
    let cache_path = tmp.path().join("cache/auth-backend.json");
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    std::fs::write(&cache_path, r#""json_file""#).unwrap();
}

#[tokio::test]
async fn auth_status_works_with_empty_backend() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    force_json_backend(&tmp);
    std::env::set_var("RUPU_HOME", tmp.path());

    let exit = rupu_cli::run(vec!["rupu".into(), "auth".into(), "status".into()]).await;
    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "auth status should exit 0 even with no credentials stored"
    );
}

#[tokio::test]
async fn login_then_status_shows_configured() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    force_json_backend(&tmp);
    std::env::set_var("RUPU_HOME", tmp.path());

    // login with --key flag
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "auth".into(),
        "login".into(),
        "--provider".into(),
        "anthropic".into(),
        "--key".into(),
        "sk-test".into(),
    ])
    .await;
    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "auth login should exit 0"
    );

    // status should still exit 0 (credential is now present)
    let exit = rupu_cli::run(vec!["rupu".into(), "auth".into(), "status".into()]).await;
    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "auth status should exit 0 after login"
    );

    // confirm the credential is retrievable via a logout (which calls forget)
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "auth".into(),
        "logout".into(),
        "--provider".into(),
        "anthropic".into(),
    ])
    .await;
    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "auth logout should exit 0 when credential exists"
    );
}

#[tokio::test]
async fn login_then_logout_round_trip() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    force_json_backend(&tmp);
    std::env::set_var("RUPU_HOME", tmp.path());

    // First login
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "auth".into(),
        "login".into(),
        "--provider".into(),
        "openai".into(),
        "--key".into(),
        "sk-openai-test".into(),
    ])
    .await;
    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "first login should exit 0"
    );

    // Logout
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "auth".into(),
        "logout".into(),
        "--provider".into(),
        "openai".into(),
    ])
    .await;
    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "logout should exit 0"
    );

    // Login again
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "auth".into(),
        "login".into(),
        "--provider".into(),
        "openai".into(),
        "--key".into(),
        "sk-openai-test-2".into(),
    ])
    .await;
    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "second login should exit 0"
    );
}

#[tokio::test]
async fn auth_backend_snapshot_defaults_to_full() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    force_json_backend(&tmp);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        // `supports-color` honors FORCE_COLOR ahead of the `--no-color`
        // flag, so a developer shell exporting it makes these plain-text
        // assertions fail. Clear it for the child.
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .arg("auth")
        .arg("backend")
        .arg("--no-color")
        .assert()
        .success()
        .stdout(predicate::str::contains("auth backend"))
        .stdout(predicate::str::contains("·  full"))
        .stdout(predicate::str::contains("state  ·  resolved backend"))
        .stdout(predicate::str::contains("paths  ·  storage files"))
        .stdout(predicate::str::contains("commands  ·  switch or override"));
}

#[tokio::test]
async fn auth_backend_focused_hides_detail_sections() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    force_json_backend(&tmp);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        // `supports-color` honors FORCE_COLOR ahead of the `--no-color`
        // flag, so a developer shell exporting it makes these plain-text
        // assertions fail. Clear it for the child.
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .arg("auth")
        .arg("backend")
        .arg("--view")
        .arg("focused")
        .arg("--no-color")
        .assert()
        .success()
        .stdout(predicate::str::contains("·  focused"))
        .stdout(predicate::str::contains("state  ·  resolved backend"))
        .stdout(predicate::str::contains("paths  ·  storage files").not())
        .stdout(predicate::str::contains("commands  ·  switch or override").not());
}

#[tokio::test]
async fn auth_backend_use_file_renders_requested_backend_snapshot() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    force_json_backend(&tmp);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        // `supports-color` honors FORCE_COLOR ahead of the `--no-color`
        // flag, so a developer shell exporting it makes these plain-text
        // assertions fail. Clear it for the child.
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .arg("auth")
        .arg("backend")
        .arg("--use")
        .arg("file")
        .arg("--no-color")
        .assert()
        .success()
        .stdout(predicate::str::contains("requested"))
        .stdout(predicate::str::contains(
            "rupu auth login --provider <name>",
        ));
}

/// Regression pin for a shipped defect: kind resolution for `auth login`
/// used to route solely through `rupu_runtime::provider_factory::resolve_kind`,
/// whose name-is-vendor fallback is LLM-only and does not include
/// github/gitlab/linear/jira. `rupu auth login --provider github` is a
/// documented, first-class flow (README.md, docs/mcp.md, docs/scm.md, and
/// several in-product error messages tell users to run exactly this) — it
/// must keep working.
#[tokio::test]
async fn login_with_provider_github_succeeds() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    force_json_backend(&tmp);
    std::env::set_var("RUPU_HOME", tmp.path());

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "auth".into(),
        "login".into(),
        "--provider".into(),
        "github".into(),
        "--mode".into(),
        "api-key".into(),
        "--key".into(),
        "ghp-test".into(),
    ])
    .await;

    let auth_json = std::fs::read_to_string(tmp.path().join("auth.json")).unwrap_or_default();
    let config_written = tmp.path().join("config.toml").exists();
    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "auth login --provider github should exit 0, auth.json was: {auth_json}"
    );
    assert!(
        auth_json.contains("\"github/api-key\""),
        "expected github/api-key in auth.json, got: {auth_json}"
    );
    assert!(
        !config_written,
        "a bare vendor name should not write a config.toml"
    );
}

/// Regression pin: `logout --all` must clear every stored credential,
/// including a declared account name (e.g. `anthropic-work`) that a
/// fixed sweep over the builtin `ProviderId` list can never see. The
/// prior implementation printed "cleared all credentials" while leaving
/// such accounts behind.
#[tokio::test]
async fn logout_all_clears_a_named_account() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    force_json_backend(&tmp);
    std::env::set_var("RUPU_HOME", tmp.path());

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "auth".into(),
        "login".into(),
        "--account".into(),
        "anthropic-work".into(),
        "--kind".into(),
        "anthropic".into(),
        "--key".into(),
        "sk-test-work".into(),
    ])
    .await;
    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "login should exit 0"
    );
    let before = std::fs::read_to_string(tmp.path().join("auth.json")).unwrap();
    assert!(
        before.contains("anthropic-work/api-key"),
        "precondition: expected anthropic-work/api-key present, got: {before}"
    );

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "auth".into(),
        "logout".into(),
        "--all".into(),
        "--yes".into(),
    ])
    .await;
    let after = std::fs::read_to_string(tmp.path().join("auth.json")).unwrap_or_default();
    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "logout --all should exit 0"
    );
    assert!(
        !after.contains("anthropic-work/api-key"),
        "named account should be gone after --all, got: {after}"
    );
}
