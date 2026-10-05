//! The watcher thread (macOS only): the glue between the kernel's `ntstat`
//! feed and the pure [`Tracker`].
//!
//! One plain `std::thread` owns the [`NstatSocket`]. The socket's receive
//! timeout ([`TICK`]) paces it. Each cycle it
//!
//! 1. drains datagrams for up to one tick. Each datagram holds several
//!    8-byte-aligned messages: a `SRC_ADDED` (while a call is live) is
//!    answered with a `GET_SRC_DESC`; a `SRC_DESC` is resolved to a bash
//!    call by walking the owning pid's ancestry up to a registered shell
//!    and fed to [`Tracker::observe`]; `SRC_COUNTS` refreshes a socket's
//!    byte counters; `SRC_REMOVED` feeds [`Tracker::close`];
//! 2. re-asks for the description of attributed sockets that are not yet
//!    ESTABLISHED (updates are not decoded, so state changes are polled)
//!    and, every few ticks, for the counters of every attributed socket;
//! 3. ticks the tracker and unregisters shells whose call has lingered out.
//!
//! Everything the tracker returns, and any capture-state note, is queued in
//! [`Shared::pending`] while the tracker lock is held (queue order is
//! tracker order) and written to its sink from this thread on ONE
//! long-lived current-thread runtime. No lock is held across `block_on`
//! except the dedicated delivery lock.

use std::collections::{HashMap, HashSet};
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rupu_netflow::{CaptureState, CaptureStateLine, FlowProcess, FlowSink};

use super::ntstat::{NstatSocket, HDR_LEN};
use super::parse::{parse_message, NstatMsg, NstatObservation};
use super::proctree::owning_ancestor;
use crate::tracker::Tracker;
use crate::types::{CallId, Emission, Emit, SocketSnapshot, Transport};

/// The watcher's cycle: the socket's receive timeout.
pub(super) const TICK: Duration = Duration::from_millis(50);
/// Byte counters are re-queried every this many ticks.
const COUNTS_EVERY_TICKS: u32 = 5;
/// Receive buffer: fixed, large enough for any ntstat datagram.
const RECV_BUF: usize = 1 << 17;
/// How far up the process tree a socket's owner is searched for a shell.
const MAX_ANCESTRY_HOPS: u32 = 32;
/// `TCPS_LISTEN`.
const TCPS_LISTEN: u32 = 1;
/// The backend name announced in `capture_state` lines.
pub(super) const BACKEND: &str = "macos-ntstat";

/// Lock a mutex, recovering the data from a poisoned one: capture state is
/// best-effort bookkeeping and must never take the bash call down with it.
pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// One live (or recently finished) call: its shell and where its notes go.
pub(super) struct CallEntry {
    /// The bash call's shell pid: the root of the attributed process tree.
    pub shell: u32,
    pub run_id: String,
    pub tool_call_id: String,
    pub sink: Arc<dyn FlowSink>,
    /// Set by `finished()` / `run_finished`; the shell is unregistered once
    /// the linger has elapsed.
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
    /// Registered calls; the registered shell pids are exactly their shells.
    pub calls: Mutex<HashMap<CallId, CallEntry>>,
    pub announced: Mutex<HashSet<String>>,
    pub running: AtomicBool,
    /// Receive-buffer overflows seen (`ENOBUFS`).
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
            calls: Mutex::new(HashMap::new()),
            announced: Mutex::new(HashSet::new()),
            running: AtomicBool::new(true),
            dropped: AtomicU64::new(0),
            linger,
            pending: Mutex::new(Vec::new()),
            deliver_lock: Mutex::new(()),
        }
    }

    /// Run `f` on the tracker and queue what it returns WHILE the tracker
    /// guard is still held, so queue order is tracker order: a `Complete`
    /// can never be queued ahead of its `Flow`.
    pub fn track(&self, f: impl FnOnce(&mut Tracker) -> Vec<Emission>) {
        let mut tracker = lock(&self.tracker);
        let emissions = f(&mut tracker);
        self.queue_emissions(emissions);
    }

    fn queue_emissions(&self, emissions: Vec<Emission>) {
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

    /// Deliver everything already queued on the CALLING thread. Used by
    /// `run_finished`, which must not return before a run's last flows are
    /// written (the process may exit right after). The write runs on a
    /// scoped helper thread with its own transient runtime, so it is safe
    /// from inside or outside an async context.
    pub fn flush_blocking(&self) {
        let _g = lock(&self.deliver_lock);
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

    /// Mark every call of `run_id` due for unregistering now.
    pub fn release_run(&self, run_id: &str, now: DateTime<Utc>) {
        let due = now - self.linger;
        for e in lock(&self.calls).values_mut() {
            if e.run_id == run_id && e.finished.is_none() {
                e.finished = Some(due);
            }
        }
        let mut announced = lock(&self.announced);
        announced.remove(run_id);
        announced.remove(&format!("dead:{run_id}"));
    }

    /// The pids of every registered shell.
    fn shells(&self) -> HashSet<u32> {
        lock(&self.calls).values().map(|e| e.shell).collect()
    }

    /// Unregister the shells of calls finished at least `linger` ago (the
    /// tracker's own `tick` drops the matching calls).
    fn reap_calls(&self, now: DateTime<Utc>) {
        let linger = self.linger;
        lock(&self.calls).retain(|_, e| e.finished.is_none_or(|at| now - at < linger));
    }

    /// Write one visible-loss note per live call (spec §13).
    fn note_loss(&self) {
        let total = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
        let mut notes = Vec::new();
        for e in lock(&self.calls).values_mut() {
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
                            "the kernel dropped socket events (ENOBUFS, {total} so far); \
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
        // A panicking sink must not take the delivery loop (or the watcher
        // thread) down with it: log it and carry on with the next item.
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            rt.block_on(async move {
                match item {
                    Pending::Emission(Emission { sink, emit }) => match emit {
                        Emit::Flow(f) => sink.record(*f).await,
                        Emit::Complete(c) => sink.complete_socket(c).await,
                    },
                    Pending::State(sink, line) => sink.capture_state(line).await,
                }
            });
        }));
        if outcome.is_err() {
            tracing::error!("netwatch: a flow sink panicked; dropped one item");
        }
    }
}

/// Spawn the watcher thread. `sock` already has its receive timeout set and
/// is subscribed.
pub(super) fn spawn(
    shared: Arc<Shared>,
    sock: NstatSocket,
    rt: tokio::runtime::Runtime,
) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("rupu-netwatch".to_string())
        .spawn(move || run(shared, sock, rt))
}

/// A socket attributed to a registered shell.
struct Tracked {
    shell: u32,
    snap: SocketSnapshot,
}

/// Watcher-local state.
#[derive(Default)]
struct Local {
    socks: HashMap<u64, Tracked>,
    ticks: u32,
}

fn run(shared: Arc<Shared>, sock: NstatSocket, rt: tokio::runtime::Runtime) {
    let mut buf = vec![0u8; RECV_BUF];
    let mut local = Local::default();

    while shared.running.load(Ordering::Acquire) {
        let started = Instant::now();
        // One bad iteration (a parse or tracker panic) must not kill the
        // watcher: log it and run the next cycle.
        let cycle = catch_unwind(AssertUnwindSafe(|| {
            drain(&shared, &sock, &mut buf, &mut local);
            housekeeping(&shared, &sock, &mut local);
            shared.flush(&rt);
        }));
        if cycle.is_err() {
            tracing::error!("netwatch: the watcher cycle panicked; continuing");
        }
        // Pacing normally comes from the receive timeout; a persistent
        // immediate error must not turn this into a hot loop.
        if let Some(rest) = TICK.checked_sub(started.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    shared.flush(&rt);
}

/// Split one datagram into its messages. Each message starts at an 8-byte
/// aligned offset and carries its own length; a malformed length ends the
/// walk.
fn messages(buf: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut off = 0usize;
    std::iter::from_fn(move || {
        if off + HDR_LEN > buf.len() {
            return None;
        }
        let len = u16::from_ne_bytes([buf[off + 12], buf[off + 13]]) as usize;
        if len < HDR_LEN || off + len > buf.len() {
            return None;
        }
        let msg = &buf[off..off + len];
        off += (len + 7) & !7;
        Some(msg)
    })
}

/// Receive datagrams for up to one tick and act on every message.
fn drain(shared: &Shared, sock: &NstatSocket, buf: &mut [u8], local: &mut Local) {
    let deadline = Instant::now() + TICK;
    loop {
        match sock.recv_into(buf) {
            Ok(0) => break,
            Ok(n) => {
                for msg in messages(&buf[..n]) {
                    handle(shared, sock, local, parse_message(msg));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.raw_os_error() == Some(nix::errno::Errno::ENOBUFS as i32) => {
                // The kernel overflowed our receive buffer: events were
                // lost. Say so, and keep going.
                shared.note_loss();
            }
            // WouldBlock (the tick timeout) or a real error: nothing more
            // this cycle.
            Err(_) => break,
        }
        if Instant::now() >= deadline {
            break;
        }
    }
}

fn handle(shared: &Shared, sock: &NstatSocket, local: &mut Local, msg: NstatMsg) {
    match msg {
        NstatMsg::SrcAdded { srcref, .. } => {
            // Only worth describing while some call is live; at subscribe
            // time (and whenever no bash call runs) the flood is ignored.
            if !lock(&shared.calls).is_empty() {
                request(sock.request_description(srcref), "description");
            }
        }
        NstatMsg::SrcDesc(obs) => on_desc(shared, local, obs),
        NstatMsg::SrcCounts {
            srcref,
            bytes_in,
            bytes_out,
        } => {
            if let Some(t) = local.socks.get_mut(&srcref) {
                t.snap.bytes_in = Some(bytes_in);
                t.snap.bytes_out = Some(bytes_out);
                if t.snap.remote.is_some() {
                    let (shell, snap) = (t.shell, t.snap.clone());
                    shared.track(|tr| tr.observe(srcref, shell as u64, snap, Utc::now()));
                }
            }
        }
        NstatMsg::SrcRemoved { srcref } => {
            if let Some(t) = local.socks.remove(&srcref) {
                shared.track(|tr| tr.close(srcref, t.snap, Utc::now()));
            }
        }
        NstatMsg::Other => {}
    }
}

/// A request to the kernel failed: the socket may be gone or the process
/// exited; not worth more than a debug line.
fn request(r: io::Result<()>, what: &str) {
    if let Err(e) = r {
        tracing::debug!(error = %e, "netwatch: ntstat {what} request failed");
    }
}

/// Attribute a described socket to a call and feed the tracker.
fn on_desc(shared: &Shared, local: &mut Local, obs: NstatObservation) {
    // A known socket keeps its attribution (its pid does not change).
    let shell = match local.socks.get(&obs.srcref) {
        Some(t) => t.shell,
        None => {
            let shells = shared.shells();
            if shells.is_empty() {
                return;
            }
            match owning_ancestor(obs.pid, MAX_ANCESTRY_HOPS, |p| shells.contains(&p)) {
                Some(s) => s,
                // Unattributed: not a descendant of any live bash call.
                None => return,
            }
        }
    };
    // A listener is not a connection.
    if obs.transport == Transport::Tcp && obs.state == TCPS_LISTEN {
        return;
    }
    let (bytes_in, bytes_out) = local
        .socks
        .get(&obs.srcref)
        .map_or((obs.bytes_in, obs.bytes_out), |t| {
            (t.snap.bytes_in, t.snap.bytes_out)
        });
    let snap = SocketSnapshot {
        transport: obs.transport,
        local: obs.local,
        remote: obs.remote,
        established: obs.established,
        bytes_in,
        bytes_out,
        process: Some(FlowProcess {
            pid: obs.pid,
            name: obs.pname,
        }),
    };
    let connected = snap.remote.is_some();
    local.socks.insert(
        obs.srcref,
        Tracked {
            shell,
            snap: snap.clone(),
        },
    );
    // A fresh TCP socket is unconnected until `connect`, and an unconnected
    // UDP socket has no destination to report: both stay local (and are
    // polled for a remote) until they have one.
    if connected {
        shared.track(|t| t.observe(obs.srcref, shell as u64, snap, Utc::now()));
    }
}

/// Per-tick polling, expiry and cleanup.
fn housekeeping(shared: &Shared, sock: &NstatSocket, local: &mut Local) {
    local.ticks = local.ticks.wrapping_add(1);
    let slow = local.ticks.is_multiple_of(COUNTS_EVERY_TICKS);
    for (srcref, t) in &local.socks {
        // State changes are not pushed to us: poll a TCP socket every tick
        // until ESTABLISHED (a short connection may be gone within a few
        // ticks), and a not-yet-connected UDP socket at the slower pace.
        if !t.snap.established && (t.snap.transport == Transport::Tcp || slow) {
            request(sock.request_description(*srcref), "description");
        }
        if slow {
            request(sock.query_counts(*srcref), "counts");
        }
    }
    let now = Utc::now();
    shared.track(|t| t.tick(now));
    shared.reap_calls(now);
    // Forget sockets whose call has been dropped (the tracker already
    // finalized them); a later removal then finds nothing to close.
    let shells = shared.shells();
    local.socks.retain(|_, t| shells.contains(&t.shell));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(len: u16, fill: u8) -> Vec<u8> {
        let padded = (len as usize + 7) & !7;
        let mut m = vec![fill; padded];
        m[12..14].copy_from_slice(&len.to_ne_bytes());
        m
    }

    #[test]
    fn messages_split_on_aligned_lengths() {
        let mut d = msg(20, 1); // pads to 24
        d.extend(msg(16, 2));
        d.extend(msg(24, 3));
        let got: Vec<&[u8]> = messages(&d).collect();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].len(), 20);
        assert_eq!(got[1].len(), 16);
        assert_eq!(got[2].len(), 24);
        assert_eq!(got[1][0], 2);
        assert_eq!(got[2][0], 3);
    }

    #[test]
    fn messages_stop_on_a_bad_length() {
        let mut d = msg(16, 1);
        let mut bad = msg(16, 9);
        bad[12..14].copy_from_slice(&200u16.to_ne_bytes());
        d.extend(bad);
        assert_eq!(messages(&d).count(), 1);
        // Too short for a header.
        assert_eq!(messages(&[0u8; 8]).count(), 0);
        assert_eq!(messages(&[]).count(), 0);
    }
}
