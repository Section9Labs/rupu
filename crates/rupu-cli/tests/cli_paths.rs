use assert_fs::prelude::*;
use rupu_cli::paths::{global_dir, project_root_for, transcripts_dir};

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
