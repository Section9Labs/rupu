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

#[derive(Debug)]
enum WriteRequest {
    Line(Box<LedgerLine>),
    Flush(tokio::sync::oneshot::Sender<()>),
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
        paths.ensure_dir()?;
        let (tx, rx) = mpsc::channel(capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        let writer = Arc::new(NetflowWriter {
            tx,
            dropped: dropped.clone(),
        });
        let task = tokio::spawn(run_writer(paths, rx, dropped));
        Ok(Self { writer, task })
    }

    /// Flush pending writes, emit a final `Dropped` line if anything was
    /// lost, then stop the task.
    pub async fn shutdown(self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.writer.tx.send(WriteRequest::Flush(tx)).await;
        let _ = rx.await;
        drop(self.writer);
        let _ = self.task.await;
    }
}

async fn run_writer(
    paths: NetflowPaths,
    mut rx: mpsc::Receiver<WriteRequest>,
    dropped_counter: Arc<AtomicU64>,
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

    while let Some(req) = rx.recv().await {
        match req {
            WriteRequest::Line(line) => {
                write_line(&mut file, &line).await;
            }
            WriteRequest::Flush(ack) => {
                let dropped = dropped_counter.load(Ordering::Relaxed);
                if dropped > last_dropped {
                    let line = LedgerLine::Dropped {
                        count: dropped - last_dropped,
                        ts: chrono::Utc::now(),
                    };
                    write_line(&mut file, &line).await;
                    last_dropped = dropped;
                }
                let _ = file.flush().await;
                let _ = ack.send(());
            }
        }
    }

    let dropped = dropped_counter.load(Ordering::Relaxed);
    if dropped > last_dropped {
        let line = LedgerLine::Dropped {
            count: dropped - last_dropped,
            ts: chrono::Utc::now(),
        };
        write_line(&mut file, &line).await;
    }
    let _ = file.flush().await;
}

async fn write_line(file: &mut tokio::fs::File, line: &LedgerLine) {
    let Ok(mut json) = serde_json::to_string(line) else {
        return;
    };
    json.push('\n');
    if let Err(e) = file.write_all(json.as_bytes()).await {
        tracing::debug!(error = %e, "netflow ledger write failed");
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
        let recorded = text.lines().count() as u64;
        // Whatever was lost is accounted for, never silently vanished.
        if dropped > 0 {
            assert!(text.contains("\"type\":\"dropped\""));
        }
        assert!(recorded > 0);
    }
}
