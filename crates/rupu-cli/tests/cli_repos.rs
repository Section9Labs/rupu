//! End-to-end tests for `rupu repos attach|prefer|tracked|forget`.
//!
//! These tests mutate process-global state (`RUPU_HOME`). Hold
//! `ENV_LOCK` for the whole body of every test to serialize them.

use assert_cmd::Command;
use predicates::prelude::*;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

fn init_git_checkout(path: &std::path::Path, remote: &str) {
    std::fs::create_dir_all(path).unwrap();
    let status = std::process::Command::new("git")
        .arg("init")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "git init should succeed");
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["remote", "add", "origin", remote])
        .status()
        .unwrap();
    assert!(status.success(), "git remote add should succeed");
}

#[tokio::test]
async fn repos_attach_and_forget_round_trip() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    init_git_checkout(&repo, "git@github.com:Section9Labs/rupu.git");
    std::env::set_var("RUPU_HOME", &home);

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "repos".into(),
        "attach".into(),
        "github:Section9Labs/rupu".into(),
        repo.display().to_string(),
    ])
    .await;
    assert_eq!(exit, std::process::ExitCode::from(0));

    let store = rupu_workspace::RepoRegistryStore {
        root: home.join("repos"),
    };
    let tracked = store.load("github:Section9Labs/rupu").unwrap().unwrap();
    assert_eq!(tracked.repo_ref, "github:Section9Labs/rupu");
    assert_eq!(tracked.known_paths.len(), 1);

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "repos".into(),
        "forget".into(),
        "github:Section9Labs/rupu".into(),
    ])
    .await;
    std::env::remove_var("RUPU_HOME");
    assert_eq!(exit, std::process::ExitCode::from(0));
    assert!(store.load("github:Section9Labs/rupu").unwrap().is_none());
}

#[tokio::test]
async fn repos_prefer_switches_preferred_path() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let repo_a = tmp.path().join("repo-a");
    let repo_b = tmp.path().join("repo-b");
    init_git_checkout(&repo_a, "git@github.com:Section9Labs/rupu.git");
    init_git_checkout(&repo_b, "git@github.com:Section9Labs/rupu.git");
    std::env::set_var("RUPU_HOME", &home);

    for path in [&repo_a, &repo_b] {
        let exit = rupu_cli::run(vec![
            "rupu".into(),
            "repos".into(),
            "attach".into(),
            "github:Section9Labs/rupu".into(),
            path.display().to_string(),
        ])
        .await;
        assert_eq!(exit, std::process::ExitCode::from(0));
    }

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "repos".into(),
        "prefer".into(),
        "github:Section9Labs/rupu".into(),
        repo_b.display().to_string(),
    ])
    .await;
    std::env::remove_var("RUPU_HOME");
    assert_eq!(exit, std::process::ExitCode::from(0));

    let store = rupu_workspace::RepoRegistryStore {
        root: home.join("repos"),
    };
    let tracked = store.load("github:Section9Labs/rupu").unwrap().unwrap();
    assert_eq!(
        tracked.preferred_path,
        repo_b.canonicalize().unwrap().display().to_string()
    );
    assert_eq!(tracked.known_paths.len(), 2);
}

#[tokio::test]
async fn repos_tracked_supports_global_json_and_csv_output() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    init_git_checkout(&repo, "git@github.com:Section9Labs/rupu.git");
    std::env::set_var("RUPU_HOME", &home);

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "repos".into(),
        "attach".into(),
        "github:Section9Labs/rupu".into(),
        repo.display().to_string(),
    ])
    .await;
    assert_eq!(exit, std::process::ExitCode::from(0));

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["--format", "json", "repos", "tracked"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"tracked_repos\""))
        .stdout(predicate::str::contains(
            "\"repo\": \"github:Section9Labs/rupu\"",
        ));

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(tmp.path())
        .args(["--format", "csv", "repos", "tracked"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "repo,preferred_path,known_paths,default_branch",
        ))
        .stdout(predicate::str::contains("github:Section9Labs/rupu"));

    std::env::remove_var("RUPU_HOME");
}
