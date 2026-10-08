//! `host: local` (W6, L12): a placed step on the coordinator's own host runs
//! through the local connector's subprocess launcher — a detached `rupu run`
//! of this binary — instead of being refused with "no agent launcher
//! configured". Driven through the real binary (`CARGO_BIN_EXE_rupu`): the
//! unit is a child `rupu run`, and only the real binary is one.

use crate::ENV_LOCK;
use assert_fs::prelude::*;
use std::process::Command;
use std::time::{Duration, Instant};

const WORKFLOW: &str = r#"
name: placed
steps:
  - id: ask
    agent: echo
    host: local
    prompt: "say hi"
"#;

/// The unit's one reply (each `rupu` process reads the script afresh).
const REPLY: &str =
    r#"[{ "AssistantText": { "text": "hi from the local host", "stop": "end_turn" } }]"#;

#[test]
fn host_local_works() {
    let _env = ENV_LOCK.blocking_lock();
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    // The placed unit is a detached `rupu run` with no tty and no `--mode`
    // (a `UnitDispatch` carries none), so — like any fleet host — this one
    // runs agents under its configured default mode.
    global
        .child("config.toml")
        .write_str("permission_mode = \"bypass\"\n")
        .unwrap();
    let project = tmp.child("proj");
    project
        .child(".rupu/workflows/placed.yaml")
        .write_str(WORKFLOW)
        .unwrap();
    project
        .child(".rupu/agents/echo.md")
        .write_str("---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nyou echo.")
        .unwrap();
    let run_id = "run_01HOSTLOCAL0000000000000";
    let stderr = global.path().join("rupu.stderr");

    let mut child = Command::new(env!("CARGO_BIN_EXE_rupu"))
        .args(["workflow", "run", "placed", "--run-id", run_id, "--plain"])
        .env("RUPU_HOME", global.path())
        .env("RUPU_MOCK_PROVIDER_SCRIPT", REPLY)
        .current_dir(project.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(&stderr).unwrap())
        .spawn()
        .expect("spawn rupu");
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "rupu workflow run did not finish; stderr: {}",
                std::fs::read_to_string(&stderr).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let err = std::fs::read_to_string(&stderr).unwrap_or_default();
    assert!(status.success(), "{status}: {err}");
    assert!(!err.contains("no agent launcher configured"), "{err}");

    let store = rupu_orchestrator::RunStore::new(global.path().join("runs"));
    let record = store.load(run_id).unwrap();
    assert_eq!(
        record.status,
        rupu_orchestrator::RunStatus::Completed,
        "{err}"
    );
    let steps = store.read_step_results(run_id).unwrap();
    let ask = steps
        .iter()
        .find(|s| s.step_id == "ask")
        .expect("the ask step ran");
    assert!(
        ask.output.contains("hi from the local host"),
        "the placed unit's answer is the step's output: {:?}",
        ask.output
    );
}
