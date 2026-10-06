//! `rupu workflow resume` and the agent attempt a dead runner left mid-step
//! (recover-on-interrupt spec §§3-4): a run whose runner died is reaped (so the
//! CLI alone can recover it, no `cp serve` sweep needed), and the step it was
//! in the middle of is *continued* from its transcript — unless
//! `--restart-interrupted` asks for it to start over.
//!
//! Driven through the real `rupu` binary with the mock-provider seam. The run
//! is built on disk the way a crash leaves it: `Running` with a dead
//! `runner_pid`, an `attempts.jsonl` row for the step in flight, and that
//! attempt's transcript cut off before its first turn.

use crate::ENV_LOCK;
use assert_cmd::Command;
use predicates::prelude::*;
use rupu_orchestrator::executor::{AttemptResumeMode, Event};
use rupu_orchestrator::runs::AttemptRecord;
use rupu_orchestrator::{RunRecord, RunStatus, RunStore};
use rupu_transcript::{Event as TranscriptEvent, JsonlWriter, RunMode};
use std::path::{Path, PathBuf};

const WORKFLOW: &str = r#"
name: crashed
steps:
  - id: only
    agent: worker
    prompt: "do the thing"
"#;

const RUN_ID: &str = "run_crashed";
/// The agent run the dead runner was in the middle of.
const INTERRUPTED: &str = "run_interrupted";
/// A pid no process has (see `pid_is_running`).
const DEAD_PID: u32 = u32::MAX;

struct Fixture {
    _tmp: tempfile::TempDir,
    global: PathBuf,
    project: PathBuf,
    store: RunStore,
}

impl Fixture {
    /// `RUPU_HOME` plus a project with a `worker` agent, and the crashed run.
    fn crashed(runner_pid: u32) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join(".rupu");
        let project = tmp.path().join("proj");
        let agents = project.join(".rupu/agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("worker.md"),
            "---\nname: worker\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nYou are a test agent.\n",
        )
        .unwrap();
        let transcripts = global.join("transcripts");
        std::fs::create_dir_all(&transcripts).unwrap();

        let now = chrono::Utc::now();
        let record: RunRecord = serde_json::from_value(serde_json::json!({
            "id": RUN_ID,
            "workflow_name": "crashed",
            "status": "running",
            "inputs": {},
            "workspace_id": "ws_test",
            "workspace_path": project,
            "transcript_dir": transcripts,
            "started_at": now,
            "runner_pid": runner_pid,
            "active_step_id": "only",
            "active_step_agent": "worker",
        }))
        .unwrap();
        let store = RunStore::new(global.join("runs"));
        store.create(record, WORKFLOW).unwrap();

        // The attempt in flight when the runner died: it has not reached its
        // first turn, so its transcript is just the start and the prompt.
        let transcript = transcripts.join(format!("{INTERRUPTED}.jsonl"));
        let mut w = JsonlWriter::create(&transcript).unwrap();
        w.write(&TranscriptEvent::RunStart {
            run_id: INTERRUPTED.into(),
            workspace_id: "ws_test".into(),
            agent: "worker".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            started_at: now,
            mode: RunMode::Bypass,
            schema: None,
            system_prompt: None,
            codename: None,
            customer: None,
        })
        .unwrap();
        w.write(&TranscriptEvent::UserMessage {
            content: "do the thing".into(),
        })
        .unwrap();
        w.flush().unwrap();
        store
            .append_attempt(
                RUN_ID,
                &AttemptRecord {
                    v: 1,
                    step_id: "only".into(),
                    unit_index: None,
                    sub_id: None,
                    agent_run_id: INTERRUPTED.into(),
                    transcript_path: transcript,
                    host: None,
                    continued_from: None,
                    started_at: now,
                },
            )
            .unwrap();
        Self {
            _tmp: tmp,
            global,
            project,
            store,
        }
    }

    /// `rupu workflow resume run_crashed <extra...>`, the provider answering
    /// `answer`.
    fn resume(&self, answer: &str, extra: &[&str]) -> assert_cmd::assert::Assert {
        let script = serde_json::json!([
            { "AssistantText": { "text": answer, "stop": "end_turn" } }
        ])
        .to_string();
        let mut args = vec!["workflow", "resume", RUN_ID, "--plain", "--mode", "bypass"];
        args.extend_from_slice(extra);
        Command::cargo_bin("rupu")
            .unwrap()
            .current_dir(&self.project)
            .env("RUPU_HOME", &self.global)
            .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
            .args(args)
            .assert()
    }

    fn events(&self) -> Vec<Event> {
        let path = self.global.join("runs").join(RUN_ID).join("events.jsonl");
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// `(mode, from_agent_run_id)` of every `attempt_resumed` event.
    fn attempt_resumed(&self) -> Vec<(AttemptResumeMode, Option<String>)> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                Event::AttemptResumed {
                    mode,
                    from_agent_run_id,
                    ..
                } => Some((mode, from_agent_run_id)),
                _ => None,
            })
            .collect()
    }

    /// The `only` step's ledger rows, in order.
    fn attempts(&self) -> Vec<AttemptRecord> {
        self.store.read_attempts(RUN_ID).unwrap()
    }

    /// The transcript of the attempt the resume started (not the interrupted
    /// one), as parsed lines.
    fn resumed_transcript(&self) -> Vec<serde_json::Value> {
        let attempts = self.attempts();
        let new = attempts
            .iter()
            .find(|a| a.agent_run_id != INTERRUPTED)
            .expect("the resume started an attempt");
        read_lines(&new.transcript_path)
    }
}

fn read_lines(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// A crashed run — `Running`, its recorded runner dead — is taken over by a
/// plain `rupu workflow resume`: reaped to `Failed` first, then resumed, and
/// the step it died inside is continued from its transcript.
#[tokio::test]
async fn resume_reaps_a_crashed_run_and_continues_its_interrupted_step() {
    let _guard = ENV_LOCK.lock().await;
    let fx = Fixture::crashed(DEAD_PID);

    // The plan is announced before dispatch, one line per planned step.
    fx.resume("continued answer", &[])
        .success()
        .stdout(predicate::str::contains("only: 1 continued"));

    let record = fx.store.load(RUN_ID).unwrap();
    assert_eq!(record.status, RunStatus::Completed);
    // The reaper's terminal event is in the log: the run was reaped, not
    // resumed over its dead runner.
    assert!(
        fx.events().iter().any(|e| matches!(
            e,
            Event::RunFailed { error, .. } if error.contains("no longer alive")
        )),
        "got {:?}",
        fx.events()
    );

    // The step was continued: announced once, linked in the ledger, and the
    // new run is seeded from the interrupted transcript.
    assert_eq!(
        fx.attempt_resumed(),
        vec![(AttemptResumeMode::Continued, Some(INTERRUPTED.to_string()))]
    );
    let attempts = fx.attempts();
    assert_eq!(
        attempts.len(),
        2,
        "the interrupted attempt + its continuation"
    );
    assert_eq!(attempts[1].continued_from.as_deref(), Some(INTERRUPTED));
    let seed = fx
        .resumed_transcript()
        .into_iter()
        .find(|v| v["type"] == "seed")
        .expect("the continuation is seeded from the interrupted run");
    assert!(seed["data"]["source_transcript"]
        .as_str()
        .unwrap()
        .ends_with(&format!("{INTERRUPTED}.jsonl")));
    let steps = fx.store.read_step_results(RUN_ID).unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].output, "continued answer");
}

/// `--restart-interrupted` skips discovery: the same crashed run is still
/// reaped and resumed, but the step starts over from its prompt.
#[tokio::test]
async fn resume_restart_interrupted_skips_discovery() {
    let _guard = ENV_LOCK.lock().await;
    let fx = Fixture::crashed(DEAD_PID);

    fx.resume("fresh answer", &["--restart-interrupted"])
        .success()
        .stdout(predicate::str::contains("continued").not());

    assert_eq!(fx.store.load(RUN_ID).unwrap().status, RunStatus::Completed);
    assert!(
        fx.attempt_resumed().is_empty(),
        "nothing was continued, recovered or announced as restarted: {:?}",
        fx.attempt_resumed()
    );
    let attempts = fx.attempts();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[1].continued_from, None);
    assert!(
        fx.resumed_transcript().iter().all(|v| v["type"] != "seed"),
        "a restart starts from the prompt, not from the interrupted transcript"
    );
    assert_eq!(
        fx.store.read_step_results(RUN_ID).unwrap()[0].output,
        "fresh answer"
    );
}

/// The reaper only takes over a runner that is dead: a run whose recorded
/// runner is still alive is refused, untouched.
#[tokio::test]
async fn resume_refuses_a_run_whose_runner_is_still_alive() {
    let _guard = ENV_LOCK.lock().await;
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .expect("spawn a live pid");
    let fx = Fixture::crashed(child.id());

    let assert = fx.resume("never sent", &[]).failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    let _ = child.kill();
    let _ = child.wait();

    assert!(stderr.contains("in-flight"), "got: {stderr}");
    let record = fx.store.load(RUN_ID).unwrap();
    assert_eq!(record.status, RunStatus::Running, "left alone");
    assert_eq!(fx.attempts().len(), 1, "nothing was dispatched");
}

/// `--if-unfinished` — how `cp serve` spawns a resume to serve a request made
/// while a run was unfinished — never retries a run that finished since, so it
/// must not *end* a crashed run only to refuse it as finished: the run is
/// refused as still in flight, untouched, and left to the operator's plain
/// `workflow resume` (which does reap it) or the sweep.
#[tokio::test]
async fn a_requested_resume_leaves_a_crashed_run_unreaped() {
    let _guard = ENV_LOCK.lock().await;
    let fx = Fixture::crashed(DEAD_PID);

    let assert = fx.resume("never sent", &["--if-unfinished"]).failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();

    assert!(stderr.contains("in-flight"), "got: {stderr}");
    assert_eq!(
        fx.store.load(RUN_ID).unwrap().status,
        RunStatus::Running,
        "not reaped"
    );
    assert!(
        !fx.events()
            .iter()
            .any(|e| matches!(e, Event::RunFailed { .. })),
        "no terminal event was appended: {:?}",
        fx.events()
    );
    assert_eq!(fx.attempts().len(), 1, "nothing was dispatched");
}
