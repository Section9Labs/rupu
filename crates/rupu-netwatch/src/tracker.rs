//! The pure socket-attribution state machine.
//!
//! The [`Tracker`] is synchronous and does no IO: backends feed it socket
//! observations and it *returns* the [`Emission`]s to deliver. Every method
//! takes `now` so tests are deterministic. A socket is attributed to a
//! registered bash call through its opaque [`OwnerId`]; a socket whose
//! owner maps to no call is ignored entirely.

use crate::types::{CallId, Emission, Emit, OwnerId, SocketId, SocketSnapshot, Transport};
use chrono::{DateTime, Utc};
use rupu_netflow::{CallAttribution, Fidelity, FlowCtx, FlowId, FlowRecord, Origin, Outcome};
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

    /// Report that a socket closed. Filled in by the close task.
    pub fn close(
        &mut self,
        sock: SocketId,
        _snap: SocketSnapshot,
        _now: DateTime<Utc>,
    ) -> Vec<Emission> {
        self.sockets.remove(&sock);
        Vec::new()
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
}
