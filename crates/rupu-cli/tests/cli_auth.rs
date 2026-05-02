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
