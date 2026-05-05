//! Gitignore handling: missing → created; present without entry → appended;
//! present with entry → unchanged.

use rupu_cli::cmd::init::{init_for_test, InitArgs};

fn run(path: &std::path::Path) {
    init_for_test(InitArgs {
        path: path.to_path_buf(),
        with_samples: false,
        force: false,
        git: false,
    })
    .unwrap();
}

#[test]
fn missing_gitignore_is_created() {
    let tmp = tempfile::tempdir().unwrap();
    run(tmp.path());
    let body = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(body.contains(".rupu/transcripts/"));
}

#[test]
fn pre_existing_gitignore_without_entry_is_appended() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(".gitignore"), "/target\nnode_modules/\n").unwrap();
    run(tmp.path());
    let body = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(body.contains("/target"), "pre-existing entries preserved");
    assert!(body.contains("node_modules/"));
    assert!(body.contains(".rupu/transcripts/"), "rupu entry appended");
}

#[test]
fn pre_existing_gitignore_with_entry_is_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let original = "/target\n.rupu/transcripts/\nnode_modules/\n";
    std::fs::write(tmp.path().join(".gitignore"), original).unwrap();
    run(tmp.path());
    let body = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert_eq!(body, original, "no change when entry already present");
}

#[test]
fn pre_existing_gitignore_no_trailing_newline_is_handled() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(".gitignore"), "/target").unwrap(); // no trailing \n
    run(tmp.path());
    let body = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    // Should be "/target\n.rupu/transcripts/\n" — both lines well-formed.
    assert!(body.starts_with("/target\n"));
    assert!(body.contains(".rupu/transcripts/"));
    assert!(body.ends_with('\n'));
}
