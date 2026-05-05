//! `rupu init` against an empty TempDir creates the skeleton:
//!   .rupu/agents/, .rupu/workflows/, .rupu/config.toml, and a
//!   .gitignore with the transcripts entry.

use std::path::Path;

use rupu_cli::cmd::init::{init_for_test, InitArgs};

fn args(path: &Path) -> InitArgs {
    InitArgs {
        path: path.to_path_buf(),
        with_samples: false,
        force: false,
        git: false,
    }
}

#[test]
fn empty_dir_gets_full_skeleton() {
    let tmp = tempfile::tempdir().unwrap();

    init_for_test(args(tmp.path())).expect("init should succeed");

    assert!(tmp.path().join(".rupu").is_dir());
    assert!(tmp.path().join(".rupu/agents").is_dir());
    assert!(tmp.path().join(".rupu/workflows").is_dir());

    let cfg = std::fs::read_to_string(tmp.path().join(".rupu/config.toml")).unwrap();
    assert!(cfg.contains("rupu project config"));
    assert!(cfg.contains("[scm.default]"));

    let gi = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(gi.contains(".rupu/transcripts/"));
}
