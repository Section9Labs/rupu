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

/// Fix round 1, Minor 1: re-running `init` on an already-opted-in project
/// must report SKIPPED for the netflow directory, not silently re-count
/// it as CREATED — mirrors `.rupu/config.toml`'s existing-file handling.
/// Exercised via the actual binary (`Command`, like `init_smoke.rs`)
/// because the printed CREATED/SKIPPED lines aren't observable through
/// `init_for_test`'s `Result`-only return.
#[test]
fn rerunning_init_reports_the_netflow_directory_as_skipped_not_created_again() {
    let tmp = tempfile::tempdir().unwrap();

    let first = std::process::Command::new(env!("CARGO_BIN_EXE_rupu"))
        .args(["init", tmp.path().to_str().unwrap()])
        .output()
        .expect("spawn rupu (first init)");
    assert!(first.status.success());
    let first_stdout = String::from_utf8_lossy(&first.stdout);
    assert!(
        first_stdout.contains("CREATED .rupu/netflow"),
        "first init should report the netflow directory as newly created: {first_stdout}"
    );

    let second = std::process::Command::new(env!("CARGO_BIN_EXE_rupu"))
        .args(["init", tmp.path().to_str().unwrap()])
        .output()
        .expect("spawn rupu (second init)");
    assert!(second.status.success());
    let second_stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        second_stdout.contains("SKIPPED .rupu/netflow"),
        "re-running init on an already-opted-in project should report SKIPPED, \
         not silently re-create: {second_stdout}"
    );
    assert!(
        !second_stdout.contains("CREATED .rupu/netflow"),
        "re-running init must not re-report the netflow directory as CREATED: {second_stdout}"
    );
}
