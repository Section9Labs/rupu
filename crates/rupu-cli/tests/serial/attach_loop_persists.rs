//! The retained / line-printer attach loop of `rupu workflow run` ends two
//! ways besides a finished run — a runner that panicked, and an inline
//! approve-resume whose step results cannot be read back — and each must
//! still persist the run's portable metadata (backend, worker, manifest
//! path, with the manifest) before the command fails with that error.
//! Driven through the real binary (`CARGO_BIN_EXE_rupu`): the line
//! printer's gate prompt reads its `a` from stdin, which a test can only
//! feed to a child process, and a child keeps the panic out of this one.
//! The no-UI invocation (cron ticks inside `cp serve`, webhooks, autoflow)
//! is driven in-process, as those callers drive it.
//!
//! Here rather than in `tests/it/`: the no-UI test sets `RUPU_HOME` and
//! `RUPU_MOCK_PROVIDER_SCRIPT` in this process, as its callers' process has
//! them.

use crate::ENV_LOCK;
use assert_fs::prelude::*;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const WORKFLOW_GATED: &str = r#"
name: gated
steps:
  - id: build
    run:
      cmd: sh
      args: ["-c", "echo built"]
  - id: gate
    approval:
      prompt: "Ship?"
  - id: ship
    run:
      cmd: sh
      args: ["-c", "echo shipped"]
"#;

const WORKFLOW_ONE_AGENT_STEP: &str = r#"
name: one-agent
steps:
  - id: ask
    agent: echo
    prompt: "say hi"
"#;

/// A provider that panics mid-send, for the mock-provider seam.
const PANICKING_PROVIDER: &str = r#"[{"Panic": "provider blew up mid-send"}]"#;

/// `<tmp>/.rupu` (with `run:` steps enabled) as the rupu home, and a
/// project `<tmp>/proj` holding `workflow` and an `echo` agent.
fn home_and_project(workflow_name: &str, workflow: &str) -> (assert_fs::TempDir, PathBuf, PathBuf) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global
        .child("config.toml")
        .write_str("[workflow]\nrun_step_enabled = true\n")
        .unwrap();
    let project = tmp.child("proj");
    project
        .child(format!(".rupu/workflows/{workflow_name}.yaml"))
        .write_str(workflow)
        .unwrap();
    project
        .child(".rupu/agents/echo.md")
        .write_str("---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nyou echo.")
        .unwrap();
    let (global, project) = (global.path().to_path_buf(), project.path().to_path_buf());
    (tmp, global, project)
}

/// A `rupu workflow run <name> --plain` child, killed if the test ends
/// before it exits. Its output goes to files: a pipe nobody drains could
/// block it.
struct Rupu {
    child: Child,
    stderr: PathBuf,
}

impl Rupu {
    fn workflow_run(
        name: &str,
        run_id: &str,
        global: &Path,
        project: &Path,
        mock_script: Option<&str>,
        stdin: Stdio,
    ) -> Self {
        let stdout = global.join("rupu.stdout");
        let stderr = global.join("rupu.stderr");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rupu"));
        cmd.args(["workflow", "run", name, "--run-id", run_id, "--plain"])
            .env("RUPU_HOME", global)
            .env_remove("RUPU_MOCK_PROVIDER_SCRIPT")
            .current_dir(project)
            .stdin(stdin)
            .stdout(std::fs::File::create(&stdout).unwrap())
            .stderr(std::fs::File::create(&stderr).unwrap());
        if let Some(script) = mock_script {
            cmd.env("RUPU_MOCK_PROVIDER_SCRIPT", script);
        }
        Self {
            child: cmd.spawn().expect("spawn rupu"),
            stderr,
        }
    }

    /// Its exit status and stderr, once it exits within `budget`.
    fn wait(&mut self, budget: Duration) -> (ExitStatus, String) {
        let deadline = Instant::now() + budget;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll rupu") {
                return (status, std::fs::read_to_string(&self.stderr).unwrap());
            }
            assert!(
                Instant::now() < deadline,
                "rupu did not exit within {budget:?}; stderr so far: {}",
                std::fs::read_to_string(&self.stderr).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Rupu {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Wait until the run's `run.json` shows `status`, within `budget`.
fn wait_for_status(
    store: &rupu_orchestrator::RunStore,
    run_id: &str,
    status: &str,
    budget: Duration,
) {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if store
            .load(run_id)
            .is_ok_and(|r| r.status.as_str() == status)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "run {run_id} did not reach `{status}` within {budget:?}: {:?}",
        store.load(run_id).map(|r| r.status)
    );
}

/// The portable metadata is on the record and its manifest is on disk:
/// the manifest, returned.
fn assert_metadata_persisted(store: &rupu_orchestrator::RunStore, run_id: &str) -> String {
    let record = store.load(run_id).unwrap();
    assert_eq!(
        record.backend_id.as_deref(),
        Some("local_worktree"),
        "the backend was persisted on the way out: {record:?}"
    );
    assert!(
        record.worker_id.is_some(),
        "the worker was persisted: {record:?}"
    );
    let manifest = record
        .artifact_manifest_path
        .as_deref()
        .expect("the manifest path was recorded");
    let body = std::fs::read_to_string(manifest).expect("the manifest was written");
    assert!(body.contains(run_id), "the manifest is this run's: {body}");
    body
}

/// A runner that panics ends `workflow run` through the same last step as
/// a runner error: the run is marked failed on disk, the portable metadata
/// is persisted, the manifest written, and the command fails with the
/// panic. The run's one agent step gets a provider that panics mid-send.
/// Before, the printer waited on the dead runner's `Running` for good.
#[test]
fn a_panicked_runner_still_persists_the_metadata() {
    let _guard = ENV_LOCK.blocking_lock();
    let (_tmp, global, project) = home_and_project("one-agent", WORKFLOW_ONE_AGENT_STEP);
    let run_id = "run_attach_loop_panicked_runner";
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));

    let mut rupu = Rupu::workflow_run(
        "one-agent",
        run_id,
        &global,
        &project,
        Some(PANICKING_PROVIDER),
        Stdio::null(),
    );
    let (status, stderr) = rupu.wait(Duration::from_secs(60));

    assert!(!status.success(), "the command fails; stderr: {stderr}");
    assert!(
        stderr.contains("workflow task panicked"),
        "the panic is what the command returned: {stderr}"
    );
    let record = store.load(run_id).unwrap();
    assert_eq!(
        record.status,
        rupu_orchestrator::RunStatus::Failed,
        "the panic ended the run on disk, which is what let the printer return"
    );
    assert_eq!(
        record.error_message.as_deref(),
        Some("workflow runner panicked: provider blew up mid-send")
    );
    assert_metadata_persisted(&store, run_id);
}

/// The inline approve-resume reads `step_results.jsonl` back before it
/// resumes; a line that is not UTF-8, appended while the run is parked at
/// the gate, fails that read after the operator's `a` (as root too, unlike
/// a permission bit). The run still ends with its portable metadata
/// persisted — the manifest written without the step transcripts it could
/// not enumerate, and saying so — and the command fails with that read's
/// error.
///
/// The seam is `RunStore::read_step_results` failing on a line that is not
/// UTF-8, where it skips a line that is merely not JSON: a read that skipped
/// both would make this test fail, and want another way to fail the read.
#[test]
fn an_unreadable_step_results_file_after_the_approve_still_persists_the_metadata() {
    let _guard = ENV_LOCK.blocking_lock();
    let (_tmp, global, project) = home_and_project("gated", WORKFLOW_GATED);
    let run_id = "run_attach_loop_unreadable_results";
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));

    let mut rupu = Rupu::workflow_run("gated", run_id, &global, &project, None, Stdio::piped());
    let mut stdin = rupu.child.stdin.take().expect("piped stdin");

    // Parked at the gate: the line printer is waiting on stdin.
    wait_for_status(&store, run_id, "awaiting_approval", Duration::from_secs(30));
    let results = store.root.join(run_id).join("step_results.jsonl");
    assert!(results.is_file(), "the build step's result is on disk");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&results)
        .unwrap()
        .write_all(b"\xff\xfe not utf-8\n")
        .unwrap();

    // The operator approves; the inline resume then fails to read the
    // results back.
    if let Err(e) = stdin.write_all(b"a\n") {
        panic!(
            "rupu stopped reading before the approve ({e}); stderr: {}",
            std::fs::read_to_string(&rupu.stderr).unwrap_or_default()
        );
    }
    drop(stdin);
    let (status, stderr) = rupu.wait(Duration::from_secs(60));

    assert!(!status.success(), "the command fails; stderr: {stderr}");
    assert!(
        stderr.contains("read step results for resume"),
        "the read's error is what the command returned: {stderr}"
    );
    let manifest = assert_metadata_persisted(&store, run_id);
    assert!(
        manifest.contains("step_results_error"),
        "the manifest says its step transcripts are missing: {manifest}"
    );
}

/// The no-UI invocation — a cron tick inside `cp serve`, a webhook, an
/// autoflow cycle — whose runner panics gets the panic back as the run's
/// error, with the run marked failed and its portable metadata persisted
/// (the trigger's wake id included). Before, the panic unwound into the
/// caller — in `cp serve`, its cron loop, for good — and left the run
/// `Running` under that live process's pid, which no reaper ends.
#[tokio::test(flavor = "multi_thread")]
async fn a_no_ui_run_whose_runner_panics_returns_the_panic_and_persists_the_metadata() {
    let _guard = ENV_LOCK.lock().await;
    let (_tmp, global, project) = home_and_project("one-agent", WORKFLOW_ONE_AGENT_STEP);
    let run_id = "run_no_ui_panicked_runner";
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    std::env::set_var("RUPU_HOME", &global);
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", PANICKING_PROVIDER);
    let ctx = rupu_cli::cmd::workflow::ExplicitWorkflowRunContext {
        project_root: Some(project.clone()),
        workspace_path: project.clone(),
        workspace_id: "ws_no_ui".into(),
        inputs: Vec::new(),
        mode: "bypass".into(),
        invocation_source: rupu_runtime::RunTriggerSource::CronEvent,
        event: None,
        issue: None,
        issue_ref: None,
        system_prompt_suffix: None,
        attach_ui: false,
        run_id_override: Some(run_id.into()),
        plain: false,
        strict_templates: false,
        run_envelope_template: Some(rupu_cli::cmd::workflow::RunEnvelopeTemplate {
            wake_id: Some("wake_42".into()),
            ..Default::default()
        }),
        worker: None,
        live_event_hook: None,
        shared_printer: None,
        live_view: rupu_cli::cmd::ui::LiveViewMode::Focused,
    };

    let err = rupu_cli::cmd::workflow::run_with_explicit_context("one-agent", ctx)
        .await
        .expect_err("the panic is the run's error");

    assert!(
        format!("{err:#}").contains("provider blew up mid-send"),
        "the panic's message is the error: {err:#}"
    );
    let record = store.load(run_id).unwrap();
    assert_eq!(record.status, rupu_orchestrator::RunStatus::Failed);
    assert_eq!(record.runner_pid, None);
    assert_eq!(record.source_wake_id.as_deref(), Some("wake_42"));
    assert_metadata_persisted(&store, run_id);
}
