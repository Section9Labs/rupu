//! End-to-end tests for `rupu config get | set`.
//!
//! These tests mutate process-global state (`RUPU_HOME`). Hold
//! `ENV_LOCK` for the whole body of every test to serialise them within
//! this binary.

use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

#[tokio::test]
async fn config_set_then_get_round_trip() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "config".into(),
        "set".into(),
        "default_model".into(),
        "claude-opus-4-7".into(),
    ])
    .await;
    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "config set should exit 0"
    );

    // Read it back via the CLI
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "config".into(),
        "get".into(),
        "default_model".into(),
    ])
    .await;

    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "config get should exit 0 after set"
    );

    // Verify the file content on disk
    let toml = std::fs::read_to_string(tmp.path().join("config.toml")).unwrap();
    assert!(
        toml.contains("default_model"),
        "config.toml should contain the key"
    );
    assert!(
        toml.contains("claude-opus-4-7"),
        "config.toml should contain the value"
    );
}

#[tokio::test]
async fn config_get_missing_key_exits_nonzero() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());

    // Set one key first so the file exists
    rupu_cli::run(vec![
        "rupu".into(),
        "config".into(),
        "set".into(),
        "some_key".into(),
        "some_value".into(),
    ])
    .await;

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "config".into(),
        "get".into(),
        "nonexistent_key".into(),
    ])
    .await;

    std::env::remove_var("RUPU_HOME");

    assert_ne!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "config get for missing key should exit nonzero"
    );
}

#[tokio::test]
async fn config_get_no_file_exits_nonzero() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "config".into(),
        "get".into(),
        "default_model".into(),
    ])
    .await;

    std::env::remove_var("RUPU_HOME");

    assert_ne!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "config get when no file exists should exit nonzero"
    );
}

#[tokio::test]
async fn config_set_creates_file_if_missing() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    // Use a subdirectory that doesn't yet exist to exercise ensure_dir
    let rupu_home = tmp.path().join("new_rupu_home");
    std::env::set_var("RUPU_HOME", &rupu_home);

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "config".into(),
        "set".into(),
        "provider".into(),
        "anthropic".into(),
    ])
    .await;

    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "config set should create config.toml even when home dir doesn't exist yet"
    );

    let config_path = rupu_home.join("config.toml");
    assert!(config_path.exists(), "config.toml should have been created");
    let toml = std::fs::read_to_string(&config_path).unwrap();
    assert!(toml.contains("provider"), "should contain the key");
    assert!(toml.contains("anthropic"), "should contain the value");
}
