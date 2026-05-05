//! `--force` overwrites pre-existing template files.

use rupu_cli::cmd::init::{init_for_test, InitArgs};
use rupu_cli::templates::MANIFEST;

#[test]
fn force_overwrites_existing_templates() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join(".rupu/agents/review-diff.md");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "stub\n").unwrap();

    init_for_test(InitArgs {
        path: tmp.path().to_path_buf(),
        with_samples: true,
        force: true,
        git: false,
    })
    .unwrap();

    let expected = MANIFEST
        .iter()
        .find(|t| t.target_relpath.ends_with("review-diff.md"))
        .unwrap()
        .content;
    let body = std::fs::read_to_string(&target).unwrap();
    assert_eq!(body, expected, "--force must overwrite with template content");
}
