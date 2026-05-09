use assert_fs::prelude::*;
use rupu_cli::paths::{
    autoflow_claims_dir, autoflow_event_cursors_dir, autoflow_wake_dedupe_dir,
    autoflow_wake_payloads_dir, autoflow_wake_processed_dir, autoflow_wake_queue_dir,
    autoflow_wakes_dir, autoflow_worktrees_dir, autoflows_dir, global_dir, project_root_for,
    repos_dir, transcripts_dir,
};

#[test]
fn global_dir_uses_rupu_home_env_when_set() {
    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());
    let g = global_dir().unwrap();
    assert_eq!(
        g.canonicalize().unwrap(),
        tmp.path().canonicalize().unwrap()
    );
    std::env::remove_var("RUPU_HOME");
}

#[test]
fn project_root_walks_up_for_dot_rupu() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child(".rupu").create_dir_all().unwrap();
    let nested = tmp.child("a/b/c");
    nested.create_dir_all().unwrap();
    let r = project_root_for(nested.path()).unwrap();
    assert_eq!(
        r.unwrap().canonicalize().unwrap(),
        tmp.path().canonicalize().unwrap()
    );
}

#[test]
fn transcripts_dir_is_project_local_when_present() {
    let tmp_global = assert_fs::TempDir::new().unwrap();
    let tmp_project = assert_fs::TempDir::new().unwrap();
    tmp_project
        .child(".rupu/transcripts")
        .create_dir_all()
        .unwrap();
    let dir = transcripts_dir(tmp_global.path(), Some(tmp_project.path()));
    assert_eq!(
        dir.canonicalize().unwrap(),
        tmp_project
            .child(".rupu/transcripts")
            .path()
            .canonicalize()
            .unwrap()
    );
}

#[test]
fn transcripts_dir_falls_back_to_global() {
    let tmp_global = assert_fs::TempDir::new().unwrap();
    let dir = transcripts_dir(tmp_global.path(), None);
    assert!(dir.ends_with("transcripts"));
}

#[test]
fn state_dirs_live_under_global_root() {
    let tmp_global = assert_fs::TempDir::new().unwrap();
    assert_eq!(
        repos_dir(tmp_global.path()),
        tmp_global.path().join("repos")
    );
    assert_eq!(
        autoflows_dir(tmp_global.path()),
        tmp_global.path().join("autoflows")
    );
    assert_eq!(
        autoflow_claims_dir(tmp_global.path()),
        tmp_global.path().join("autoflows/claims")
    );
    assert_eq!(
        autoflow_worktrees_dir(tmp_global.path()),
        tmp_global.path().join("autoflows/worktrees")
    );
    assert_eq!(
        autoflow_event_cursors_dir(tmp_global.path()),
        tmp_global.path().join("autoflows/event-cursors")
    );
    assert_eq!(
        autoflow_wakes_dir(tmp_global.path()),
        tmp_global.path().join("autoflows/wakes")
    );
    assert_eq!(
        autoflow_wake_queue_dir(tmp_global.path()),
        tmp_global.path().join("autoflows/wakes/queue")
    );
    assert_eq!(
        autoflow_wake_processed_dir(tmp_global.path()),
        tmp_global.path().join("autoflows/wakes/processed")
    );
    assert_eq!(
        autoflow_wake_payloads_dir(tmp_global.path()),
        tmp_global.path().join("autoflows/wakes/payloads")
    );
    assert_eq!(
        autoflow_wake_dedupe_dir(tmp_global.path()),
        tmp_global.path().join("autoflows/wakes/dedupe")
    );
}
