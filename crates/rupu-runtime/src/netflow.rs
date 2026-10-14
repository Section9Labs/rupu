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
/// `stream` is the coordinator-launched root's netflow stream this run tees
/// every ledger line into (spec 2026-10-14 remote netflow transport §N1;
/// [`stream_for`] / [`link_stream`] resolve it), `None` to write the ledger
/// alone.
///
/// Returns the composed sink, plus the `NetflowWriterHandle` when the ledger
/// opened, so the owner can `shutdown()` it once the run is over for a
/// prompt flush. `None` means there is nothing to shut down.
pub fn for_run(
    global: &Path,
    project_root: Option<&Path>,
    run_id: &str,
    transcript_path: &Path,
    stream: Option<rupu_netflow::NetflowStream>,
) -> (
    Arc<dyn rupu_netflow::FlowSink>,
    Option<rupu_netflow::NetflowWriterHandle>,
) {
    let netflow_dir = rupu_netflow::netflow_dir(global, project_root);
    let netflow_paths =
        rupu_netflow::NetflowPaths::for_run(&netflow_dir, run_id).with_stream(stream);
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

/// The stream a run tees into when `<root_run_dir>/netflow.jsonl` has been
/// begun by a coordinator-launched root (spec §N1); `None` otherwise — a
/// root that doesn't tee (a terminal `rupu run`, an agentiflow) has no
/// begun stream, so nothing beneath it tees either.
pub fn stream_for(root_run_dir: &Path, ledger_id: &str) -> Option<rupu_netflow::NetflowStream> {
    let path = root_run_dir.join(rupu_netflow::NETFLOW_STREAM_FILE);
    rupu_netflow::stream::stream_has_begin(&path).then(|| rupu_netflow::NetflowStream {
        path,
        ledger_id: ledger_id.to_string(),
    })
}

/// A process child's stream: its root's run directory is the directory of
/// the root's usage ledger its link carries.
pub fn link_stream(
    link: &crate::argv::LaunchLink,
    ledger_id: &str,
) -> Option<rupu_netflow::NetflowStream> {
    let dir = link.usage_root.as_deref()?.parent()?;
    stream_for(dir, ledger_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_for_is_some_only_once_the_root_has_begun() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("runs").join("run_root");
        assert_eq!(stream_for(&root, "run_step"), None, "no file");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("netflow.jsonl"), "").unwrap();
        assert_eq!(stream_for(&root, "run_step"), None, "no begin line");
        rupu_netflow::stream::write_stream_begin(&root.join("netflow.jsonl"), "run_root").unwrap();
        assert_eq!(
            stream_for(&root, "run_step"),
            Some(rupu_netflow::NetflowStream {
                path: root.join("netflow.jsonl"),
                ledger_id: "run_step".into(),
            })
        );
    }

    #[test]
    fn link_stream_follows_the_usage_root_into_the_roots_run_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("runs").join("run_root");
        let link = crate::argv::LaunchLink {
            depth: 1,
            ceiling: rupu_tools::PermissionMode::Bypass,
            parent_run: "run_root".into(),
            root_run: "run_root".into(),
            usage_root: Some(root.join("usage.jsonl")),
            agents: Vec::new(),
            workflows: Vec::new(),
            actions: None,
            no_messaging: false,
        };
        assert_eq!(link_stream(&link, "run_child"), None, "root never began");
        rupu_netflow::stream::write_stream_begin(&root.join("netflow.jsonl"), "run_root").unwrap();
        assert_eq!(
            link_stream(&link, "run_child").map(|s| s.path),
            Some(root.join("netflow.jsonl"))
        );
        let unrooted = crate::argv::LaunchLink {
            usage_root: None,
            ..link
        };
        assert_eq!(
            link_stream(&unrooted, "run_child"),
            None,
            "no usage root, no stream"
        );
    }
}
