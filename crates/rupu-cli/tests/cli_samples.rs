//! Verifies that running `rupu agent list` and `rupu workflow list`
//! from the rupu repo's own checkout surfaces the repo-local sample
//! library under `.rupu/` via normal project-discovery.

use assert_cmd::Command;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::OnceLock;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn ok_str() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| format!("{:?}", ExitCode::from(0)))
}

#[tokio::test]
async fn agent_list_finds_repo_local_samples() {
    let _guard = ENV_LOCK.lock().await;
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap().to_path_buf();
    std::env::set_current_dir(&repo_root).unwrap();
    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());
    let exit = rupu_cli::run(vec!["rupu".into(), "agent".into(), "list".into()]).await;
    std::env::remove_var("RUPU_HOME");
    assert_eq!(format!("{exit:?}"), ok_str());
}

#[tokio::test]
async fn workflow_list_finds_repo_local_sample() {
    let _guard = ENV_LOCK.lock().await;
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap().to_path_buf();
    std::env::set_current_dir(&repo_root).unwrap();
    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());
    let exit = rupu_cli::run(vec!["rupu".into(), "workflow".into(), "list".into()]).await;
    std::env::remove_var("RUPU_HOME");
    assert_eq!(format!("{exit:?}"), ok_str());
}

#[test]
fn autoflow_list_finds_repo_local_autoflows() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap().to_path_buf();
    let tmp = assert_fs::TempDir::new().unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&repo_root)
        .env("RUPU_HOME", tmp.path())
        .args(["autoflow", "list"])
        .assert()
        .success()
        .stdout(predicates::str::contains("issue-supervisor-dispatch"))
        .stdout(predicates::str::contains("phase-delivery-cycle"));
}
