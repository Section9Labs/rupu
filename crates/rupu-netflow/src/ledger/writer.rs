use crate::ledger::paths::NetflowPaths;
use crate::record::{FlowId, FlowRecord, LedgerLine};
use crate::sink::FlowSink;
use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const CHANNEL_CAPACITY: usize = 1024;

/// How often the writer task checks for loss and surfaces it, even with
/// no explicit `Flush`/`Stop` and no channel close.
///
/// Load-bearing for every LONG-RUNNING production caller: `rupu cp
/// serve` installs its sink once and never calls `shutdown()` — the
/// process just keeps running — and the global `OnceLock` the sink lives
/// in is never dropped before process exit either, so neither the
/// `Flush`/`Stop` path nor the "all senders dropped" natural-close path
/// (the tail of this file's main loop) was ever reachable in practice.
/// Without an independent timer, a channel-overflow drop could sit
/// silently unrecorded on disk for the entire remaining life of the
/// daemon — invariant 2 ("loss must be visible, never silent"), broken.
const DROPPED_VISIBILITY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug)]
enum WriteRequest {
    Line(Box<LedgerLine>),
    Flush(tokio::sync::oneshot::Sender<()>),
    /// Explicit shutdown signal. `shutdown()` must not rely on the last
    /// `Sender` being dropped to end the task — callers legitimately hold
    /// `Arc<NetflowWriter>` clones (e.g. a client built with `handle.writer.clone()`)
    /// whose lifetime outlives the handle, which would otherwise hang `shutdown`.
    Stop,
}

/// Best-effort append-only ledger sink.
///
/// Writes go through a BOUNDED channel to a background task. When the
/// channel is full the record is dropped and counted — visible loss,
/// surfaced by the UI, never silent loss. Capture must never block or
/// break the request that produced it.
#[derive(Debug)]
pub struct NetflowWriter {
    tx: mpsc::Sender<WriteRequest>,
    /// Shared with the writer task. MUST be a separate `Arc`, not reached
    /// via `Arc<NetflowWriter>` — that cycle deadlocks `shutdown`.
    dropped: Arc<AtomicU64>,
}

impl NetflowWriter {
    /// Records lost to channel overflow since process start.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn offer(&self, line: LedgerLine) {
        if self
            .tx
            .try_send(WriteRequest::Line(Box::new(line)))
            .is_err()
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[async_trait]
impl FlowSink for NetflowWriter {
    async fn record(&self, flow: FlowRecord) {
        self.offer(LedgerLine::Flow(Box::new(flow)));
    }

    async fn complete(&self, id: FlowId, bytes_in: u64, duration_ms: u64) {
        self.offer(LedgerLine::Complete {
            id,
            bytes_in,
            duration_ms,
        });
    }
}

pub struct NetflowWriterHandle {
    pub writer: Arc<NetflowWriter>,
    task: JoinHandle<()>,
}

impl NetflowWriterHandle {
    pub fn spawn(paths: NetflowPaths) -> std::io::Result<Self> {
        Self::spawn_with_capacity(paths, CHANNEL_CAPACITY)
    }

    pub fn spawn_with_capacity(paths: NetflowPaths, capacity: usize) -> std::io::Result<Self> {
        Self::spawn_with_capacity_and_interval(paths, capacity, DROPPED_VISIBILITY_INTERVAL)
    }

    /// As [`Self::spawn_with_capacity`], with an explicit drop-visibility
    /// interval. `pub(crate)` — production callers get the tuned default
    /// above; only the test in this module needs a short interval to stay
    /// fast.
    pub(crate) fn spawn_with_capacity_and_interval(
        paths: NetflowPaths,
        capacity: usize,
        flush_interval: std::time::Duration,
    ) -> std::io::Result<Self> {
        paths.ensure_dir()?;
        let (tx, rx) = mpsc::channel(capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        let writer = Arc::new(NetflowWriter {
            tx,
            dropped: dropped.clone(),
        });
        let task = tokio::spawn(run_writer(paths, rx, dropped, flush_interval));
        Ok(Self { writer, task })
    }

    /// Flush pending writes, emit a final `Dropped` line if anything was
    /// lost, then stop the task.
    ///
    /// Sends an explicit `Stop` after the flush ack rather than relying on
    /// the last `Sender` being dropped — a caller may hold its own
    /// `Arc<NetflowWriter>` clone (via the public `writer` field) whose
    /// `Sender` would otherwise keep the task's `rx.recv()` alive forever.
    pub async fn shutdown(self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.writer.tx.send(WriteRequest::Flush(tx)).await;
        let _ = rx.await;
        let _ = self.writer.tx.send(WriteRequest::Stop).await;
        drop(self.writer);
        let _ = self.task.await;
    }
}

async fn run_writer(
    paths: NetflowPaths,
    mut rx: mpsc::Receiver<WriteRequest>,
    dropped_counter: Arc<AtomicU64>,
    flush_interval: std::time::Duration,
) {
    let mut file = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.flows)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(error = %e, path = ?paths.flows, "netflow ledger unavailable; flows will not be persisted");
            return;
        }
    };

    let mut last_dropped = 0u64;
    // See `DROPPED_VISIBILITY_INTERVAL`'s doc comment: this is the ONLY
    // mechanism that surfaces loss for a caller that never calls
    // `Flush`/`Stop` and never lets the channel close naturally — which,
    // in production, is every caller (`rupu run` keeps the sink installed
    // in a process-wide `OnceLock` for the whole process; `rupu cp serve`
    // runs indefinitely). The `Line` arm below ALSO checks on every
    // successful write, so loss is typically visible on the very next
    // record rather than waiting out a full tick; the interval exists for
    // the case where nothing more is ever successfully sent.
    let mut ticker = tokio::time::interval(flush_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await; // consume the immediate first tick (interval fires at t=0)

    loop {
        tokio::select! {
            biased;
            req = rx.recv() => {
                let Some(req) = req else {
                    // All senders dropped — natural close. Unreachable in
                    // production today (see above) but kept correct for
                    // tests / future callers that DO drop the handle.
                    break;
                };
                match req {
                    WriteRequest::Line(line) => {
                        write_line(&mut file, &line, &dropped_counter).await;
                        last_dropped =
                            flush_dropped(&mut file, &dropped_counter, last_dropped).await;
                    }
                    WriteRequest::Flush(ack) => {
                        last_dropped =
                            flush_dropped(&mut file, &dropped_counter, last_dropped).await;
                        let _ = file.flush().await;
                        let _ = ack.send(());
                    }
                    WriteRequest::Stop => {
                        // Close first, THEN drain. `Stop` is FIFO-ordered
                        // against concurrent producers, so a clone's
                        // `try_send` can land behind it; breaking
                        // immediately would discard that record with no
                        // counter bump — silent loss.
                        //
                        // `close()` makes every subsequent `try_send`
                        // fail, so `offer()` counts those as dropped
                        // (visible), while `recv()` still yields
                        // everything already queued.
                        rx.close();
                        while let Some(pending) = rx.recv().await {
                            if let WriteRequest::Line(line) = pending {
                                write_line(&mut file, &line, &dropped_counter).await;
                            }
                        }
                        break;
                    }
                }
            }
            _ = ticker.tick() => {
                last_dropped = flush_dropped(&mut file, &dropped_counter, last_dropped).await;
            }
        }
    }

    flush_dropped(&mut file, &dropped_counter, last_dropped).await;
    let _ = file.flush().await;
}

/// Compute how many records have been lost since `last_dropped`, write a
/// `Dropped` line covering the delta if any, and return the new total.
/// Shared by the `Flush` request and the terminal drain so the accounting
/// logic isn't duplicated.
async fn flush_dropped(
    file: &mut tokio::fs::File,
    dropped_counter: &AtomicU64,
    last_dropped: u64,
) -> u64 {
    let dropped = dropped_counter.load(Ordering::Relaxed);
    if dropped > last_dropped {
        let line = LedgerLine::Dropped {
            count: dropped - last_dropped,
            ts: chrono::Utc::now(),
        };
        write_line(file, &line, dropped_counter).await;
    }
    dropped
}

/// Append one line. A failure here is REAL LOSS and must be counted —
/// a full disk or revoked permission must not silently vanish a record any
/// more than channel overflow may.
async fn write_line(file: &mut tokio::fs::File, line: &LedgerLine, dropped: &AtomicU64) {
    let json = match serde_json::to_string(line) {
        Ok(mut j) => {
            j.push('\n');
            j
        }
        Err(e) => {
            dropped.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(error = %e, "netflow ledger serialize failed; record lost");
            return;
        }
    };
    if let Err(e) = file.write_all(json.as_bytes()).await {
        dropped.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(error = %e, "netflow ledger write failed; record lost");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::{FlowCtx, Origin};
    use crate::record::{Fidelity, Outcome};

    fn flow(id: FlowId) -> FlowRecord {
        FlowRecord {
            id,
            ts: chrono::Utc::now(),
            ctx: FlowCtx::system(Origin::Update),
            fidelity: Fidelity::Http,
            method: "GET".into(),
            scheme: "https".into(),
            host: "example.test".into(),
            port: 443,
            path: "/releases".into(),
            peer_ip: None,
            resolved_ips: vec![],
            http_version: None,
            status: Some(200),
            outcome: Outcome::Ok,
            error: None,
            bytes_out: None,
            bytes_in: None,
            body_complete: false,
            ttfb_ms: None,
            duration_ms: None,
        }
    }

    #[tokio::test]
    async fn writer_appends_flow_and_complete_lines() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = NetflowPaths::new(tmp.path());
        let handle = NetflowWriterHandle::spawn(paths.clone()).unwrap();

        let id = FlowId::from_parts(9, 9);
        handle.writer.record(flow(id)).await;
        handle.writer.complete(id, 4096, 250).await;
        tokio::time::timeout(std::time::Duration::from_secs(10), handle.shutdown())
            .await
            .expect("writer shutdown deadlocked");

        let text = std::fs::read_to_string(&paths.flows).unwrap();
        let lines: Vec<LedgerLine> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();

        assert_eq!(lines.len(), 2);
        assert!(matches!(&lines[0], LedgerLine::Flow(f) if f.id == id));
        assert!(matches!(
            lines[1],
            LedgerLine::Complete { id: got, bytes_in: 4096, duration_ms: 250 } if got == id
        ));
    }

    #[tokio::test]
    async fn shutdown_drains_a_record_queued_behind_stop() {
        // Regression test for the "Stop discards a concurrently-queued
        // record" bug: `Stop` is FIFO-ordered against other producers, so a
        // caller holding a live `Arc<NetflowWriter>` clone can have its
        // `record()` land BEHIND `Stop` in the channel. An immediate
        // `break` on `Stop` (the pre-fix code) would silently discard that
        // record — no ledger line, no `dropped()` bump.
        //
        // Reproduced DETERMINISTICALLY, not as a timing race: the writer
        // task is spawned but never gets a chance to run until the test
        // task's first genuine yield point (current-thread runtime; no
        // `.await` before this point actually suspends, since `try_send`
        // is synchronous and `record()`'s body has no internal await). So
        // pre-seeding `Stop` via the private `tx` field (this `tests`
        // module is a descendant of `writer`, so it can see private
        // fields), then enqueuing the clone's record through the real
        // `FlowSink::record` path, deterministically produces the exact
        // queue order the bug depends on: `Stop` first, the clone's `Line`
        // right behind it, both sitting unread when the writer task
        // finally runs. Only then is the production `shutdown()` called,
        // exercising the real close-then-drain fix end to end.
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = NetflowPaths::new(tmp.path());
        let handle = NetflowWriterHandle::spawn(paths.clone()).unwrap();
        let writer_clone = handle.writer.clone();

        handle
            .writer
            .tx
            .try_send(WriteRequest::Stop)
            .expect("pre-seeding Stop must succeed before the writer task has run");

        let id = FlowId::from_parts(42, 42);
        writer_clone.record(flow(id)).await;

        tokio::time::timeout(std::time::Duration::from_secs(10), handle.shutdown())
            .await
            .expect("writer shutdown deadlocked");

        let text = std::fs::read_to_string(&paths.flows).unwrap();
        let lines: Vec<LedgerLine> = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();

        assert!(
            lines
                .iter()
                .any(|l| matches!(l, LedgerLine::Flow(f) if f.id == id)),
            "the clone's record must survive shutdown even though it was \
             queued behind Stop; ledger contents: {lines:?}"
        );
    }

    // `write_line`'s own `Err` arm — invariant 2's disk-failure half (spec
    // §10: "a full disk or revoked permission must not silently vanish a
    // record any more than channel overflow may") — is NOT covered by a
    // test here. `dropped_count_is_recorded_not_silent` below only
    // exercises the bounded-channel half.
    //
    // Investigated and rejected as fragile rather than shipped: opening
    // the destination with `.read(true)` only (no `.write`) looks like it
    // should force an OS-level EBADF on the next `write_all`, and it does
    // for a raw `std::fs::File` — but empirically, on `tokio` 1.53 /
    // macOS (arm64, this workstation), `tokio::fs::File::write_all`
    // reports `Ok(())` for that same read-only-opened file while writing
    // ZERO bytes; the real EBADF only surfaces later, on `sync_all()`. A
    // test built on that call would assert something true on this
    // machine today for the wrong reason, and could just as easily assert
    // the opposite on a different tokio/OS combination — exactly the
    // "fragile" case this task's instructions call out. No other
    // portable, `unsafe`-free way to force a real OS write failure was
    // found (`/dev/full` is Linux-only, not present on macOS; disk-quota
    // / tmpfs-size tricks need root or platform-specific mount tooling
    // and would not run identically in CI and on a dev machine).
    //
    // `write_line`'s failure-handling CODE itself (increment the counter,
    // log, return without propagating) is still exercised indirectly by
    // `dropped_count_is_recorded_not_silent`'s `Dropped`-line write, which
    // goes through the very same function on the success path; only the
    // `Err` branch specifically is unverified.

    #[tokio::test]
    async fn dropped_count_is_recorded_not_silent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = NetflowPaths::new(tmp.path());
        let handle = NetflowWriterHandle::spawn_with_capacity(paths.clone(), 1).unwrap();

        // Flood far past the channel capacity. The writer task cannot
        // keep up, so some sends hit the bounded-channel backstop.
        for i in 0..5_000u64 {
            handle
                .writer
                .record(flow(FlowId::from_parts(i, i as u128)))
                .await;
        }
        let dropped = handle.writer.dropped();
        tokio::time::timeout(std::time::Duration::from_secs(10), handle.shutdown())
            .await
            .expect("writer shutdown deadlocked");

        let text = std::fs::read_to_string(&paths.flows).unwrap();

        // DETERMINISTIC, not scheduler-dependent: current-thread runtime,
        // no await point in record(), capacity 1 — the first send is
        // buffered and the other 4,999 overflow.
        assert!(dropped > 0, "flooding a capacity-1 channel must drop");

        let dropped_line = text
            .lines()
            .filter_map(|l| serde_json::from_str::<LedgerLine>(l).ok())
            .find_map(|l| match l {
                LedgerLine::Dropped { count, .. } => Some(count),
                _ => None,
            })
            .expect("loss must leave a Dropped line in the ledger");
        assert_eq!(
            dropped_line, dropped,
            "the ledger must account for every lost record"
        );
    }

    /// Fix 2 (Task 10 review): the ONLY thing that ever calls
    /// `shutdown()` in production is a test. `rupu run` installs the sink
    /// into a process-wide `OnceLock` that's never cleared before process
    /// exit, and `rupu cp serve` never exits at all under normal
    /// operation — so `dropped_count_is_recorded_not_silent` above,
    /// which calls `handle.shutdown()` directly, was proving a code path
    /// no real invocation ever takes. Loss accounting that only reaches
    /// disk via a method nothing calls is loss that is, in practice,
    /// silent — invariant 2, broken.
    ///
    /// This test is shaped like that reality: it floods a
    /// capacity-1 channel past its limit exactly like the test above, but
    /// then does NOT call `shutdown()` — the handle is just held (mirroring
    /// `rupu run`'s `Arc<dyn FlowSink>` sitting in `OnceLock` forever) and
    /// the test polls the ledger FILE ON DISK until the `Dropped` line
    /// appears. A short `spawn_with_capacity_and_interval` keeps the poll
    /// fast without weakening what's proven: the SAME periodic mechanism
    /// runs at `DROPPED_VISIBILITY_INTERVAL` in production.
    #[tokio::test]
    async fn loss_becomes_visible_without_an_explicit_shutdown() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = NetflowPaths::new(tmp.path());
        let handle = NetflowWriterHandle::spawn_with_capacity_and_interval(
            paths.clone(),
            1,
            std::time::Duration::from_millis(20),
        )
        .unwrap();

        for i in 0..5_000u64 {
            handle
                .writer
                .record(flow(FlowId::from_parts(i, i as u128)))
                .await;
        }
        let dropped = handle.writer.dropped();
        assert!(dropped > 0, "flooding a capacity-1 channel must drop");

        // Deliberately NOT calling `handle.shutdown()` — that is exactly
        // the production shape this test exists to cover. `handle` (and
        // therefore the channel) stays alive for the rest of the test,
        // same as the process-wide `OnceLock` keeps it alive for the rest
        // of a real `rupu run` / `rupu cp serve` process.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Ok(text) = std::fs::read_to_string(&paths.flows) {
                let found = text
                    .lines()
                    .filter_map(|l| serde_json::from_str::<LedgerLine>(l).ok())
                    .find_map(|l| match l {
                        LedgerLine::Dropped { count, .. } => Some(count),
                        _ => None,
                    });
                if let Some(count) = found {
                    assert_eq!(
                        count, dropped,
                        "the ledger must account for every lost record"
                    );
                    return;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "loss was never surfaced to disk without an explicit shutdown() call \
                 (the exact shape every production caller uses)"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}
