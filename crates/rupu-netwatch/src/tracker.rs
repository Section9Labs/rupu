//! The pure socket-attribution state machine.
//!
//! The [`Tracker`] is synchronous and does no IO: backends feed it socket
//! observations and it *returns* the [`Emission`]s to deliver. Every method
//! takes `now` so tests are deterministic. A socket is attributed to a
//! registered bash call through its opaque [`OwnerId`]; a socket whose
//! owner maps to no call is ignored entirely.

use crate::types::{CallId, Emission, Emit, OwnerId, SocketId, SocketSnapshot, Transport};
use chrono::{DateTime, Utc};
use rupu_netflow::{
    CallAttribution, Fidelity, FlowCtx, FlowId, FlowRecord, Origin, Outcome, SocketCompletion,
};
use std::collections::HashMap;

/// What a backend hands the tracker when a bash call begins.
pub struct CallInfo {
    /// The backend's attribution handle for this call.
    pub owner: OwnerId,
    /// Where the call's flows go and what they are attributed to.
    pub attribution: CallAttribution,
}

struct CallState {
    owner: OwnerId,
    attribution: CallAttribution,
    run_id: String,
    /// Set by `finish_call`; the call is dropped once the linger elapses.
    closing_at: Option<DateTime<Utc>>,
}

struct SockState {
    call_id: CallId,
    id: FlowId,
    flow_emitted: bool,
    ever_established: bool,
    first_seen: DateTime<Utc>,
    last: SocketSnapshot,
}

/// Attributes observed sockets to bash calls and decides what to emit.
pub struct Tracker {
    /// How long a finished call stays attributable.
    linger: chrono::Duration,
    calls: HashMap<CallId, CallState>,
    owner_to_call: HashMap<OwnerId, CallId>,
    sockets: HashMap<SocketId, SockState>,
}

impl Tracker {
    /// A tracker whose finished calls stay attributable for `linger`.
    pub fn new(linger: chrono::Duration) -> Self {
        Self {
            linger,
            calls: HashMap::new(),
            owner_to_call: HashMap::new(),
            sockets: HashMap::new(),
        }
    }

    /// Begin attributing sockets owned by `info.owner` to call `id`.
    pub fn register_call(&mut self, id: CallId, info: CallInfo, _now: DateTime<Utc>) {
        let run_id = info.attribution.run_id.clone();
        self.owner_to_call.insert(info.owner, id);
        self.calls.insert(
            id,
            CallState {
                owner: info.owner,
                attribution: info.attribution,
                run_id,
                closing_at: None,
            },
        );
    }

    /// Report the current state of a socket. Emits the socket's `Flow` the
    /// first time it is both attributable and reportable (TCP established,
    /// or any UDP); later observations emit nothing.
    pub fn observe(
        &mut self,
        sock: SocketId,
        owner: OwnerId,
        snap: SocketSnapshot,
        now: DateTime<Utc>,
    ) -> Vec<Emission> {
        let call_id = match self.sockets.get(&sock) {
            Some(s) => s.call_id,
            None => match self.owner_to_call.get(&owner) {
                Some(c) => *c,
                None => return Vec::new(),
            },
        };
        let Some(call) = self.calls.get(&call_id) else {
            return Vec::new();
        };

        let state = self.sockets.entry(sock).or_insert_with(|| SockState {
            call_id,
            id: FlowId::new(),
            flow_emitted: false,
            ever_established: false,
            first_seen: now,
            last: snap.clone(),
        });
        state.ever_established |= snap.established;
        state.last = snap;

        let reportable = match state.last.transport {
            Transport::Tcp => state.ever_established,
            Transport::Udp => true,
        };
        if state.flow_emitted || !reportable {
            return Vec::new();
        }
        state.flow_emitted = true;
        let record = flow_record_from(&call.attribution, state);
        vec![Emission {
            sink: call.attribution.sink.clone(),
            emit: Emit::Flow(Box::new(record)),
        }]
    }

    /// Report that a socket closed, with its final snapshot.
    ///
    /// A socket that already emitted a `Flow` is finalized with a
    /// `Complete`. A TCP socket that never reached ESTABLISHED never emitted
    /// one, so it is reported now as a single `TransportError` flow — the
    /// attempt happened and failed, and a `Complete` with no `Flow` to
    /// finalize would be dropped.
    pub fn close(
        &mut self,
        sock: SocketId,
        snap: SocketSnapshot,
        now: DateTime<Utc>,
    ) -> Vec<Emission> {
        let Some(mut state) = self.sockets.remove(&sock) else {
            return Vec::new();
        };
        let Some(call) = self.calls.get(&state.call_id) else {
            return Vec::new();
        };
        state.ever_established |= snap.established;
        state.last = snap;
        let sink = call.attribution.sink.clone();

        if state.flow_emitted {
            let (outcome, error) = settle_outcome(
                state.last.transport,
                state.ever_established,
                state.last.bytes_in,
            );
            return vec![Emission {
                sink,
                emit: Emit::Complete(completion(&state, now, outcome, error)),
            }];
        }

        // Attributed but never reportable: the never-established case.
        let mut record = flow_record_from(&call.attribution, &state);
        record.outcome = Outcome::TransportError;
        record.error = Some(NEVER_ESTABLISHED.to_string());
        record.body_complete = true;
        record.bytes_in = state.last.bytes_in;
        record.bytes_out = state.last.bytes_out;
        record.duration_ms = Some(elapsed_ms(state.first_seen, now));
        vec![Emission {
            sink,
            emit: Emit::Flow(Box::new(record)),
        }]
    }
}

const NEVER_ESTABLISHED: &str = "connection never established";

fn elapsed_ms(from: DateTime<Utc>, to: DateTime<Utc>) -> u64 {
    (to - from).num_milliseconds().max(0) as u64
}

/// The final outcome of a socket that emitted a flow, and the error note
/// that goes with a failure.
fn settle_outcome(
    transport: Transport,
    ever_established: bool,
    bytes_in: Option<u64>,
) -> (Outcome, Option<String>) {
    if ever_established || transport == Transport::Udp || bytes_in.is_some_and(|b| b > 0) {
        (Outcome::Ok, None)
    } else {
        (Outcome::TransportError, Some(NEVER_ESTABLISHED.to_string()))
    }
}

/// The `Complete` for a socket, from its latest snapshot.
fn completion(
    sock: &SockState,
    now: DateTime<Utc>,
    outcome: Outcome,
    error: Option<String>,
) -> SocketCompletion {
    SocketCompletion {
        id: sock.id,
        duration_ms: elapsed_ms(sock.first_seen, now),
        bytes_in: sock.last.bytes_in,
        bytes_out: sock.last.bytes_out,
        outcome: Some(outcome),
        error,
    }
}

/// The `Flow` record for a socket, from its first-seen state and latest
/// snapshot.
fn flow_record_from(call: &CallAttribution, sock: &SockState) -> FlowRecord {
    let snap = &sock.last;
    let name = snap
        .process
        .as_ref()
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "unknown".to_string());
    FlowRecord {
        id: sock.id,
        ts: sock.first_seen,
        ctx: FlowCtx {
            run_id: Some(call.run_id.clone()),
            step_id: call.step_id.clone(),
            agent: call.agent.clone(),
            workspace_id: None,
            tool_call_id: Some(call.tool_call_id.clone()),
            origin: Origin::Subprocess(name),
        },
        fidelity: Fidelity::Socket,
        method: String::new(),
        scheme: match snap.transport {
            Transport::Tcp => "tcp".to_string(),
            Transport::Udp => "udp".to_string(),
        },
        host: snap.remote.map(|r| r.ip().to_string()).unwrap_or_default(),
        port: snap.remote.map(|r| r.port()).unwrap_or(0),
        path: String::new(),
        peer_ip: snap.remote.map(|r| r.ip()),
        resolved_ips: Vec::new(),
        process: snap.process.clone(),
        local_addr: snap.local.map(|l| l.to_string()),
        direction: None,
        http_version: None,
        status: None,
        outcome: Outcome::Ok,
        error: None,
        bytes_out: None,
        bytes_in: None,
        body_complete: false,
        ttfb_ms: None,
        duration_ms: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rupu_netflow::{CallAttribution, Fidelity, FlowProcess, FlowRecord, MemorySink, Origin};
    use std::sync::Arc;

    fn t0() -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0).unwrap()
    }

    fn attribution(sink: Arc<MemorySink>) -> CallAttribution {
        CallAttribution {
            run_id: "run-1".into(),
            step_id: Some("step-1".into()),
            agent: Some("reviewer".into()),
            codename: None,
            tool_call_id: "toolu_1".into(),
            sink,
        }
    }

    /// A tracker with call 1 registered for `owner`, plus its sink.
    fn tracker_with_call(owner: OwnerId) -> (Tracker, Arc<MemorySink>) {
        let sink = Arc::new(MemorySink::default());
        let mut t = Tracker::new(chrono::Duration::seconds(3));
        t.register_call(
            1,
            CallInfo {
                owner,
                attribution: attribution(sink.clone()),
            },
            t0(),
        );
        (t, sink)
    }

    fn tcp(established: bool) -> SocketSnapshot {
        SocketSnapshot {
            transport: Transport::Tcp,
            local: Some("10.0.0.2:54321".parse().unwrap()),
            remote: Some("140.82.116.3:443".parse().unwrap()),
            established,
            bytes_in: None,
            bytes_out: None,
            process: Some(FlowProcess {
                pid: 4412,
                name: "curl".into(),
            }),
        }
    }

    fn only_flow(mut v: Vec<Emission>) -> FlowRecord {
        assert_eq!(v.len(), 1, "expected exactly one emission");
        match v.remove(0).emit {
            Emit::Flow(f) => *f,
            Emit::Complete(_) => panic!("expected a Flow"),
        }
    }

    #[test]
    fn observe_before_established_emits_nothing_then_flow_on_established() {
        let (mut t, _sink) = tracker_with_call(100);
        assert!(t.observe(7, 100, tcp(false), t0()).is_empty());

        let f = only_flow(t.observe(7, 100, tcp(true), t0()));
        assert_eq!(f.fidelity, Fidelity::Socket);
        assert_eq!(f.ctx.origin, Origin::Subprocess("curl".into()));
        assert_eq!(f.ctx.tool_call_id.as_deref(), Some("toolu_1"));
        assert_eq!(f.ctx.run_id.as_deref(), Some("run-1"));
        assert_eq!(f.host, "140.82.116.3");
        assert_eq!(f.port, 443);
        assert_eq!(f.scheme, "tcp");
        assert!(!f.body_complete);
        assert_eq!(f.process.as_ref().unwrap().pid, 4412);

        assert!(t.observe(7, 100, tcp(true), t0()).is_empty());
    }

    #[test]
    fn udp_emits_flow_on_first_observation() {
        let (mut t, _sink) = tracker_with_call(100);
        let mut snap = tcp(false);
        snap.transport = Transport::Udp;
        let f = only_flow(t.observe(8, 100, snap.clone(), t0()));
        assert_eq!(f.scheme, "udp");
        assert_eq!(f.port, 443);

        snap.remote = None;
        let f = only_flow(t.observe(9, 100, snap, t0()));
        assert_eq!(f.host, "");
        assert_eq!(f.port, 0);
        assert_eq!(f.peer_ip, None);
    }

    #[test]
    fn observation_for_unregistered_owner_is_ignored() {
        let (mut t, _sink) = tracker_with_call(100);
        assert!(t.observe(7, 999, tcp(true), t0()).is_empty());
        assert!(t.close(7, tcp(true), t0()).is_empty());
    }

    fn only_complete(mut v: Vec<Emission>) -> rupu_netflow::SocketCompletion {
        assert_eq!(v.len(), 1, "expected exactly one emission");
        match v.remove(0).emit {
            Emit::Complete(c) => c,
            Emit::Flow(_) => panic!("expected a Complete"),
        }
    }

    fn ms(n: i64) -> chrono::Duration {
        chrono::Duration::milliseconds(n)
    }

    #[test]
    fn established_socket_closes_with_complete_ok() {
        let (mut t, _sink) = tracker_with_call(100);
        let flow = only_flow(t.observe(7, 100, tcp(true), t0()));

        let mut fin = tcp(true);
        fin.bytes_in = Some(50_000);
        fin.bytes_out = Some(1_300);
        let c = only_complete(t.close(7, fin, t0() + ms(1_500)));
        assert_eq!(c.id, flow.id);
        assert_eq!(c.bytes_in, Some(50_000));
        assert_eq!(c.bytes_out, Some(1_300));
        assert_eq!(c.outcome, Some(Outcome::Ok));
        assert_eq!(c.error, None);
        assert_eq!(c.duration_ms, 1_500);
    }

    #[test]
    fn tcp_never_established_closes_with_single_transport_error_flow() {
        let (mut t, _sink) = tracker_with_call(100);
        assert!(t.observe(7, 100, tcp(false), t0()).is_empty());

        let f = only_flow(t.close(7, tcp(false), t0() + ms(200)));
        assert_eq!(f.outcome, Outcome::TransportError);
        assert_eq!(f.error.as_deref(), Some("connection never established"));
        assert!(f.body_complete);
        assert_eq!(f.duration_ms, Some(200));
        assert_eq!(f.fidelity, Fidelity::Socket);
    }

    #[test]
    fn close_of_unknown_socket_is_noop() {
        let (mut t, _sink) = tracker_with_call(100);
        assert!(t.close(42, tcp(true), t0()).is_empty());
    }

    #[test]
    fn udp_closes_with_complete_ok() {
        let (mut t, _sink) = tracker_with_call(100);
        let mut snap = tcp(false);
        snap.transport = Transport::Udp;
        let flow = only_flow(t.observe(8, 100, snap.clone(), t0()));
        let c = only_complete(t.close(8, snap, t0() + ms(50)));
        assert_eq!(c.id, flow.id);
        assert_eq!(c.outcome, Some(Outcome::Ok));
        assert_eq!(c.error, None);
    }
}
