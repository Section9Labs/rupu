//! `--git` runs git init on a non-repo dir and is a no-op on a repo.

use rupu_cli::cmd::init::{init_for_test, InitArgs};

fn run_git(path: &std::path::Path) {
    init_for_test(InitArgs {
        path: path.to_path_buf(),
        with_samples: false,
        force: false,
        git: true,
    })
    .unwrap();
}

#[test]
fn git_flag_inits_git_in_empty_dir() {
    if which::which("git").is_err() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    run_git(tmp.path());
    assert!(tmp.path().join(".git").exists(), ".git/ should be created");
}

#[test]
fn git_flag_is_noop_in_existing_repo() {
    if which::which("git").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .arg("init")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let head_before = std::fs::read_to_string(tmp.path().join(".git/HEAD")).unwrap();
    run_git(tmp.path());
    let head_after = std::fs::read_to_string(tmp.path().join(".git/HEAD")).unwrap();
    assert_eq!(head_before, head_after, "second init should be no-op");
}
