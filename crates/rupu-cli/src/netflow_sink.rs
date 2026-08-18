//! The shared per-run netflow sink builder.
//!
//! Every agent-driven entry point in this crate (`rupu run`, `rupu
//! session`'s per-turn worker, `rupu workflow run`/`resume`, autoflow via
//! `DefaultStepFactory`, sub-agent dispatch) needs to hand its provider —
//! and its SCM registry — an `Arc<dyn rupu_netflow::FlowSink>` scoped to
//! exactly the run doing the calling.
//!
//! # The rule
//!
//! A sink built by [`for_run`] belongs to exactly ONE run. Never install
//! it once and reuse it across multiple runs/turns in a long-lived
//! process (`rupu session`, `rupu cp serve`) — that reintroduces the
//! first-call-wins `OnceLock` defect this plan removes (every later run's
//! flows would silently land in the first run's ledger). Callers that
//! host many runs in one process (a session worker's turn loop, a
//! workflow's step-by-step dispatch) must call this once PER run/turn,
//! not once at daemon start.
//!
//! Where genuinely no run exists yet (a bare CLI read command, a
//! long-lived registry that outlives every run, self-update/login
//! traffic), pass `Arc::new(rupu_netflow::NullSink)` explicitly instead —
//! never a sink built here for a run that hasn't started.

use std::path::Path;
use std::sync::Arc;

use crate::paths;

/// Build the netflow sink for one run: a ledger writer rooted at
/// [`paths::netflow_dir`] (project-local when `<project>/.rupu/netflow/`
/// already exists, global otherwise — same rule as `transcripts_dir`, so
/// a repo that was never `rupu init`'d never gets a ledger written inside
/// it) plus a `TranscriptSink` streaming into this run's own transcript.
///
/// Best-effort — a ledger that cannot be opened logs at debug and the run
/// continues with transcript-only capture. Capture must never break a
/// run.
///
/// Returns the composed sink for the caller to hand to
/// `provider_factory::build_for_provider_with_config` / `Registry::discover`
/// / connector construction, plus the `NetflowWriterHandle` (when the
/// ledger opened) so the caller can `shutdown()` it once the run is over
/// for a prompt flush — the writer task's own periodic ticker is a safety
/// net for long-running daemons, not a substitute for an explicit
/// shutdown on a run/turn that may finish within milliseconds of the
/// ticker's next tick. `None` means the ledger was unavailable
/// (best-effort degrade to transcript-only capture); there is nothing to
/// shut down.
pub fn for_run(
    global: &Path,
    project_root: Option<&Path>,
    run_id: &str,
    transcript_path: &Path,
) -> (
    Arc<dyn rupu_netflow::FlowSink>,
    Option<rupu_netflow::NetflowWriterHandle>,
) {
    let netflow_dir = paths::netflow_dir(global, project_root);
    let netflow_paths = rupu_netflow::NetflowPaths::for_run(&netflow_dir, run_id);
    let mut sinks: Vec<Arc<dyn rupu_netflow::FlowSink>> = vec![Arc::new(
        rupu_transcript::TranscriptSink::new(transcript_path.to_path_buf()),
    )];
    let handle = match rupu_netflow::NetflowWriterHandle::spawn(netflow_paths) {
        Ok(handle) => {
            sinks.push(handle.writer.clone());
            Some(handle)
        }
        Err(e) => {
            tracing::debug!(error = %e, run_id, "netflow ledger unavailable for this run");
            None
        }
    };
    (Arc::new(rupu_netflow::FanoutSink::new(sinks)), handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Copied from `rupu-netflow/src/ledger/writer.rs`'s test module and
    /// parameterised on `host` — see that module for the canonical
    /// literal this mirrors.
    fn flow(host: &str) -> rupu_netflow::FlowRecord {
        rupu_netflow::FlowRecord {
            id: rupu_netflow::FlowId::new(),
            ts: chrono::Utc::now(),
            ctx: rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Update),
            fidelity: rupu_netflow::Fidelity::Http,
            method: "GET".into(),
            scheme: "https".into(),
            host: host.into(),
            port: 443,
            path: "/".into(),
            peer_ip: None,
            resolved_ips: vec![],
            http_version: None,
            status: Some(200),
            outcome: rupu_netflow::Outcome::Ok,
            error: None,
            bytes_out: None,
            bytes_in: None,
            body_complete: false,
            ttfb_ms: None,
            duration_ms: None,
        }
    }

    #[tokio::test]
    async fn two_runs_in_one_process_get_separate_ledgers() {
        // Proves the BUILDER itself: two direct `for_run` calls never
        // collide on a shared `OnceLock`-style sink. It does NOT prove any
        // real entry point actually calls `for_run` per run/turn rather
        // than once at process start -- a `run_turn` (or a future daemon
        // path) that regressed to building one sink before its turn/
        // request loop would still pass THIS test, since it never touches
        // `run_turn` at all.
        // `crates/rupu-cli/tests/netflow_run.rs`'s
        // `two_sequential_rupu_run_invocations_in_one_process_get_separate_ledgers`
        // is the one that drives a real entry point (`rupu_cli::run`)
        // twice in one process and is the actual regression guard for
        // that class of bug.
        let tmp = tempfile::TempDir::new().unwrap();
        let global = tmp.path().join("global");
        let t_a = tmp.path().join("a.jsonl");
        let t_b = tmp.path().join("b.jsonl");
        rupu_transcript::JsonlWriter::create(&t_a).unwrap();
        rupu_transcript::JsonlWriter::create(&t_b).unwrap();

        let (sink_a, h_a) = for_run(&global, None, "run-a", &t_a);
        let (sink_b, h_b) = for_run(&global, None, "run-b", &t_b);

        rupu_netflow::FlowSink::record(sink_a.as_ref(), flow("a.example")).await;
        rupu_netflow::FlowSink::record(sink_b.as_ref(), flow("b.example")).await;
        if let Some(h) = h_a {
            h.shutdown().await;
        }
        if let Some(h) = h_b {
            h.shutdown().await;
        }

        let a = rupu_netflow::ledger::read_flows(&global.join("netflow/run-a.jsonl")).unwrap();
        let b = rupu_netflow::ledger::read_flows(&global.join("netflow/run-b.jsonl")).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].host, "a.example");
        assert_eq!(b[0].host, "b.example");
    }
}
