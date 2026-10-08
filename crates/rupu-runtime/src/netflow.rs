//! The per-run netflow sink.
//!
//! A sink built by [`for_run`] belongs to exactly ONE run. Never install it
//! once and reuse it across runs in a long-lived process (`rupu session`,
//! `rupu cp serve`): every later run's flows would land in the first run's
//! ledger. The run assembler builds one per run it assembles unless the
//! launch site hands it the one it already built (a top-level run's sink
//! exists before its SCM registry and provider, which both take it).
//!
//! Where genuinely no run exists (a bare CLI read command, a long-lived
//! registry that outlives every run, self-update/login traffic), pass
//! `Arc::new(rupu_netflow::NullSink)` instead.

use std::path::Path;
use std::sync::Arc;

/// Build the netflow sink for one run: a ledger writer rooted at
/// [`rupu_netflow::netflow_dir`] (project-local when
/// `<project>/.rupu/netflow/` already exists, global otherwise, so a repo
/// that was never `rupu init`'d never gets a ledger written inside it) plus a
/// `TranscriptSink` streaming into the run's own transcript.
///
/// Best-effort: a ledger that cannot be opened logs at debug and the run
/// continues with transcript-only capture. Capture must never break a run.
///
/// Returns the composed sink, plus the `NetflowWriterHandle` when the ledger
/// opened, so the owner can `shutdown()` it once the run is over for a
/// prompt flush. `None` means there is nothing to shut down.
pub fn for_run(
    global: &Path,
    project_root: Option<&Path>,
    run_id: &str,
    transcript_path: &Path,
) -> (
    Arc<dyn rupu_netflow::FlowSink>,
    Option<rupu_netflow::NetflowWriterHandle>,
) {
    let netflow_dir = rupu_netflow::netflow_dir(global, project_root);
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
