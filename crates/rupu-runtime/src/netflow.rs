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

use std::collections::HashSet;
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
pub fn for_run(
    global: &Path,
    project_root: Option<&Path>,
    run_id: &str,
    transcript_path: &Path,
) -> RunNetflow {
    let netflow_dir = rupu_netflow::netflow_dir(global, project_root);
    let paths = rupu_netflow::NetflowPaths::for_run(&netflow_dir, run_id);
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
/// The transcript copy is the flow as it was first recorded: a streamed
/// body's final size and duration were only ever in the ledger's
/// `complete` line, which is gone with the line it completes. A ledger
/// whose last line is torn (a writer killed mid-line) gets a newline first,
/// so the torn line stays the only bad one.
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
    for_each_line(&paths.flows, |line| {
        torn = line.last() != Some(&b'\n');
        if let Ok(LedgerIdLine {
            kind: LedgerKind::Flow,
            id: Some(id),
        }) = serde_json::from_slice(line)
        {
            wanted.remove(&id);
        }
    })?;
    if wanted.is_empty() {
        return Ok(0);
    }
    let mut ledger: Option<std::io::BufWriter<std::fs::File>> = None;
    let mut added = 0;
    let mut failed = None;
    for_each_line(transcript, |line| {
        if failed.is_some() || !transcript_flow_id(line).is_some_and(|id| wanted.remove(&id)) {
            return;
        }
        let Ok(rupu_transcript::Event::NetFlow { flow }) = serde_json::from_slice(line) else {
            return;
        };
        if let Err(e) = append_flow(&mut ledger, paths, torn, flow) {
            failed = Some(e);
            return;
        }
        added += 1;
    })?;
    if let Some(e) = failed {
        return Err(e);
    }
    if let Some(mut ledger) = ledger {
        ledger.flush()?;
    }
    Ok(added)
}

/// Append `flow` as a ledger `flow` line, opening the ledger on the first
/// one (after a newline when its last line is torn).
fn append_flow(
    ledger: &mut Option<std::io::BufWriter<std::fs::File>>,
    paths: &rupu_netflow::NetflowPaths,
    torn: bool,
    flow: Box<rupu_netflow::FlowRecord>,
) -> std::io::Result<()> {
    let w = match ledger {
        Some(w) => w,
        None => {
            paths.ensure_dir()?;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&paths.flows)?;
            let w = ledger.insert(std::io::BufWriter::with_capacity(1 << 16, file));
            if torn {
                w.write_all(b"\n")?;
            }
            w
        }
    };
    serde_json::to_writer(&mut *w, &rupu_netflow::LedgerLine::Flow(flow))?;
    w.write_all(b"\n")
}

/// A ledger line read for its flow id only.
#[derive(serde::Deserialize)]
struct LedgerIdLine {
    #[serde(rename = "type")]
    kind: LedgerKind,
    #[serde(default)]
    id: Option<rupu_netflow::FlowId>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum LedgerKind {
    Flow,
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
