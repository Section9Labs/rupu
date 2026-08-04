//! Bridges netflow records into the run transcript as `Event::NetFlow`.
//!
//! This lives in `rupu-transcript`, not `rupu-netflow`, because the
//! dependency runs transcript → netflow. Putting the bridge in the
//! netflow crate would create a cycle.

use crate::event::Event;
use crate::writer::JsonlWriter;
use async_trait::async_trait;
use rupu_netflow::{FlowId, FlowRecord, FlowSink};
use std::path::PathBuf;

/// Appends each flow to a run's transcript JSONL.
///
/// Opens, appends and drops per record — the same pattern
/// `rupu-cli`'s dispatch path uses. At flow volumes the syscall cost is
/// irrelevant and it avoids contending with the runner's own writer.
pub struct TranscriptSink {
    path: PathBuf,
}

impl TranscriptSink {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[async_trait]
impl FlowSink for TranscriptSink {
    async fn record(&self, flow: FlowRecord) {
        let event = Event::NetFlow {
            flow: Box::new(flow),
        };
        match JsonlWriter::append(&self.path) {
            Ok(mut w) => {
                if let Err(e) = w.write(&event) {
                    tracing::debug!(error = %e, "netflow transcript write failed");
                }
                let _ = w.flush();
            }
            Err(e) => {
                tracing::debug!(error = %e, path = ?self.path, "netflow transcript unavailable");
            }
        }
    }

    /// No-op by design. A transcript is an append-only narrative and
    /// cannot amend a line it already wrote; the ledger owns finalized
    /// byte counts for streamed bodies.
    async fn complete(&self, _id: FlowId, _bytes_in: u64, _duration_ms: u64) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_netflow::{Fidelity, FlowCtx, FlowId, FlowSink, Origin, Outcome};

    fn flow() -> FlowRecord {
        FlowRecord {
            id: FlowId::from_parts(1, 1),
            ts: chrono::Utc::now(),
            ctx: FlowCtx::system(Origin::Scm("github".into())),
            fidelity: Fidelity::Http,
            method: "GET".into(),
            scheme: "https".into(),
            host: "api.github.com".into(),
            port: 443,
            path: "/repos".into(),
            peer_ip: None,
            resolved_ips: vec![],
            http_version: None,
            status: Some(200),
            outcome: Outcome::Ok,
            error: None,
            bytes_out: None,
            bytes_in: Some(64),
            body_complete: true,
            ttfb_ms: Some(3),
            duration_ms: Some(9),
        }
    }

    #[tokio::test]
    async fn writes_a_netflow_event_line_to_the_transcript() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("transcript.jsonl");
        JsonlWriter::create(&path).unwrap();

        let sink = TranscriptSink::new(path.clone());
        sink.record(flow()).await;

        let text = std::fs::read_to_string(&path).unwrap();
        let last = text.lines().last().unwrap();
        let event: Event = serde_json::from_str(last).unwrap();
        match event {
            Event::NetFlow { flow } => {
                assert_eq!(flow.host, "api.github.com");
                assert_eq!(flow.status, Some(200));
            }
            other => panic!("expected NetFlow, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_unwritable_path_is_silently_tolerated() {
        // Capture must never break the caller. A bad path is a logged
        // no-op, not a panic and not an error the request path sees.
        let sink = TranscriptSink::new(std::path::PathBuf::from("/nonexistent/dir/t.jsonl"));
        sink.record(flow()).await;
        sink.complete(FlowId::from_parts(1, 1), 10, 10).await;
    }

    #[tokio::test]
    async fn complete_writes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("transcript.jsonl");
        JsonlWriter::create(&path).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        TranscriptSink::new(path.clone())
            .complete(FlowId::from_parts(2, 2), 99, 99)
            .await;

        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }
}
