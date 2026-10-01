//! Pre-existing template files are SKIPPED by default.

use rupu_cli::cmd::init::{init_for_test, InitArgs};

#[test]
fn pre_existing_template_is_skipped_default() {
    let tmp = tempfile::tempdir().unwrap();
    let agents = tmp.path().join(".rupu/agents");
    std::fs::create_dir_all(&agents).unwrap();
    let stub = "MY CUSTOM AGENT — DO NOT TOUCH\n";
    std::fs::write(agents.join("review-diff.md"), stub).unwrap();

    init_for_test(InitArgs {
        path: tmp.path().to_path_buf(),
        with_samples: true,
        force: false,
        git: false,
    })
    .unwrap();

    let body = std::fs::read_to_string(agents.join("review-diff.md")).unwrap();
    assert_eq!(body, stub, "pre-existing file must NOT be overwritten");

    // Other templates should still be created.
    assert!(agents.join("add-tests.md").exists());
}
