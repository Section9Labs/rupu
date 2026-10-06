//! `rupu workflow run --file <PATH>` runs a workflow straight from a file
//! path, without it being in the catalog (`<global>/workflows/` or the
//! project's `.rupu/workflows/`). A later agentiflow step launches
//! lead-generated workflows this way.
//!
//! The workflow under test is a lone approval gate: the run parks at it
//! before any agent step, so no provider call is made and the test needs
//! neither a mock provider nor the network.

use crate::ENV_LOCK;
use assert_cmd::Command as AssertCommand;
use assert_fs::prelude::*;
use predicates::prelude::*;

const OFFCAT_WORKFLOW: &str = r#"
name: offcat-gate
description: lives outside every catalog dir
steps:
  - id: gate
    approval:
      prompt: "Approve?"
      timeout_seconds: 3600
      on_timeout: fail
"#;

/// `<tmp>/.rupu` (global, empty catalog), `<tmp>/proj` (cwd, no
/// `.rupu/`), and `<tmp>/elsewhere/offcat.yaml` (the off-catalog file).
fn fixture() -> (
    assert_fs::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("workflows").create_dir_all().unwrap();
    global.child("agents").create_dir_all().unwrap();
    let project = tmp.child("proj");
    project.create_dir_all().unwrap();
    let file = tmp.child("elsewhere/offcat.yaml");
    file.write_str(OFFCAT_WORKFLOW).unwrap();
    let (g, p, f) = (
        global.path().to_path_buf(),
        project.path().to_path_buf(),
        file.path().to_path_buf(),
    );
    (tmp, g, p, f)
}

/// The workflow is not in any catalog, yet `--file` runs it: the run
/// record exists, is named by the file's own `name:`, and parked at its
/// gate (so the file was read, parsed and executed).
#[tokio::test(flavor = "multi_thread")]
async fn run_file_loads_workflow_outside_catalog() {
    let _guard = ENV_LOCK.lock().await;
    let (_tmp, global, project, file) = fixture();
    let run_id = "run_from_file_offcat";

    // Not resolvable by name: the file is not in the catalog.
    assert!(!global.join("workflows/offcat.yaml").exists());
    assert!(!global.join("workflows/offcat-gate.yaml").exists());

    std::env::set_var("RUPU_HOME", &global);
    std::env::set_current_dir(&project).unwrap();
    let _exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "run".into(),
        "--file".into(),
        file.to_string_lossy().into_owned(),
        "--run-id".into(),
        run_id.into(),
        "--plain".into(),
    ])
    .await;
    std::env::remove_var("RUPU_HOME");

    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    let record = store
        .load(run_id)
        .expect("--file must have started a run under the pinned run id");
    assert_eq!(record.workflow_name, "offcat-gate");
    assert_eq!(
        record.status,
        rupu_orchestrator::RunStatus::AwaitingApproval,
        "the lone gate step must have parked the run"
    );

    // Running from a file does not add it to the catalog.
    assert!(!global.join("workflows/offcat-gate.yaml").exists());
}

/// The same workflow cannot be run by NAME — proving the test above
/// succeeded because of `--file`, not a catalog lookup that happened to
/// resolve.
#[tokio::test(flavor = "multi_thread")]
async fn the_same_workflow_is_not_runnable_by_name() {
    let _guard = ENV_LOCK.lock().await;
    let (_tmp, global, project, _file) = fixture();

    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &global)
        .current_dir(&project)
        .args(["workflow", "run", "offcat-gate", "--plain"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("workflow not found"));
}

/// A `--file` path that cannot be read fails loudly and names the path;
/// no run is started.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_file_errors_clearly() {
    let _guard = ENV_LOCK.lock().await;
    let (_tmp, global, project, _file) = fixture();
    let bogus = project.join("no-such-workflow.yaml");

    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &global)
        .current_dir(&project)
        .args(["workflow", "run", "--plain", "--file"])
        .arg(&bogus)
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("--file")
                .and(predicates::str::contains(bogus.to_string_lossy().as_ref())),
        );

    let runs = global.join("runs");
    let started = std::fs::read_dir(&runs)
        .map(|d| d.count())
        .unwrap_or_default();
    assert_eq!(started, 0, "a bad --file must not leave a run record");
}

/// A file that is not a valid workflow errors, naming the path.
#[tokio::test(flavor = "multi_thread")]
async fn an_unparseable_file_errors_clearly() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, global, project, _file) = fixture();
    let bad = tmp.child("elsewhere/bad.yaml");
    bad.write_str("not: a workflow\n").unwrap();

    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &global)
        .current_dir(&project)
        .args(["workflow", "run", "--plain", "--file"])
        .arg(bad.path())
        .assert()
        .failure()
        .stderr(predicates::str::contains("--file"));
}

/// clap shape: `--file` replaces the name, and conflicts with a run
/// target (a generated workflow has no clone/PR/issue target).
#[tokio::test(flavor = "multi_thread")]
async fn file_conflicts_with_name_and_target_and_one_is_required() {
    let _guard = ENV_LOCK.lock().await;
    let (_tmp, global, project, file) = fixture();
    let run = |args: &[&str]| {
        AssertCommand::cargo_bin("rupu")
            .unwrap()
            .env("RUPU_HOME", &global)
            .current_dir(&project)
            .args(args)
            .assert()
    };
    let file = file.to_string_lossy().into_owned();

    run(&["workflow", "run", "some-name", "--file", &file]).failure();
    run(&["workflow", "run", "--file", &file, "github:o/r"]).failure();
    // Neither a name nor --file.
    run(&["workflow", "run"])
        .failure()
        .stderr(predicates::str::contains("required"));
}
