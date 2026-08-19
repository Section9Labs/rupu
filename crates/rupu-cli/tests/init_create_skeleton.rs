//! `rupu init` against an empty TempDir creates the skeleton:
//!   .rupu/agents/, .rupu/workflows/, .rupu/netflow/, .rupu/config.toml,
//!   and a .gitignore with the transcripts + netflow entries.

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

/// `rupu_netflow::netflow_dir`'s existence gate routes a run's ledger to
/// `<project>/.rupu/netflow/` only when that directory ALREADY EXISTS
/// (else every run falls back to the global root regardless of project) --
/// so `init` must create it, not just list it in `GITIGNORE_ENTRIES`, or
/// every default install's project-scoped Network tab is empty forever.
#[test]
fn creates_the_project_local_netflow_directory_so_it_routes_locally() {
    let tmp = tempfile::tempdir().unwrap();

    init_for_test(args(tmp.path())).expect("init should succeed");

    let netflow_dir = tmp.path().join(".rupu/netflow");
    assert!(
        netflow_dir.is_dir(),
        "init must create .rupu/netflow/ so netflow_dir's existence gate routes this \
         project's ledgers locally from its very first run"
    );
    // Directory-local self-ignoring `.gitignore` -- the same protection
    // every `NetflowPaths::ensure_dir` call installs, belt-and-suspenders
    // with the project-level `.gitignore` entry `ensure_gitignore_entry`
    // writes separately.
    let gitignore = std::fs::read_to_string(netflow_dir.join(".gitignore")).unwrap();
    assert!(
        gitignore.lines().any(|l| l.trim() == "*"),
        "expected the netflow dir's own self-ignoring .gitignore, got: {gitignore:?}"
    );
}
