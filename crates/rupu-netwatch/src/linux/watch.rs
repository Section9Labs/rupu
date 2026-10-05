//! The watcher thread (Linux only): the glue between the kernel's
//! `sock_diag` view and the pure [`Tracker`].
//!
//! One plain `std::thread` owns both [`DiagSocket`]s. Each cycle it
//!
//! 1. dumps the TCP and UDP socket tables (only while a call is live) and
//!    feeds every socket owned by a live call's cgroup to
//!    [`Tracker::observe`];
//! 2. drains the socket-destroy multicast socket, feeding each close to
//!    [`Tracker::close`] (the kernel's destroy messages carry no owner, so a
//!    close is only meaningful for a cookie a dump already attributed);
//! 3. ticks the tracker and reaps the cgroups of finished calls.
//!
//! The thread never spins: the destroy socket's receive timeout is the
//! cycle's pacing. Everything the tracker returns, and any capture-state
//! note, is queued in [`Shared::pending`] and written to its sink from this
//! thread on ONE long-lived current-thread runtime (sinks only enqueue, so
//! `block_on` is cheap).

use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rupu_netflow::{CaptureState, CaptureStateLine, FlowSink};

use super::cgroup::CallCgroup;
use super::netlink::DiagSocket;
use super::parse::{nlmsgs, parse_inet_diag, InetDiagObservation};
use super::req::dump_request;
use crate::tracker::Tracker;
use crate::types::{CallId, Emission, Emit, SocketSnapshot, Transport};

/// `NLMSG_NOOP`.
const NLMSG_NOOP: u16 = 1;
/// `NLMSG_ERROR`.
const NLMSG_ERROR: u16 = 2;
/// `NLMSG_DONE`.
const NLMSG_DONE: u16 = 3;

/// `AF_INET` / `AF_INET6`.
const AF_INET: u8 = 2;
const AF_INET6: u8 = 10;
/// `IPPROTO_TCP` / `IPPROTO_UDP`.
const IPPROTO_TCP: u8 = 6;
const IPPROTO_UDP: u8 = 17;

/// What each cycle dumps.
const DUMPS: [(u8, u8, Transport); 4] = [
    (AF_INET, IPPROTO_TCP, Transport::Tcp),
    (AF_INET6, IPPROTO_TCP, Transport::Tcp),
    (AF_INET, IPPROTO_UDP, Transport::Udp),
    (AF_INET6, IPPROTO_UDP, Transport::Udp),
];

/// Receive buffer for one netlink datagram.
const RECV_BUF: usize = 1 << 17;
/// `SO_RCVBUF` for the destroy socket, so a burst of closes is not lost.
pub(super) const DESTROY_RCVBUF: usize = 4 << 20;
/// How many `linger`s a finished call's cgroup is retried before giving up
/// on removing it (something is still running inside).
const CGROUP_GIVE_UP_LINGERS: i32 = 10;
/// The backend name announced in `capture_state` lines.
pub(super) const BACKEND: &str = "linux-cgroup-sock_diag";

/// Lock a mutex, recovering the data from a poisoned one: capture state is
/// best-effort bookkeeping and must never take the bash call down with it.
pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// One live (or recently finished) call's cgroup and where its notes go.
pub(super) struct CgEntry {
    pub cg: CallCgroup,
    pub run_id: String,
    pub tool_call_id: String,
    pub sink: Arc<dyn FlowSink>,
    /// Set by `finished()` / `run_finished`; the cgroup is removed once the
    /// linger has elapsed.
    pub finished: Option<DateTime<Utc>>,
    /// A loss note has already been written for this call.
    pub loss_noted: bool,
}

/// Something queued for delivery to a sink.
pub(super) enum Pending {
    Emission(Emission),
    State(Arc<dyn FlowSink>, CaptureStateLine),
}

/// State shared by the capture handle, its calls and the watcher thread.
pub(super) struct Shared {
    pub tracker: Mutex<Tracker>,
    pub cgroups: Mutex<HashMap<CallId, CgEntry>>,
    pub announced: Mutex<HashSet<String>>,
    pub running: AtomicBool,
    /// Destroy-socket overflows seen (`ENOBUFS`).
    pub dropped: AtomicU64,
    linger: chrono::Duration,
    pending: Mutex<Vec<Pending>>,
    /// Held across take-and-deliver so the watcher and `run_finished`
    /// never reorder a `Flow` behind its `Complete`.
    deliver_lock: Mutex<()>,
}

impl Shared {
    pub fn new(linger: chrono::Duration) -> Self {
        Self {
            tracker: Mutex::new(Tracker::new(linger)),
            cgroups: Mutex::new(HashMap::new()),
            announced: Mutex::new(HashSet::new()),
            running: AtomicBool::new(true),
            dropped: AtomicU64::new(0),
            linger,
            pending: Mutex::new(Vec::new()),
            deliver_lock: Mutex::new(()),
        }
    }

    pub fn queue_emissions(&self, emissions: Vec<Emission>) {
        if emissions.is_empty() {
            return;
        }
        lock(&self.pending).extend(emissions.into_iter().map(Pending::Emission));
    }

    pub fn queue_state(&self, sink: Arc<dyn FlowSink>, line: CaptureStateLine) {
        lock(&self.pending).push(Pending::State(sink, line));
    }

    fn take_pending(&self) -> Vec<Pending> {
        std::mem::take(&mut *lock(&self.pending))
    }

    /// Deliver everything queued, on the watcher's runtime.
    fn flush(&self, rt: &tokio::runtime::Runtime) {
        let _g = lock(&self.deliver_lock);
        deliver(rt, self.take_pending());
    }

    /// Queue `emissions` behind whatever is already queued and deliver the
    /// lot now, on the CALLING thread. Used by `run_finished`, which must not
    /// return before a run's last flows are written (the process may exit
    /// right after). The write runs on a scoped helper thread with its own
    /// transient runtime, so it is safe from inside or outside an async
    /// context.
    pub fn flush_blocking(&self, emissions: Vec<Emission>) {
        let _g = lock(&self.deliver_lock);
        self.queue_emissions(emissions);
        let items = self.take_pending();
        if items.is_empty() {
            return;
        }
        std::thread::scope(|s| {
            let worker = s.spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => deliver(&rt, items),
                    Err(e) => tracing::debug!(
                        error = %e,
                        "netwatch: no runtime to deliver a run's final flows"
                    ),
                }
            });
            let _ = worker.join();
        });
    }

    /// Mark every cgroup of `run_id` due for removal now.
    pub fn release_run(&self, run_id: &str, now: DateTime<Utc>) {
        let due = now - self.linger;
        for e in lock(&self.cgroups).values_mut() {
            if e.run_id == run_id && e.finished.is_none() {
                e.finished = Some(due);
            }
        }
        lock(&self.announced).remove(run_id);
    }

    /// Remove the cgroups of calls finished at least `linger` ago. A cgroup
    /// that still holds a process is retried every cycle until it empties or
    /// the call is [`CGROUP_GIVE_UP_LINGERS`] lingers old.
    fn reap_cgroups(&self, now: DateTime<Utc>) {
        let due: Vec<(CallId, CallCgroup, DateTime<Utc>)> = lock(&self.cgroups)
            .iter()
            .filter_map(|(id, e)| {
                let at = e.finished?;
                (now - at >= self.linger).then(|| (*id, e.cg.clone(), at))
            })
            .collect();
        if due.is_empty() {
            return;
        }
        let mut done = Vec::new();
        for (id, cg, at) in due {
            cg.remove_if_empty();
            let gone = !cg.procs_path.exists();
            let stale = now - at >= self.linger * CGROUP_GIVE_UP_LINGERS;
            if gone || stale {
                done.push(id);
            }
        }
        let mut map = lock(&self.cgroups);
        for id in done {
            map.remove(&id);
        }
    }

    /// Remove every call cgroup that is empty now, finished or not. Called
    /// once the watcher has stopped.
    pub fn reap_all(&self) {
        let all: Vec<CallCgroup> = lock(&self.cgroups).values().map(|e| e.cg.clone()).collect();
        for cg in all {
            cg.remove_if_empty();
        }
        lock(&self.cgroups).clear();
    }

    /// Write one visible-loss note per live call (spec §13).
    fn note_loss(&self) {
        let total = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
        let mut notes = Vec::new();
        for e in lock(&self.cgroups).values_mut() {
            if e.finished.is_none() && !e.loss_noted {
                e.loss_noted = true;
                notes.push((
                    e.sink.clone(),
                    CaptureStateLine {
                        state: CaptureState::Active {
                            backend: BACKEND.to_string(),
                        },
                        tool_call_id: Some(e.tool_call_id.clone()),
                        note: Some(format!(
                            "the kernel dropped socket-close events (ENOBUFS, {total} so far); \
                             some short-lived connections may be missing"
                        )),
                    },
                ));
            }
        }
        for (sink, line) in notes {
            self.queue_state(sink, line);
        }
    }
}

/// Write queued items to their sinks.
fn deliver(rt: &tokio::runtime::Runtime, items: Vec<Pending>) {
    for item in items {
        rt.block_on(async move {
            match item {
                Pending::Emission(Emission { sink, emit }) => match emit {
                    Emit::Flow(f) => sink.record(*f).await,
                    Emit::Complete(c) => sink.complete_socket(c).await,
                },
                Pending::State(sink, line) => sink.capture_state(line).await,
            }
        });
    }
}

/// Spawn the watcher thread. `dump` and `destroy` already have their receive
/// timeout (the poll interval) set; `destroy` is bound to the destroy groups.
pub(super) fn spawn(
    shared: Arc<Shared>,
    dump: DiagSocket,
    destroy: DiagSocket,
    rt: tokio::runtime::Runtime,
    poll: Duration,
) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("rupu-netwatch".to_string())
        .spawn(move || run(shared, dump, destroy, rt, poll))
}

fn run(
    shared: Arc<Shared>,
    dump: DiagSocket,
    destroy: DiagSocket,
    rt: tokio::runtime::Runtime,
    poll: Duration,
) {
    let mut buf = vec![0u8; RECV_BUF];
    // Cookie -> transport, for sockets attributed to a live call. A destroy
    // message carries neither owner nor reliably its protocol.
    let mut known: HashMap<u64, Transport> = HashMap::new();
    let mut seq: u32 = 0;
    let mut stale_dump = false;

    while shared.running.load(Ordering::Acquire) {
        dump_cycle(
            &shared,
            &dump,
            &mut buf,
            &mut known,
            &mut seq,
            &mut stale_dump,
        );
        shared.flush(&rt);
        drain_destroy(&shared, &destroy, &mut buf, &mut known, poll);
        let tick = lock(&shared.tracker).tick(Utc::now());
        shared.queue_emissions(tick);
        shared.reap_cgroups(Utc::now());
        shared.flush(&rt);
    }
    shared.flush(&rt);
}

/// The socket as the tracker sees it. The process is not resolved here.
fn snapshot_of(obs: &InetDiagObservation) -> SocketSnapshot {
    SocketSnapshot {
        transport: obs.transport,
        local: obs.local,
        remote: obs.remote,
        established: obs.established,
        bytes_in: obs.bytes_in,
        bytes_out: obs.bytes_out,
        process: None,
    }
}

/// Dump the socket tables and observe every socket owned by a live call.
fn dump_cycle(
    shared: &Shared,
    sock: &DiagSocket,
    buf: &mut [u8],
    known: &mut HashMap<u64, Transport>,
    seq: &mut u32,
    stale: &mut bool,
) {
    let owners: HashSet<u64> = lock(&shared.cgroups).values().map(|e| e.cg.id).collect();
    if owners.is_empty() {
        // Nothing to attribute to: skip the (whole-system) dump.
        known.clear();
        return;
    }

    if *stale {
        // A previous dump was abandoned mid-reply; discard its leftovers.
        loop {
            match sock.recv_into(buf) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {}
                Err(_) => break,
            }
        }
        *stale = false;
    }

    for (family, proto, transport) in DUMPS {
        *seq = seq.wrapping_add(1);
        if let Err(e) = sock.send(&dump_request(family, proto, *seq)) {
            tracing::debug!(error = %e, family, proto, "netwatch: dump request failed");
            continue;
        }
        let mut finished = false;
        'recv: loop {
            let n = match sock.recv_into(buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                    tracing::debug!("netwatch: sock_diag dump datagram truncated");
                    continue;
                }
                // WouldBlock / TimedOut (the poll timeout) or a real error.
                Err(_) => break,
            };
            for (ty, body) in nlmsgs(&buf[..n]) {
                match ty {
                    NLMSG_DONE | NLMSG_ERROR => {
                        finished = true;
                        break 'recv;
                    }
                    NLMSG_NOOP => {}
                    _ => {
                        let Some(obs) = parse_inet_diag(body, transport) else {
                            continue;
                        };
                        let Some(cgroup) = obs.cgroup_id else {
                            continue;
                        };
                        if !owners.contains(&cgroup) {
                            continue;
                        }
                        known.insert(obs.socket_id, obs.transport);
                        let emissions = lock(&shared.tracker).observe(
                            obs.socket_id,
                            cgroup,
                            snapshot_of(&obs),
                            Utc::now(),
                        );
                        shared.queue_emissions(emissions);
                    }
                }
            }
        }
        if !finished {
            *stale = true;
        }
    }
}

/// Drain the destroy multicast socket for up to one poll interval.
fn drain_destroy(
    shared: &Shared,
    sock: &DiagSocket,
    buf: &mut [u8],
    known: &mut HashMap<u64, Transport>,
    poll: Duration,
) {
    let deadline = Instant::now() + poll;
    loop {
        match sock.recv_into(buf) {
            Ok(0) => break,
            Ok(n) => {
                for (ty, body) in nlmsgs(&buf[..n]) {
                    if matches!(ty, NLMSG_NOOP | NLMSG_ERROR | NLMSG_DONE) {
                        continue;
                    }
                    handle_close(shared, body, known);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                tracing::debug!("netwatch: sock_diag destroy datagram truncated");
            }
            Err(e) if e.raw_os_error() == Some(rustix::io::Errno::NOBUFS.raw_os_error()) => {
                // The kernel overflowed our receive buffer: closes were
                // lost. Say so, and keep going.
                shared.note_loss();
            }
            // WouldBlock / TimedOut: nothing more this cycle.
            Err(_) => break,
        }
        if Instant::now() >= deadline {
            break;
        }
    }
}

/// Feed one destroy message to the tracker, when it is a socket of ours.
fn handle_close(shared: &Shared, body: &[u8], known: &mut HashMap<u64, Transport>) {
    let Some(probe) = parse_inet_diag(body, Transport::Tcp) else {
        return;
    };
    let Some(transport) = known.remove(&probe.socket_id) else {
        // Never attributed to a live call: not ours.
        return;
    };
    let obs = if transport == Transport::Tcp {
        probe
    } else {
        match parse_inet_diag(body, transport) {
            Some(o) => o,
            None => return,
        }
    };
    let emissions = lock(&shared.tracker).close(obs.socket_id, snapshot_of(&obs), Utc::now());
    shared.queue_emissions(emissions);
}
