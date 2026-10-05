//! Live end-to-end proof for subprocess netflow capture (Plan 5): a real
//! `rupu run` whose agent's `bash` tool runs a real `curl` must leave an
//! attributed `Fidelity::Socket` flow in the run's netflow ledger.
//!
//! Nothing here is mocked except the LLM (the mock provider emits one bash
//! tool call, then stops): the real macOS network-statistics backend, the
//! real bash tool, the real runner and the real per-run ledger. It needs
//! network access to example.com and `curl`, so it is `#[ignore]`d; run it
//! on a Mac with
//!
//! ```text
//! cargo test -p rupu-cli --test serial -- --ignored a_bash_curl_flow_is_captured_and_attributed
//! ```
//!
//! This also folds the Plan-1 deferred end-to-end `SocketComplete`
//! assertion: the captured flow must come back from `read_flows` with its
//! completion folded in (`body_complete`, byte counts).

#![cfg(target_os = "macos")]

use crate::ENV_LOCK;
use assert_cmd::Command;
use rupu_netflow::{Fidelity, FlowRecord, Origin};
use std::path::Path;
use std::time::{Duration, Instant};

const RUN_ID: &str = "run_netflow_live";
const CALL_ID: &str = "call_live_curl";

/// One bash tool call, then a closing answer. `--limit-rate` holds the
/// connection ESTABLISHED for a few seconds: the watcher only sees sockets
/// that are alive across a poll, and a bare `curl http://example.com`
/// finishes in ~100ms and is usually missed.
fn script() -> String {
    serde_json::json!([
        { "AssistantToolUse": {
            "text": null,
            "tool_id": CALL_ID,
            "tool_name": "bash",
            "tool_input": {
                "command": "curl -s -o /dev/null --limit-rate 300 --max-time 5 http://example.com || true"
            },
            "stop": "tool_use"
        } },
        { "AssistantText": { "text": "fetched", "stop": "end_turn" } }
    ])
    .to_string()
}

/// The captured flows for the run, polled until a socket flow with a folded
/// completion shows up (the watcher polls and counts lag a little behind
/// the process exiting) or the deadline passes.
fn wait_for_flows(ledger: &Path) -> Vec<FlowRecord> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let flows = rupu_netflow::ledger::read_flows(ledger).unwrap_or_default();
        let done = flows.iter().any(|f| {
            f.fidelity == Fidelity::Socket
                && f.ctx.tool_call_id.as_deref() == Some(CALL_ID)
                && f.body_complete
        });
        if done || Instant::now() >= deadline {
            return flows;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live: needs network access to example.com, curl and the macOS socket watcher"]
async fn a_bash_curl_flow_is_captured_and_attributed() {
    let _guard = ENV_LOCK.lock().await;

    let dir = tempfile::tempdir().unwrap();
    let agents = dir.path().join(".rupu/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("fetcher.md"),
        "---\nname: fetcher\nprovider: anthropic\nmodel: claude-sonnet-4-6\n\
         maxTurns: 3\ntools: [bash]\n---\nyou fetch.\n",
    )
    .unwrap();
    // Project-local ledger: `.rupu/netflow/` must already exist (opt-in gate).
    std::fs::create_dir_all(dir.path().join(".rupu/netflow")).unwrap();
    std::fs::write(
        dir.path().join(".rupu/config.toml"),
        "[netflow]\nsubprocess_capture = true\n",
    )
    .unwrap();

    // A child process: its own `net_capture::shared` OnceLock, so the real
    // backend starts fresh and the env vars never touch this process.
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(dir.path())
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script())
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .env_remove("RUPU_NETFLOW_SUBPROCESS")
        .write_stdin("")
        .args([
            "run", "fetcher", "--mode", "bypass", "--run-id", RUN_ID, "fetch it",
        ])
        .assert()
        .success();

    let ledger = dir
        .path()
        .join(".rupu/netflow")
        .join(format!("{RUN_ID}.jsonl"));
    let flows = wait_for_flows(&ledger);
    let raw = std::fs::read_to_string(&ledger).unwrap_or_default();

    let socket: Vec<&FlowRecord> = flows
        .iter()
        .filter(|f| f.fidelity == Fidelity::Socket)
        .collect();
    assert!(
        !socket.is_empty(),
        "no Socket flow captured (requires network access to example.com:80 and curl; \
         the ledger dump below shows what was captured); flows: {flows:#?}\nledger:\n{raw}"
    );

    let flow = socket
        .iter()
        .find(|f| f.port == 80 && f.ctx.tool_call_id.as_deref() == Some(CALL_ID))
        .unwrap_or_else(|| panic!("no port-80 flow attributed to {CALL_ID}: {socket:#?}\n{raw}"));

    assert!(
        matches!(&flow.ctx.origin, Origin::Subprocess(name) if name == "curl"),
        "origin should be Subprocess(curl): {flow:#?}"
    );
    assert_eq!(flow.ctx.run_id.as_deref(), Some(RUN_ID), "{flow:#?}");
    // `Ok` means the established+completed path (the Complete fold), not the
    // tracker's never-established branch that also emits `body_complete`.
    assert_eq!(
        flow.outcome,
        rupu_netflow::Outcome::Ok,
        "captured flow should be Ok, got {:?}",
        flow.outcome
    );
    assert!(
        flow.body_complete,
        "the SocketComplete must be folded in: {flow:#?}\n{raw}"
    );
    assert!(
        flow.bytes_in.is_some_and(|n| n > 0),
        "a completed curl flow carries inbound bytes: {flow:#?}"
    );
    assert!(flow.bytes_out.is_some_and(|n| n > 0), "{flow:#?}");
    assert!(flow.duration_ms.is_some(), "{flow:#?}");
    eprintln!("CAPTURED FLOW: {flow:#?}");
}
