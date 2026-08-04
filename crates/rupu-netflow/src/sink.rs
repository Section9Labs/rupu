//! The `FlowSink` port and its in-process adapters.

use crate::record::{FlowId, FlowRecord};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// Where flow records go.
///
/// Implementations MUST be non-panicking and best-effort: a sink failure
/// must never surface into the request path that produced the record.
#[async_trait]
pub trait FlowSink: Send + Sync {
    /// Record a flow. Called at response-header time.
    async fn record(&self, flow: FlowRecord);

    /// Finalize a streamed body recorded earlier. Sinks that cannot
    /// express completion may ignore this.
    async fn complete(&self, id: FlowId, bytes_in: u64, duration_ms: u64);
}

/// Capture disabled.
pub struct NullSink;

#[async_trait]
impl FlowSink for NullSink {
    async fn record(&self, _flow: FlowRecord) {}
    async fn complete(&self, _id: FlowId, _bytes_in: u64, _duration_ms: u64) {}
}

/// Delivers to every child in order.
pub struct FanoutSink {
    children: Vec<Arc<dyn FlowSink>>,
}

impl FanoutSink {
    pub fn new(children: Vec<Arc<dyn FlowSink>>) -> Self {
        Self { children }
    }
}

#[async_trait]
impl FlowSink for FanoutSink {
    async fn record(&self, flow: FlowRecord) {
        for child in &self.children {
            let child = child.clone();
            let flow = flow.clone();
            if tokio::task::spawn(async move { child.record(flow).await })
                .await
                .is_err()
            {
                tracing::debug!(
                    "netflow sink child panicked during record; continuing to remaining sinks"
                );
            }
        }
    }

    async fn complete(&self, id: FlowId, bytes_in: u64, duration_ms: u64) {
        for child in &self.children {
            let child = child.clone();
            if tokio::task::spawn(async move { child.complete(id, bytes_in, duration_ms).await })
                .await
                .is_err()
            {
                tracing::debug!(
                    "netflow sink child panicked during complete; continuing to remaining sinks"
                );
            }
        }
    }
}

/// Test double.
#[derive(Default)]
pub struct MemorySink {
    inner: Mutex<MemoryState>,
}

#[derive(Default)]
struct MemoryState {
    records: Vec<FlowRecord>,
    completions: Vec<(FlowId, u64, u64)>,
}

impl MemorySink {
    pub fn records(&self) -> Vec<FlowRecord> {
        self.inner
            .lock()
            .map(|s| s.records.clone())
            .unwrap_or_default()
    }

    pub fn completions(&self) -> Vec<(FlowId, u64, u64)> {
        self.inner
            .lock()
            .map(|s| s.completions.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl FlowSink for MemorySink {
    async fn record(&self, flow: FlowRecord) {
        if let Ok(mut s) = self.inner.lock() {
            s.records.push(flow);
        }
    }

    async fn complete(&self, id: FlowId, bytes_in: u64, duration_ms: u64) {
        if let Ok(mut s) = self.inner.lock() {
            s.completions.push((id, bytes_in, duration_ms));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::{FlowCtx, Origin};
    use crate::record::{Fidelity, FlowId, FlowRecord, Outcome};
    use std::sync::Arc;

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
            path: "/".into(),
            peer_ip: None,
            resolved_ips: vec![],
            http_version: None,
            status: Some(200),
            outcome: Outcome::Ok,
            error: None,
            bytes_out: None,
            bytes_in: None,
            body_complete: true,
            ttfb_ms: None,
            duration_ms: None,
        }
    }

    #[tokio::test]
    async fn memory_sink_collects_records_and_completions() {
        let sink = MemorySink::default();
        let id = FlowId::from_parts(1, 1);
        sink.record(flow(id)).await;
        sink.complete(id, 512, 30).await;

        assert_eq!(sink.records().len(), 1);
        assert_eq!(sink.completions(), vec![(id, 512, 30)]);
    }

    #[tokio::test]
    async fn fanout_delivers_to_every_child() {
        let a = Arc::new(MemorySink::default());
        let b = Arc::new(MemorySink::default());
        let fan = FanoutSink::new(vec![a.clone(), b.clone()]);

        fan.record(flow(FlowId::from_parts(2, 2))).await;

        assert_eq!(a.records().len(), 1);
        assert_eq!(b.records().len(), 1);
    }

    #[tokio::test]
    async fn null_sink_is_inert() {
        let sink = NullSink;
        sink.record(flow(FlowId::from_parts(3, 3))).await;
        sink.complete(FlowId::from_parts(3, 3), 1, 1).await;
        // No panic, no state. Nothing to assert beyond reaching here.
    }

    /// A sink that panics on every record call, used to test isolation.
    struct PanicSink;

    #[async_trait]
    impl FlowSink for PanicSink {
        async fn record(&self, _flow: FlowRecord) {
            panic!("intentional panic in PanicSink");
        }

        async fn complete(&self, _id: FlowId, _bytes_in: u64, _duration_ms: u64) {
            panic!("intentional panic in PanicSink");
        }
    }

    #[tokio::test]
    async fn fanout_isolates_panicking_child() {
        // Suppress panic output to keep test output clean.
        let old_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));

        let panicker = Arc::new(PanicSink);
        let healthy = Arc::new(MemorySink::default());
        let fan = FanoutSink::new(vec![panicker, healthy.clone()]);

        let id = FlowId::from_parts(4, 4);
        fan.record(flow(id)).await;

        // Restore the panic hook.
        std::panic::set_hook(old_hook);

        // Assert the healthy sink still received the record despite the panic.
        assert_eq!(healthy.records().len(), 1);
        assert_eq!(healthy.records()[0].id, id);
    }
}
