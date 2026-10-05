//! Types shared between capture backends and the [`Tracker`](crate::tracker::Tracker).

use rupu_netflow::{FlowProcess, FlowRecord, FlowSink, SocketCompletion};
use std::net::SocketAddr;
use std::sync::Arc;

/// One id per `begin()`, assigned by the backend.
pub type CallId = u64;
/// The backend's attribution handle: cgroup id (Linux) / shell pid (macOS).
pub type OwnerId = u64;
/// A socket's stable identity: cookie (Linux) / srcref (macOS).
pub type SocketId = u64;

/// Transport protocol of an observed socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Tcp,
    Udp,
}

/// What a backend currently knows about one socket.
#[derive(Debug, Clone)]
pub struct SocketSnapshot {
    /// TCP or UDP.
    pub transport: Transport,
    /// Local end, when known.
    pub local: Option<SocketAddr>,
    /// Remote end; `None` for an unconnected UDP socket.
    pub remote: Option<SocketAddr>,
    /// TCP reached ESTABLISHED (or a later state) at least once.
    pub established: bool,
    /// Bytes received so far, when the backend can count them.
    pub bytes_in: Option<u64>,
    /// Bytes sent so far, when the backend can count them.
    pub bytes_out: Option<u64>,
    /// The owning process, when resolved.
    pub process: Option<FlowProcess>,
}

/// One thing the tracker wants written to a sink.
pub enum Emit {
    /// A new socket flow.
    Flow(Box<FlowRecord>),
    /// The finalization of a flow emitted earlier.
    Complete(SocketCompletion),
}

/// An [`Emit`] addressed to the sink of the call it belongs to.
pub struct Emission {
    /// The run's sink that should receive `emit`.
    pub sink: Arc<dyn FlowSink>,
    /// What to deliver.
    pub emit: Emit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_snapshot_carries_transport_and_addrs() {
        let s = SocketSnapshot {
            transport: Transport::Tcp,
            local: Some("10.0.0.2:54321".parse().unwrap()),
            remote: Some("140.82.116.3:443".parse().unwrap()),
            established: true,
            bytes_in: Some(48_000),
            bytes_out: Some(1_200),
            process: Some(rupu_netflow::FlowProcess {
                pid: 4412,
                name: "curl".into(),
            }),
        };
        assert_eq!(s.transport, Transport::Tcp);
        assert_eq!(s.remote.unwrap().port(), 443);
    }
}
