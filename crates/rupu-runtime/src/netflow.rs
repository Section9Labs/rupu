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

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One run's netflow capture: the sink its provider, SCM registry and
/// `bash` record into, the ledger writer behind it, and where its two
/// copies of every flow land. Built by [`for_run`]; [`RunNetflow::finish`]
/// ends it.
pub struct RunNetflow {
    pub sink: Arc<dyn rupu_netflow::FlowSink>,
    writer: Option<rupu_netflow::NetflowWriterHandle>,
    paths: rupu_netflow::NetflowPaths,
    transcript: PathBuf,
}

impl RunNetflow {
    /// End the run's capture: flush and close the ledger writer, then
    /// append to the ledger every flow the transcript holds that the ledger
    /// lacks ([`backfill_ledger`]), so every reader of the ledger sees the
    /// run's whole traffic even when the writer never opened or dropped
    /// lines. Call it once the run will record nothing more.
    pub async fn finish(self) {
        if let Some(writer) = self.writer {
            writer.shutdown().await;
        }
        let RunNetflow {
            paths, transcript, ..
        } = self;
        let ledger = paths.flows.clone();
        let backfilled =
            tokio::task::spawn_blocking(move || backfill_ledger(&paths, &transcript)).await;
        match backfilled {
            Ok(Ok(0)) => {}
            Ok(Ok(n)) => {
                tracing::info!(ledger = ?ledger, flows = n, "netflow ledger backfilled from the transcript")
            }
            Ok(Err(e)) => {
                tracing::debug!(error = %e, ledger = ?ledger, "netflow ledger backfill failed")
            }
            Err(e) => tracing::debug!(error = %e, "netflow ledger backfill task failed"),
        }
    }
}

/// Build the netflow sink for one run: a ledger writer rooted at
/// [`rupu_netflow::netflow_dir`] (project-local when
/// `<project>/.rupu/netflow/` already exists, global otherwise, so a repo
/// that was never `rupu init`'d never gets a ledger written inside it) plus a
/// `TranscriptSink` streaming into the run's own transcript.
///
/// Best-effort: a ledger that cannot be opened logs at debug and the run
/// continues with transcript-only capture. Capture must never break a run.
/// [`RunNetflow::finish`] copies what the ledger missed from the transcript
/// once the run is over.
///
/// `stream` is the coordinator-launched root's netflow stream this run tees
/// every ledger line into — backfilled lines included (spec 2026-10-14
/// remote netflow transport §N1; [`stream_for`] / [`link_stream`] resolve
/// it), `None` to write the ledger alone.
pub fn for_run(
    global: &Path,
    project_root: Option<&Path>,
    run_id: &str,
    transcript_path: &Path,
    stream: Option<rupu_netflow::NetflowStream>,
) -> RunNetflow {
    let netflow_dir = rupu_netflow::netflow_dir(global, project_root);
    let paths = rupu_netflow::NetflowPaths::for_run(&netflow_dir, run_id).with_stream(stream);
    let mut sinks: Vec<Arc<dyn rupu_netflow::FlowSink>> = vec![Arc::new(
        rupu_transcript::TranscriptSink::new(transcript_path.to_path_buf()),
    )];
    let writer = match rupu_netflow::NetflowWriterHandle::spawn(paths.clone()) {
        Ok(handle) => {
            sinks.push(handle.writer.clone());
            Some(handle)
        }
        Err(e) => {
            tracing::debug!(error = %e, run_id, "netflow ledger unavailable for this run");
            None
        }
    };
    RunNetflow {
        sink: Arc::new(rupu_netflow::FanoutSink::new(sinks)),
        writer,
        paths,
        transcript: transcript_path.to_path_buf(),
    }
}

/// Append to `paths`' ledger every flow `transcript` records whose id the
/// ledger has no `flow` line for, in transcript order; returns how many.
///
/// Every flow is recorded through exactly one run's sink, into that run's
/// transcript and ledger, so once the run's writer is closed nothing else
/// writes these ids to this ledger: another writer on the same file (an
/// agentiflow's launch sink beside its rounds') records other ids. Reading
/// the whole ledger before appending makes a second call, or a later run
/// under the same id over the same transcript, append nothing.
///
/// The transcript copy is the flow as it was first recorded. A streamed
/// body's final size and duration are only in the ledger's `complete` (or
/// `socket_complete`) line: one that reached the ledger without its flow
/// line is appended again after the flow, and one that was lost too stays
/// lost. The ledger's `dropped` lines counted the flows it restores, so it
/// ends with a `recovered` line taking them back off, never more than those
/// lines still count. A ledger whose last line is torn (a writer killed mid-line) gets
/// a newline first, so the torn line stays the only bad one.
///
/// A run can record hundreds of thousands of flows, so both files are
/// first read for flow ids only, and the transcript is read a second time,
/// parsing whole records, only when the ledger lacks one.
pub fn backfill_ledger(
    paths: &rupu_netflow::NetflowPaths,
    transcript: &Path,
) -> std::io::Result<usize> {
    let mut wanted: HashSet<rupu_netflow::FlowId> = HashSet::new();
    for_each_line(transcript, |line| {
        if let Some(id) = transcript_flow_id(line) {
            wanted.insert(id);
        }
    })?;
    if wanted.is_empty() {
        return Ok(0);
    }
    let mut torn = false;
    let mut unrecovered = 0u64;
    let mut patches: HashMap<rupu_netflow::FlowId, Vec<Vec<u8>>> = HashMap::new();
    for_each_line(&paths.flows, |line| {
        torn = line.last() != Some(&b'\n');
        let Ok(LedgerIdLine { kind, id, count }) = serde_json::from_slice(line) else {
            return;
        };
        match (kind, count) {
            (LedgerKind::Dropped, Some(n)) => unrecovered += n,
            (LedgerKind::Recovered, Some(n)) => unrecovered = unrecovered.saturating_sub(n),
            _ => {}
        }
        let Some(id) = id else {
            return;
        };
        match kind {
            LedgerKind::Flow => {
                wanted.remove(&id);
                patches.remove(&id);
            }
            LedgerKind::Complete | LedgerKind::SocketComplete if wanted.contains(&id) => {
                let mut patch = line.to_vec();
                if patch.last() != Some(&b'\n') {
                    patch.push(b'\n');
                }
                patches.entry(id).or_default().push(patch);
            }
            _ => {}
        }
    })?;
    if wanted.is_empty() {
        return Ok(0);
    }
    let mut ledger = Appender {
        paths,
        torn,
        file: None,
        buf: Vec::new(),
    };
    let mut added = 0;
    let mut failed = None;
    for_each_line(transcript, |line| {
        if failed.is_some() {
            return;
        }
        let Some(id) = transcript_flow_id(line).filter(|id| wanted.remove(id)) else {
            return;
        };
        let Ok(rupu_transcript::Event::NetFlow { flow }) = serde_json::from_slice(line) else {
            return;
        };
        match ledger.push(flow, patches.remove(&id).unwrap_or_default()) {
            Ok(()) => added += 1,
            Err(e) => failed = Some(e),
        }
    })?;
    if let Some(e) = failed {
        return Err(e);
    }
    ledger.recovered(unrecovered.min(added as u64))?;
    ledger.flush()?;
    Ok(added)
}

/// Appends backfilled lines to a ledger, opened on the first one (after a
/// newline when its last line is torn). Lines go out in writes of whole
/// lines only, so another writer appending to the same ledger (an
/// agentiflow's launch sink beside its rounds') can't land inside one.
struct Appender<'a> {
    paths: &'a rupu_netflow::NetflowPaths,
    torn: bool,
    file: Option<std::fs::File>,
    buf: Vec<u8>,
}

impl Appender<'_> {
    const FLUSH_AT: usize = 1 << 16;

    /// `flow` as a `flow` line, then the `complete` / `socket_complete`
    /// lines for it the ledger already had: a reader folds one into the
    /// flow line before it, and these came before no flow line.
    fn push(
        &mut self,
        flow: Box<rupu_netflow::FlowRecord>,
        patches: Vec<Vec<u8>>,
    ) -> std::io::Result<()> {
        serde_json::to_writer(&mut self.buf, &rupu_netflow::LedgerLine::Flow(flow))?;
        self.buf.push(b'\n');
        for patch in patches {
            self.buf.extend_from_slice(&patch);
        }
        if self.buf.len() >= Self::FLUSH_AT {
            self.flush()?;
        }
        Ok(())
    }

    /// A `recovered` line, so the ledger's dropped count stops counting
    /// `count` flows it holds again.
    fn recovered(&mut self, count: u64) -> std::io::Result<()> {
        if count == 0 {
            return Ok(());
        }
        let line = rupu_netflow::LedgerLine::Recovered {
            count,
            ts: chrono::Utc::now(),
        };
        serde_json::to_writer(&mut self.buf, &line)?;
        self.buf.push(b'\n');
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let file = match &mut self.file {
            Some(file) => file,
            None => {
                self.paths.ensure_dir()?;
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.paths.flows)?;
                if self.torn {
                    file.write_all(b"\n")?;
                }
                self.file.insert(file)
            }
        };
        file.write_all(&self.buf)?;
        // A coordinator-launched run's backfilled lines reach its stream
        // too, or a remote coordinator would never see them (spec
        // 2026-10-14 §N1). Best-effort, like every stream write.
        if let Some(stream) = &self.paths.stream {
            if let Err(e) = rupu_netflow::stream::append_stream_raw_lines(
                &stream.path,
                &stream.ledger_id,
                &self.buf,
            ) {
                tracing::warn!(error = %e, "netflow stream write of backfilled lines failed; a coordinator will see this run's flows as partial");
            }
        }
        self.buf.clear();
        Ok(())
    }
}

/// A ledger line read for its flow id only.
#[derive(serde::Deserialize)]
struct LedgerIdLine {
    #[serde(rename = "type")]
    kind: LedgerKind,
    #[serde(default)]
    id: Option<rupu_netflow::FlowId>,
    #[serde(default)]
    count: Option<u64>,
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum LedgerKind {
    Flow,
    Complete,
    SocketComplete,
    Dropped,
    Recovered,
    #[serde(other)]
    Other,
}

/// A transcript `net_flow` line read for its flow id only.
#[derive(serde::Deserialize)]
struct TranscriptFlowLine {
    data: TranscriptFlowData,
}

#[derive(serde::Deserialize)]
struct TranscriptFlowData {
    flow: FlowIdOnly,
}

#[derive(serde::Deserialize)]
struct FlowIdOnly {
    id: rupu_netflow::FlowId,
}

/// The flow id of a transcript `net_flow` line. The transcript writer puts
/// an event's `type` first, so any other line is passed over unparsed.
fn transcript_flow_id(line: &[u8]) -> Option<rupu_netflow::FlowId> {
    if !line.starts_with(br#"{"type":"net_flow","#) {
        return None;
    }
    serde_json::from_slice::<TranscriptFlowLine>(line)
        .ok()
        .map(|l| l.data.flow.id)
}

/// Call `f` with each line of `path`, its newline included. A missing file
/// has no lines.
fn for_each_line(path: &Path, mut f: impl FnMut(&[u8])) -> std::io::Result<()> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut reader = BufReader::with_capacity(1 << 16, file);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        f(&line);
    }
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
