//! The remote sources of `wait`. A run on a registered host is read from the
//! host's event stream (`stream_run_events_after`, resumed from the cursor):
//! its events feed `run_events`, its `end` frame fires `run_settled`. Its
//! gates are polled through `get_run`, and a remote message space through
//! `list_messages` from the id of the last message seen (a string cursor).
//!
//! Requests go out in rounds, per host: each host's first round at the
//! call's start, then one every [`REMOTE_POLL`] — [`SSH_POLL`] for a host
//! whose every request is a connection of its own (SSH) — or at once when an
//! `end` frame needs its run's status. A host has at most one round in
//! flight, and within it its requests go one after another; hosts' rounds are
//! independent owned futures the wait loop polls beside its other sources,
//! each applied as it lands, so a slow or hung host delays neither the local
//! sources nor another host. A stream is its host's long-lived connection,
//! not a poll; one that ends without its `end` frame is reopened from its
//! cursor at that host's next round, silently when the reopen succeeds. On a
//! host the [`Throttle`] spaces, a gate or message ask held back by another
//! call's recent ask doesn't hold up its round: the round goes on without it,
//! and the host's round at the ask's turn asks it.
//!
//! A failure of a source's first answer fails the call; a later failure is
//! retried at its next round and reported in the result's `warnings` (a later
//! poll the call's timeout cut short is one only when it had at least
//! [`FIRST_ROUND_BUDGET`] to answer: a healthy host's poll that went out just
//! before the timeout is no news; a cut-short status always is). A first
//! answer may come up to [`FIRST_ROUND_BUDGET`] past a shorter timeout (a
//! `timeout_ms: 0` peek, which then reads what the streams it opened already
//! have). Once something fired, a source still without its first answer
//! holds the result back by at most that much from the call's start; past
//! it, that source is named in `warnings` (a held-back one with its turn).

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use futures_util::stream::FuturesUnordered;
use futures_util::{Stream, StreamExt as _};
use rupu_fleet::{LogLine, MessageKind};
use rupu_runtime::services::operator::{MessagesView, ViewQuery};
use serde_json::{json, Value};
use tokio::time::Instant;

use super::parse::{Condition, Kind};
use super::throttle::{Claim, Throttle};
use super::{Got, CATCH_UP_MAX};
use crate::host::connector::{EventByteStream, HostConnector, HostConnectorError};
use crate::host::messages::MessageRoute;
use crate::sse::{parse_run_frames, RunFrame};
use crate::state::AppState;

/// How often a remote run's gates and a remote message space are polled, and
/// an ended stream is reopened, on a host that holds a connection (HTTP) or
/// answers from the coordinator's own mirror (tunnel, bucket).
pub(super) const REMOTE_POLL: Duration = Duration::from_secs(2);
/// The same on a host whose every request opens a connection of its own
/// ([`HostConnector::connects_per_request`]: SSH, where `get_run` and
/// `list_messages` are each a fresh `ssh` process). A gate and a space polled
/// every 2 s would open about one connection a second for as long as an
/// agent keeps waiting — the kind of burst that gets the coordinator blocked
/// by the host. (Its run's gates can't come from the coordinator's mirror
/// instead: the SSH mirror holds only runs this coordinator launched, and
/// takes their `run.json` only once it is terminal.) The spacing holds across
/// calls too ([`Throttle`]).
pub(super) const SSH_POLL: Duration = Duration::from_secs(10);
/// The most messages one poll of a space reads; the cursor then stops at the
/// last of them, so the next poll reads on.
const MESSAGES_PAGE: usize = 200;
/// Once something fired, the frames still arriving are collected until the
/// streams stay quiet this long.
const DRAIN_QUIET: Duration = Duration::from_millis(100);
/// The least the first round may take to answer, whatever the call's timeout:
/// so `timeout_ms: 0` still peeks at a remote gate or space, and still finds
/// an unknown run.
pub(super) const FIRST_ROUND_BUDGET: Duration = Duration::from_secs(5);
/// The `after` of a stream only watched for its `end`: every event is behind
/// it, so the host sends none.
const NO_EVENTS: u64 = u64::MAX;

type Frames = Pin<Box<dyn Stream<Item = io::Result<RunFrame>> + Send>>;

struct Host {
    id: String,
    conn: Arc<dyn HostConnector>,
    /// A round of its is in flight.
    busy: bool,
    /// How often its poll rounds go out ([`poll_every`]).
    poll: Duration,
    /// Every request is a connection of its own: its gate and message asks
    /// go through the [`Throttle`], spaced across calls.
    spaced: bool,
    /// When its next poll round is due.
    next_poll: Instant,
}

/// How often `conn`'s poll rounds go out.
fn poll_every(conn: &dyn HostConnector) -> Duration {
    if conn.connects_per_request() {
        SSH_POLL
    } else {
        REMOTE_POLL
    }
}

/// A remote run's event stream, read for the `run_events` and `run_settled`
/// conditions on it.
struct RunSrc {
    host: usize,
    run: String,
    source: String,
    /// A `run_events` condition reads it, so it keeps a cursor.
    collects: bool,
    stream: Option<Frames>,
    /// Its `end` frame arrived: the run is over, and it isn't reopened.
    ended: bool,
    /// Its `end` arrived with `run_settled` conditions unfired: the next
    /// round asks for the run's status, which fires them.
    needs_status: bool,
    /// Its first answer arrived (a failure or a warning included).
    answered: bool,
}

/// A remote run whose gates are polled.
struct GateSrc {
    host: usize,
    run: String,
    source: String,
    first: First,
}

/// A remote message space, polled.
struct MsgSrc {
    host: usize,
    root: String,
    source: String,
    /// The kinds its conditions want; every kind when one wants all.
    kinds: Option<Vec<MessageKind>>,
    first: First,
}

/// Where a polled source stands on its first answer.
#[derive(Default)]
struct First {
    answered: bool,
    /// Its first ask was held back by the [`Throttle`]: asked at `.0` by
    /// another call, its turn at `.1`, within the first round's bound.
    held: Option<(Instant, Instant)>,
}

/// The remote side of one call.
pub(super) struct Remote {
    hosts: Vec<Host>,
    runs: Vec<RunSrc>,
    gates: Vec<GateSrc>,
    msgs: Vec<MsgSrc>,
    /// Where the next look for a frame starts, so no stream starves another.
    turn: usize,
    /// The hosts' rounds in flight, at most one per host.
    inflight: FuturesUnordered<HostRound>,
    /// Shared by every call on the CP.
    throttle: Arc<Throttle>,
    /// What bounds a first answer: the call's deadline, or
    /// [`FIRST_ROUND_BUDGET`] from its start when that is later.
    first_by: Instant,
}

/// One host's round in flight: the host, then what it brought.
type HostRound = Pin<Box<dyn Future<Output = (usize, Round)> + Send>>;

/// What a host's round brought.
pub(super) struct Round {
    answers: Vec<Answer>,
    /// For a host the [`Throttle`] spaces: when its gate and message asks may
    /// next go out (the earliest), which is when its next poll round is due.
    next_due: Option<Instant>,
}

/// What the remote side has for the wait loop.
pub(super) enum RemoteWake {
    /// A stream yielded (its source's index), or ended (`None`).
    Frame(usize, Option<io::Result<RunFrame>>),
    /// A host's round landed.
    Round(usize, Round),
}

enum Ask {
    Open {
        src: usize,
        run: String,
        after: u64,
    },
    Gate {
        src: usize,
        run: String,
    },
    Msgs {
        src: usize,
        root: String,
        q: ViewQuery,
    },
    Status {
        src: usize,
        run: String,
    },
}

/// Which requests a round makes.
#[derive(Clone, Copy, PartialEq)]
enum RoundKind {
    /// Reopen streams, poll gates and spaces, and ask the statuses owed.
    All,
    /// Only the statuses owed.
    Status,
}

/// Why a request gave no answer.
pub(super) enum Fail {
    Conn(HostConnectorError),
    /// The call's timeout came first, `had` after the request went out.
    Late {
        had: Duration,
    },
    /// Held back by the [`Throttle`]: asked at `asked` by another call, its
    /// turn at `due`. The round doesn't wait for it, so it holds up nothing
    /// else of its host; the host's next round asks at `due`.
    Deferred {
        asked: Instant,
        due: Instant,
    },
}

pub(super) enum Answer {
    Open(usize, Result<EventByteStream, Fail>),
    Gate(usize, Result<Value, Fail>),
    Msgs(usize, Result<MessagesView, Fail>),
    Status(usize, Result<Value, Fail>),
}

impl Remote {
    /// The remote sources of `watch`, each host resolved. An error names a
    /// host that isn't registered.
    pub(super) fn new(
        state: &AppState,
        watch: &[Condition],
        throttle: Arc<Throttle>,
    ) -> Result<Self, String> {
        let mut r = Remote {
            hosts: Vec::new(),
            runs: Vec::new(),
            gates: Vec::new(),
            msgs: Vec::new(),
            turn: 0,
            inflight: FuturesUnordered::new(),
            throttle,
            first_by: Instant::now() + FIRST_ROUND_BUDGET,
        };
        for c in watch {
            let Some(host_id) = &c.host else { continue };
            let host = match r.hosts.iter().position(|h| h.id == *host_id) {
                Some(h) => h,
                None => {
                    let conn = crate::api::runs::resolve_host(state, host_id).map_err(|e| {
                        if e.0 == axum::http::StatusCode::NOT_FOUND {
                            format!("unknown host `{host_id}`")
                        } else {
                            format!("host `{host_id}`: {}", e.1)
                        }
                    })?;
                    r.hosts.push(Host {
                        id: host_id.clone(),
                        poll: poll_every(conn.as_ref()),
                        spaced: conn.connects_per_request(),
                        conn,
                        busy: false,
                        next_poll: Instant::now(),
                    });
                    r.hosts.len() - 1
                }
            };
            match &c.kind {
                Kind::Settled | Kind::Events { .. } => {
                    let collects = matches!(c.kind, Kind::Events { .. });
                    match r.runs.iter_mut().find(|s| s.source == c.source) {
                        Some(s) => s.collects |= collects,
                        None => r.runs.push(RunSrc {
                            host,
                            run: c.target.clone(),
                            source: c.source.clone(),
                            collects,
                            stream: None,
                            ended: false,
                            needs_status: false,
                            answered: false,
                        }),
                    }
                }
                Kind::Gate => {
                    if !r.gates.iter().any(|s| s.source == c.source) {
                        r.gates.push(GateSrc {
                            host,
                            run: c.target.clone(),
                            source: c.source.clone(),
                            first: First::default(),
                        });
                    }
                }
                Kind::Messages { kinds } => {
                    match r.msgs.iter_mut().find(|s| s.source == c.source) {
                        Some(s) => {
                            s.kinds = match (s.kinds.take(), kinds) {
                                (Some(mut have), Some(more)) => {
                                    for k in more {
                                        if !have.contains(k) {
                                            have.push(*k);
                                        }
                                    }
                                    Some(have)
                                }
                                _ => None,
                            }
                        }
                        None => r.msgs.push(MsgSrc {
                            host,
                            root: c.target.clone(),
                            source: c.source.clone(),
                            kinds: kinds.clone(),
                            first: First::default(),
                        }),
                    }
                }
                // The other kinds take no `host`.
                _ => {}
            }
        }
        Ok(r)
    }

    /// Every host's first round, each bounded by `first_by`.
    pub(super) fn start(&mut self, watch: &[Condition], got: &Got, first_by: Instant) {
        self.first_by = first_by;
        for h in 0..self.hosts.len() {
            self.hosts[h].next_poll = Instant::now() + self.hosts[h].poll;
            self.launch(h, watch, got, first_by, RoundKind::All);
        }
    }

    /// Start each idle host's round that is due: its poll round every
    /// [`poll_every`], else the statuses its `end` frames owe. A host whose
    /// source still waits for its first answer is asked until `first_by`, the
    /// others until `deadline`.
    pub(super) fn schedule(&mut self, watch: &[Condition], got: &Got, deadline: Instant) {
        self.start_due(watch, got, deadline, false);
    }

    /// [`Self::schedule`], for every host or, with `first_only`, only for the
    /// hosts with a source still waiting for its first answer.
    fn start_due(&mut self, watch: &[Condition], got: &Got, deadline: Instant, first_only: bool) {
        let now = Instant::now();
        for h in 0..self.hosts.len() {
            if first_only && !self.host_first_pending(h) {
                continue;
            }
            // A hung host can keep the loop running to its first-round cut,
            // past the deadline: a round started then would only be cut off
            // and warn, unless it brings a first answer.
            let until = self.until(h, deadline);
            if self.hosts[h].busy || now >= until {
                continue;
            }
            if now >= self.hosts[h].next_poll {
                self.hosts[h].next_poll = now + self.hosts[h].poll;
                self.launch(h, watch, got, until, RoundKind::All);
            } else if self.runs.iter().any(|s| s.host == h && s.needs_status) {
                self.launch(h, watch, got, until, RoundKind::Status);
            }
        }
    }

    /// Until when host `h` is asked: `first_by` while a source of its waits
    /// for its first answer, else `deadline`.
    fn until(&self, h: usize, deadline: Instant) -> Instant {
        if self.host_first_pending(h) {
            self.first_by.max(deadline)
        } else {
            deadline
        }
    }

    /// When the next idle host's poll round is due; none for a host past what
    /// it is asked until, since `schedule` starts no round for it then and a
    /// due time already behind us would wake the loop again at once.
    pub(super) fn next_poll_at(&self, deadline: Instant) -> Option<Instant> {
        let now = Instant::now();
        (0..self.hosts.len())
            .filter(|&h| !self.hosts[h].busy && now < self.until(h, deadline))
            .map(|h| self.hosts[h].next_poll)
            .min()
    }

    /// A remote source still waits for its first answer.
    pub(super) fn first_pending(&self) -> bool {
        (0..self.hosts.len()).any(|h| self.host_first_pending(h))
    }

    /// A source of host `h` still waits for its first answer.
    fn host_first_pending(&self, h: usize) -> bool {
        self.runs.iter().any(|s| s.host == h && !s.answered)
            || self.gates.iter().any(|s| s.host == h && !s.first.answered)
            || self.msgs.iter().any(|s| s.host == h && !s.first.answered)
    }

    /// Host `h`'s round of `kind` in flight, its requests bounded by
    /// `deadline`; `false` when it has nothing to ask.
    fn launch(
        &mut self,
        h: usize,
        watch: &[Condition],
        got: &Got,
        deadline: Instant,
        kind: RoundKind,
    ) -> bool {
        let asks = self.asks_for(h, watch, got, kind);
        if asks.is_empty() {
            return false;
        }
        // A held-back ask going out now is no longer held: if it doesn't
        // answer in time, it is named as any unanswered ask is.
        for ask in &asks {
            match ask {
                Ask::Gate { src, .. } => self.gates[*src].first.held = None,
                Ask::Msgs { src, .. } => self.msgs[*src].first.held = None,
                Ask::Open { .. } | Ask::Status { .. } => {}
            }
        }
        let memo = self.hosts[h]
            .spaced
            .then(|| (Arc::clone(&self.throttle), self.hosts[h].id.clone()));
        let conn = Arc::clone(&self.hosts[h].conn);
        self.hosts[h].busy = true;
        self.inflight.push(Box::pin(async move {
            (h, ask_host(conn, asks, deadline, memo).await)
        }));
        true
    }

    /// Host `h`'s requests for a round of `kind`.
    fn asks_for(&self, h: usize, watch: &[Condition], got: &Got, kind: RoundKind) -> Vec<Ask> {
        let mut asks = Vec::new();
        for (src, s) in self.runs.iter().enumerate().filter(|(_, s)| s.host == h) {
            if s.needs_status {
                asks.push(Ask::Status {
                    src,
                    run: s.run.clone(),
                });
            } else if kind == RoundKind::All && s.stream.is_none() && !s.ended {
                let after = if s.collects {
                    got.cursor.get(&s.source).copied().unwrap_or(0)
                } else {
                    NO_EVENTS
                };
                asks.push(Ask::Open {
                    src,
                    run: s.run.clone(),
                    after,
                });
            }
        }
        if kind == RoundKind::All {
            for (src, s) in self.gates.iter().enumerate().filter(|(_, s)| s.host == h) {
                let pending = watch.iter().enumerate().any(|(i, c)| {
                    c.source == s.source
                        && matches!(c.kind, Kind::Gate)
                        && !got.fired.contains_key(&i)
                });
                if pending {
                    asks.push(Ask::Gate {
                        src,
                        run: s.run.clone(),
                    });
                }
            }
            for (src, s) in self.msgs.iter().enumerate().filter(|(_, s)| s.host == h) {
                asks.push(Ask::Msgs {
                    src,
                    root: s.root.clone(),
                    q: ViewQuery {
                        after: got.ids.get(&s.source).cloned(),
                        limit: MESSAGES_PAGE,
                        kinds: s.kinds.clone(),
                    },
                });
            }
        }
        asks
    }

    /// Take host `h`'s round: a failure of a source's first answer is the
    /// call's error; later, a warning.
    pub(super) fn landed(
        &mut self,
        h: usize,
        round: Round,
        watch: &[Condition],
        got: &mut Got,
    ) -> Result<(), String> {
        let host = &mut self.hosts[h];
        host.busy = false;
        // Its next poll goes out when the spacing allows, counted from the
        // asks themselves.
        if let Some(due) = round.next_due {
            host.next_poll = due;
        }
        self.apply(round.answers, watch, got)
    }

    /// Take a round's answers. A failure of a source's first answer is the
    /// call's error; later, a warning.
    fn apply(
        &mut self,
        answers: Vec<Answer>,
        watch: &[Condition],
        got: &mut Got,
    ) -> Result<(), String> {
        let first_by = self.first_by;
        for answer in answers {
            match answer {
                Answer::Open(i, r) => {
                    let asked = Asked::of(std::mem::replace(&mut self.runs[i].answered, true));
                    match r {
                        Ok(bytes) => self.runs[i].stream = Some(Box::pin(parse_run_frames(bytes))),
                        Err(f) => {
                            let s = &self.runs[i];
                            let host = &self.hosts[s.host].id;
                            failed(got, asked, &s.source, f, |e| {
                                describe(host, "stream run events", "run", &s.run, e)
                            })?
                        }
                    }
                }
                Answer::Gate(i, r) => {
                    let src = &mut self.gates[i];
                    let Some(asked) = src.first.take(&r, first_by, got, &src.source) else {
                        continue;
                    };
                    let s = &self.gates[i];
                    match r {
                        Ok(v) => {
                            let gates = gates_of(&v);
                            if !gates.is_empty() {
                                for (c, cond) in watch.iter().enumerate() {
                                    if cond.source == s.source && matches!(cond.kind, Kind::Gate) {
                                        got.fired
                                            .entry(c)
                                            .or_insert_with(|| json!({ "gates": gates }));
                                    }
                                }
                            }
                        }
                        Err(f) => {
                            let host = &self.hosts[s.host].id;
                            failed(got, asked, &s.source, f, |e| {
                                describe(host, "read runs", "run", &s.run, e)
                            })?
                        }
                    }
                }
                Answer::Msgs(i, r) => {
                    let src = &mut self.msgs[i];
                    let Some(asked) = src.first.take(&r, first_by, got, &src.source) else {
                        continue;
                    };
                    let s = &self.msgs[i];
                    match r {
                        Ok(view) => take_messages(watch, &s.source, view, got),
                        Err(f) => {
                            let host = &self.hosts[s.host].id;
                            let noun = MessageRoute::of_id(&s.root).map_or("run", |r| r.noun());
                            failed(got, asked, &s.source, f, |e| {
                                describe(host, "read messages", noun, &s.root, e)
                            })?
                        }
                    }
                }
                Answer::Status(i, r) => {
                    let s = &mut self.runs[i];
                    s.needs_status = false;
                    // The `end` frame says the run is over: its conditions
                    // fire whether or not the status could be read.
                    let status = match r {
                        Ok(v) => v.get("run").unwrap_or(&v).get("status").cloned(),
                        Err(f) => {
                            let host = &self.hosts[s.host].id;
                            let _ = failed(got, Asked::Status, &s.source, f, |e| {
                                describe(host, "read runs", "run", &s.run, e)
                            });
                            None
                        }
                    };
                    for (c, cond) in watch.iter().enumerate() {
                        if cond.source == s.source && matches!(cond.kind, Kind::Settled) {
                            got.fired.entry(c).or_insert_with(
                                || json!({ "status": status.clone().unwrap_or(Value::Null) }),
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// The first answers still out, once something else fired: taken as they
    /// land (a failure the call's error) until `cut`, a held-back ask going
    /// out at its turn when that comes first. A source without its first
    /// answer by then is named in `warnings`.
    pub(super) async fn settle_first(
        &mut self,
        cut: Instant,
        deadline: Instant,
        watch: &[Condition],
        got: &mut Got,
    ) -> Result<(), String> {
        while self.first_pending() {
            // Nothing out, and no held-back ask's turn before the cut: no
            // answer can come.
            let turn = self.held_turn();
            if self.inflight.is_empty() && turn.is_none_or(|t| t > cut) {
                break;
            }
            let wake = turn.map_or(cut, |t| t.min(cut));
            tokio::select! {
                w = self.next() => match w {
                    RemoteWake::Round(h, round) => self.landed(h, round, watch, got)?,
                    RemoteWake::Frame(i, item) => self.on_frame(i, item, watch, got),
                },
                _ = tokio::time::sleep_until(wake) => {
                    if Instant::now() >= cut {
                        break;
                    }
                    // Only the rounds that can bring a first answer: one for
                    // another host would land past the loop, unread (a
                    // status it carries lost to this call).
                    self.start_due(watch, got, deadline, true);
                    // A turn that started no round would only wake this
                    // again at once.
                    if self.inflight.is_empty() {
                        break;
                    }
                }
            }
        }
        let why = if cut >= deadline || Instant::now() >= deadline {
            "no answer before the call's timeout".to_string()
        } else {
            format!("no first answer within {} s", FIRST_ROUND_BUDGET.as_secs())
        };
        let runs = self
            .runs
            .iter()
            .filter(|s| !s.answered)
            .map(|s| (&s.source, why.clone()));
        let polled = self
            .gates
            .iter()
            .map(|s| (&s.source, &s.first))
            .chain(self.msgs.iter().map(|s| (&s.source, &s.first)))
            .filter(|(_, f)| !f.answered)
            .map(|(source, f)| {
                (
                    source,
                    f.held
                        .map_or_else(|| why.clone(), |(asked, due)| held_back(asked, due)),
                )
            });
        for (source, msg) in runs.chain(polled) {
            got.warnings.entry(source.clone()).or_insert(msg);
        }
        Ok(())
    }

    /// The earliest turn of a held-back first ask whose host is idle.
    fn held_turn(&self) -> Option<Instant> {
        self.gates
            .iter()
            .map(|s| (s.host, &s.first))
            .chain(self.msgs.iter().map(|s| (s.host, &s.first)))
            .filter(|(h, f)| !f.answered && !self.hosts[*h].busy)
            .filter_map(|(_, f)| f.held.map(|(_, due)| due))
            .min()
    }

    /// The next frame of any open stream, or the next host round to land.
    /// Pending while there is neither.
    pub(super) fn next(&mut self) -> impl Future<Output = RemoteWake> + Send + '_ {
        std::future::poll_fn(move |cx| {
            if let Poll::Ready((i, item)) = self.poll_frame(cx) {
                return Poll::Ready(RemoteWake::Frame(i, item));
            }
            match self.inflight.poll_next_unpin(cx) {
                Poll::Ready(Some((h, round))) => Poll::Ready(RemoteWake::Round(h, round)),
                // Empty: a round pushed later is polled on the next call.
                Poll::Ready(None) | Poll::Pending => Poll::Pending,
            }
        })
    }

    /// The next frame of any open stream, with the index of its source;
    /// `None` when that stream ended. Pending while no stream is open.
    fn next_frame(
        &mut self,
    ) -> impl Future<Output = (usize, Option<io::Result<RunFrame>>)> + Send + '_ {
        std::future::poll_fn(move |cx| self.poll_frame(cx))
    }

    fn poll_frame(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<(usize, Option<io::Result<RunFrame>>)> {
        let n = self.runs.len();
        for k in 0..n {
            let i = (self.turn + k) % n;
            if let Some(s) = self.runs[i].stream.as_mut() {
                if let Poll::Ready(item) = s.as_mut().poll_next(cx) {
                    self.turn = (i + 1) % n;
                    return Poll::Ready((i, item));
                }
            }
        }
        Poll::Pending
    }

    /// Take what source `i`'s stream yielded: an event its `run_events`
    /// conditions want; its `end`, which owes its `run_settled` conditions a
    /// status (its host's next round asks); or its end without one, broken or not,
    /// which the next round reopens (a warning only if that fails).
    pub(super) fn on_frame(
        &mut self,
        i: usize,
        item: Option<io::Result<RunFrame>>,
        watch: &[Condition],
        got: &mut Got,
    ) {
        let s = &mut self.runs[i];
        match item {
            Some(Ok(RunFrame::Event { id, json })) => {
                if !s.collects {
                    return;
                }
                // A host that ignored the cursor replays from the start.
                if id <= got.cursor.get(&s.source).copied().unwrap_or(0) {
                    return;
                }
                got.cursor.insert(s.source.clone(), id);
                let Ok(event) = serde_json::from_str::<Value>(&json) else {
                    return;
                };
                let ty = event.get("type").and_then(Value::as_str);
                let mut taken = false;
                for (c, cond) in watch.iter().enumerate() {
                    let Kind::Events { types } = &cond.kind else {
                        continue;
                    };
                    if cond.source == s.source
                        && types
                            .as_ref()
                            .is_none_or(|t| ty.is_some_and(|ty| t.iter().any(|x| x == ty)))
                    {
                        *got.counts.entry(c).or_default() += 1;
                        taken = true;
                    }
                }
                if taken {
                    got.events
                        .push(json!({"source": s.source, "id": id, "event": event}));
                }
            }
            Some(Ok(RunFrame::End)) => {
                s.stream = None;
                s.ended = true;
                s.needs_status = watch.iter().enumerate().any(|(c, cond)| {
                    cond.source == s.source
                        && matches!(cond.kind, Kind::Settled)
                        && !got.fired.contains_key(&c)
                });
            }
            Some(Err(e)) => {
                tracing::debug!(source = %s.source, error = %e, "wait: remote event stream broke; reopening");
                s.stream = None;
            }
            None => s.stream = None,
        }
    }

    /// Once something fired: the frames that still arrive before the streams
    /// go quiet for [`DRAIN_QUIET`] (or the call's timeout), then the statuses
    /// an `end` among them owes.
    pub(super) async fn drain(&mut self, watch: &[Condition], got: &mut Got, deadline: Instant) {
        for _ in 0..CATCH_UP_MAX {
            if self.runs.iter().all(|s| s.stream.is_none()) {
                break;
            }
            let quiet = (Instant::now() + DRAIN_QUIET).min(deadline);
            match tokio::time::timeout_at(quiet, self.next_frame()).await {
                Ok((i, item)) => self.on_frame(i, item, watch, got),
                Err(_) => break,
            }
        }
        // The statuses its `end` frames owe, from each idle host.
        let rounds: Vec<_> = (0..self.hosts.len())
            .filter(|&h| !self.hosts[h].busy)
            .map(|h| (h, self.asks_for(h, watch, got, RoundKind::Status)))
            .filter(|(_, asks)| !asks.is_empty())
            .map(|(h, asks)| ask_host(Arc::clone(&self.hosts[h].conn), asks, deadline, None))
            .collect();
        for round in futures_util::future::join_all(rounds).await {
            // Only statuses, never an error.
            let _ = self.apply(round.answers, watch, got);
        }
    }
}

/// `asks` against one host, one after another, each bounded by `deadline`.
/// With `memo` (a host the [`Throttle`] spaces), a gate or message ask goes
/// out only as the spacing allows: it takes another call's recent answer
/// when that answer stands in, else is [`Fail::Deferred`] to its turn (the
/// round doesn't wait for it), and its answer is recorded for the next.
async fn ask_host(
    conn: Arc<dyn HostConnector>,
    asks: Vec<Ask>,
    deadline: Instant,
    memo: Option<(Arc<Throttle>, String)>,
) -> Round {
    let mut answers = Vec::with_capacity(asks.len());
    let mut next_due: Option<Instant> = None;
    let mut note = |due: Instant| next_due = Some(next_due.map_or(due, |n| n.min(due)));
    for ask in asks {
        answers.push(match ask {
            Ask::Open { src, run, after } => Answer::Open(
                src,
                bounded(deadline, conn.stream_run_events_after(&run, after)).await,
            ),
            Ask::Gate { src, run } => Answer::Gate(
                src,
                match &memo {
                    None => bounded(deadline, conn.get_run(&run)).await,
                    Some((t, host)) => match t.claim_gate(host, &run) {
                        Claim::Ask { asked } => {
                            note(asked + t.spacing());
                            let r = bounded(deadline, conn.get_run(&run)).await;
                            if let Ok(v) = &r {
                                t.gate_answered(host, &run, asked, v);
                            }
                            r
                        }
                        Claim::Reuse { answer, due } => {
                            note(due);
                            Ok(answer)
                        }
                        Claim::Wait { asked, due } => {
                            note(due);
                            Err(Fail::Deferred { asked, due })
                        }
                    },
                },
            ),
            Ask::Msgs { src, root, q } => Answer::Msgs(
                src,
                match &memo {
                    None => bounded(deadline, conn.list_messages(&root, q)).await,
                    Some((t, host)) => match t.claim_msgs(host, &root, &q) {
                        Claim::Ask { asked } => {
                            note(asked + t.spacing());
                            let r = bounded(deadline, conn.list_messages(&root, q.clone())).await;
                            if let Ok(view) = &r {
                                t.msgs_answered(host, &root, asked, q, view);
                            }
                            r
                        }
                        Claim::Reuse { answer, due } => {
                            note(due);
                            Ok(answer)
                        }
                        Claim::Wait { asked, due } => {
                            note(due);
                            Err(Fail::Deferred { asked, due })
                        }
                    },
                },
            ),
            Ask::Status { src, run } => {
                Answer::Status(src, bounded(deadline, conn.get_run(&run)).await)
            }
        });
    }
    Round { answers, next_due }
}

/// A held-back first ask as a warning: asked at `asked` by another call,
/// its turn at `due`.
fn held_back(asked: Instant, due: Instant) -> String {
    let now = Instant::now();
    format!(
        "asked {} s ago over SSH; next ask in {} s",
        now.saturating_duration_since(asked).as_secs(),
        due.saturating_duration_since(now).as_secs_f64().ceil() as u64
    )
}

async fn bounded<T>(
    deadline: Instant,
    request: impl Future<Output = Result<T, HostConnectorError>>,
) -> Result<T, Fail> {
    let sent = Instant::now();
    match tokio::time::timeout_at(deadline, request).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(Fail::Conn(e)),
        Err(_) => Err(Fail::Late {
            had: deadline.saturating_duration_since(sent),
        }),
    }
}

/// Which request gave no answer.
#[derive(Clone, Copy, PartialEq)]
enum Asked {
    /// A source's first.
    First,
    /// A later poll (a gate, a space, a stream reopen).
    Poll,
    /// A run's status after its stream's `end`.
    Status,
}

impl Asked {
    /// A source's ask, by whether it had `answered` before.
    fn of(answered: bool) -> Self {
        if answered {
            Asked::Poll
        } else {
            Asked::First
        }
    }
}

impl First {
    /// Take the answer `r` to this source's ask (`source`'s): how to report
    /// a failure of it, or `None` when there is nothing more to do with it.
    /// A first ask held back within `first_by` keeps the source waiting for
    /// its first answer (its host's next round asks at its turn); one held
    /// back past it is named in `warnings`; a later one is no news.
    fn take<T>(
        &mut self,
        r: &Result<T, Fail>,
        first_by: Instant,
        got: &mut Got,
        source: &str,
    ) -> Option<Asked> {
        let asked = Asked::of(self.answered);
        if let Err(Fail::Deferred { asked: at, due }) = r {
            if !self.answered {
                if *due <= first_by {
                    self.held = Some((*at, *due));
                } else {
                    self.answered = true;
                    self.held = None;
                    got.warnings
                        .insert(source.to_string(), held_back(*at, *due));
                }
            }
            return None;
        }
        self.answered = true;
        self.held = None;
        Some(asked)
    }
}

/// A request for `source` that gave no answer: the call's error on the first
/// round, else a warning. A request the call's timeout cut short is only a
/// warning. A later poll's is one only when it had [`FIRST_ROUND_BUDGET`] (the
/// time a host is given to answer): a poll that went out just before the
/// timeout says nothing about the host, and the next poll asks again. A
/// status's always is, since its `run_settled` fires without one. (An ask the
/// [`Throttle`] held back is taken by [`First::take`].)
fn failed(
    got: &mut Got,
    asked: Asked,
    source: &str,
    f: Fail,
    describe: impl FnOnce(HostConnectorError) -> String,
) -> Result<(), String> {
    let msg = match f {
        Fail::Late { had } if asked != Asked::Poll || had >= FIRST_ROUND_BUDGET => {
            "no answer before the call's timeout".to_string()
        }
        Fail::Late { .. } => return Ok(()),
        // Taken by `First::take` before it gets here.
        Fail::Deferred { .. } => return Ok(()),
        Fail::Conn(e) if asked == Asked::First => return Err(describe(e)),
        Fail::Conn(e) => describe(e),
    };
    got.warnings.insert(source.to_string(), msg);
    Ok(())
}

/// A connector failure as the call reports it.
fn describe(host: &str, what: &str, noun: &str, target: &str, e: HostConnectorError) -> String {
    match e {
        HostConnectorError::Unsupported(why) => format!("host `{host}` can't {what} ({why})"),
        HostConnectorError::Remote(501, body) => format!(
            "host `{host}` can't {what} ({})",
            crate::api::runs::remote_error_text(&body)
        ),
        HostConnectorError::NotFound(_) => format!("no {noun} `{target}` on host `{host}`"),
        other => format!("host `{host}`: {other}"),
    }
}

/// The gates a remote run's `get_run` reply says are waiting, in the detail
/// shape of a local `run_gate`: its `awaiting` set, else (a record from
/// before it) the legacy `awaiting_step_id`.
pub(super) fn gates_of(reply: &Value) -> Vec<Value> {
    let run = reply.get("run").unwrap_or(reply);
    if let Some(set) = run
        .get("awaiting")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
    {
        return set
            .iter()
            .map(|g| {
                json!({
                    "step_id": g.get("step_id"),
                    "prompt": g.get("prompt"),
                    "expires_at": g.get("expires_at"),
                })
            })
            .collect();
    }
    match run.get("awaiting_step_id").and_then(Value::as_str) {
        Some(step_id) => vec![json!({
            "step_id": step_id,
            "prompt": run.get("approval_prompt"),
            "expires_at": run.get("expires_at"),
        })],
        None => Vec::new(),
    }
}

/// Keep the messages of a poll of `source` its conditions want, each in the
/// shape a local `messages` event has (its `LogLine::Msg` line) plus the
/// `seen` receipts the view folds in, and move its cursor to the last one
/// read.
fn take_messages(watch: &[Condition], source: &str, view: MessagesView, got: &mut Got) {
    for m in view.messages {
        let id = m.message.id.clone();
        let kind = m.message.kind;
        let mut taken = false;
        for (c, cond) in watch.iter().enumerate() {
            let Kind::Messages { kinds } = &cond.kind else {
                continue;
            };
            if cond.source == source && kinds.as_ref().is_none_or(|k| k.contains(&kind)) {
                *got.counts.entry(c).or_default() += 1;
                taken = true;
            }
        }
        if taken {
            if let Ok(mut event) = serde_json::to_value(LogLine::Msg(m.message)) {
                event["seen"] = json!(m.seen);
                got.events
                    .push(json!({"source": source, "id": id, "event": event}));
            }
        }
        got.ids.insert(source.to_string(), id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn past_the_deadline_no_poll_is_due_to_wake_the_loop() {
        use crate::host::connector::testing::StubConnector;
        let now = Instant::now();
        let mut r = Remote {
            hosts: vec![Host {
                id: "h".into(),
                conn: Arc::new(StubConnector::default()),
                busy: false,
                poll: REMOTE_POLL,
                spaced: false,
                next_poll: now - Duration::from_secs(1),
            }],
            runs: Vec::new(),
            gates: Vec::new(),
            msgs: Vec::new(),
            turn: 0,
            inflight: FuturesUnordered::new(),
            throttle: Arc::default(),
            first_by: Instant::now(),
        };
        // Before the deadline the due poll is the wake target.
        assert!(r.next_poll_at(now + Duration::from_secs(5)).is_some());
        // Past it nothing is: `schedule` would start no round and leave
        // `next_poll` behind, so the loop would spin on it.
        let past = now - Duration::from_millis(1);
        assert!(r.next_poll_at(past).is_none());
        r.schedule(&[], &Got::default(), past);
        assert!(!r.hosts[0].busy);
    }

    /// A host with the stub connector, its next poll due.
    fn idle_host(id: &str) -> Host {
        use crate::host::connector::testing::StubConnector;
        Host {
            id: id.into(),
            conn: Arc::new(StubConnector::default()),
            busy: false,
            poll: REMOTE_POLL,
            spaced: false,
            next_poll: Instant::now() - Duration::from_secs(1),
        }
    }

    /// While waiting for first answers, only the hosts that can bring one
    /// are asked: a status another host owes isn't started there, where its
    /// round would land unread.
    #[tokio::test]
    async fn waiting_for_first_answers_asks_only_their_hosts() {
        let req = super::super::parse::parse(json!({"watch": [
            {"kind": "run_gate", "run": "run_g", "host": "a"},
            {"kind": "run_settled", "run": "run_r", "host": "b"}]}))
        .unwrap();
        let mut r = Remote {
            hosts: vec![idle_host("a"), idle_host("b")],
            runs: vec![RunSrc {
                host: 1,
                run: "run_r".into(),
                source: req.watch[1].source.clone(),
                collects: false,
                stream: None,
                ended: true,
                needs_status: true,
                answered: true,
            }],
            gates: vec![GateSrc {
                host: 0,
                run: "run_g".into(),
                source: req.watch[0].source.clone(),
                first: First {
                    answered: false,
                    held: Some((Instant::now(), Instant::now())),
                },
            }],
            msgs: Vec::new(),
            turn: 0,
            inflight: FuturesUnordered::new(),
            throttle: Arc::default(),
            first_by: Instant::now() + FIRST_ROUND_BUDGET,
        };
        let far = Instant::now() + Duration::from_secs(60);
        r.start_due(&req.watch, &Got::default(), far, true);
        assert!(r.hosts[0].busy, "the held gate's host is asked");
        // Its ask is out: no longer held, so an unanswered one reads as any.
        assert!(r.gates[0].first.held.is_none());
        assert!(!r.hosts[1].busy, "the status host is left for the loop");
        r.start_due(&req.watch, &Got::default(), far, false);
        assert!(r.hosts[1].busy);

        // The gate's ask went out at its turn and hasn't answered by the
        // cut: named as an unanswered first ask, not as the other call's.
        r.inflight = FuturesUnordered::new();
        r.hosts[0].busy = false;
        let mut got = Got::default();
        let now = Instant::now();
        r.settle_first(now, far, &req.watch, &mut got)
            .await
            .unwrap();
        assert_eq!(
            got.warnings[&req.watch[0].source],
            "no first answer within 5 s"
        );
    }

    #[tokio::test]
    async fn a_host_that_connects_per_request_is_polled_every_10s() {
        use crate::host::connector::testing::StubConnector;
        let ssh = StubConnector {
            connects_per_request: true,
            ..Default::default()
        };
        assert_eq!(poll_every(&ssh), SSH_POLL);
        assert!(SSH_POLL >= Duration::from_secs(10));
        assert_eq!(poll_every(&StubConnector::default()), REMOTE_POLL);

        // The host's interval is what `start` schedules its next round at.
        let mut r = Remote {
            hosts: vec![Host {
                id: "h".into(),
                poll: poll_every(&ssh),
                spaced: true,
                conn: Arc::new(ssh),
                busy: false,
                next_poll: Instant::now(),
            }],
            runs: Vec::new(),
            gates: Vec::new(),
            msgs: Vec::new(),
            turn: 0,
            inflight: FuturesUnordered::new(),
            throttle: Arc::default(),
            first_by: Instant::now(),
        };
        let before = Instant::now();
        let far = before + Duration::from_secs(60);
        r.start(&[], &Got::default(), far);
        assert!(r.next_poll_at(far).unwrap() >= before + SSH_POLL);
    }

    #[test]
    fn a_late_answer_warns_only_when_the_host_had_time_to_answer() {
        let late = |had| Fail::Late { had };
        let mut got = Got::default();
        // A later round that went out just before the timeout: no news.
        let short = || late(Duration::from_millis(300));
        failed(&mut got, Asked::Poll, "s", short(), |e| e.to_string()).unwrap();
        assert!(got.warnings.is_empty());
        // One given the time a host has to answer, and didn't: a warning.
        failed(&mut got, Asked::Poll, "s", late(FIRST_ROUND_BUDGET), |e| {
            e.to_string()
        })
        .unwrap();
        assert_eq!(got.warnings["s"], "no answer before the call's timeout");
        // A first round's and a status's always are: a status cut short
        // leaves `run_settled` firing with `status: null`.
        for asked in [Asked::First, Asked::Status] {
            let mut got = Got::default();
            failed(&mut got, asked, "s", short(), |e| e.to_string()).unwrap();
            assert_eq!(got.warnings["s"], "no answer before the call's timeout");
        }
    }

    #[test]
    fn gates_come_from_the_awaiting_set_else_the_legacy_fields() {
        let set = json!({"run": {"awaiting": [{"step_id": "a", "prompt": "ok?", "since": "x"}],
            "awaiting_step_id": "a"}});
        assert_eq!(
            gates_of(&set),
            vec![json!({"step_id": "a", "prompt": "ok?", "expires_at": null})]
        );
        let legacy = json!({"run": {"awaiting": [], "awaiting_step_id": "b",
            "approval_prompt": "go?", "expires_at": "2026-10-19T00:00:00Z"}});
        assert_eq!(
            gates_of(&legacy),
            vec![json!({"step_id": "b", "prompt": "go?", "expires_at": "2026-10-19T00:00:00Z"})]
        );
        assert!(gates_of(&json!({"run": {"awaiting": []}})).is_empty());
    }

    #[test]
    fn a_failure_reads_by_its_kind() {
        let d = |e| describe("h1", "read messages", "session", "ses_a", e);
        assert_eq!(
            d(HostConnectorError::Unsupported("too old".into())),
            "host `h1` can't read messages (too old)"
        );
        assert_eq!(
            d(HostConnectorError::Remote(501, r#"{"error":"no"}"#.into())),
            "host `h1` can't read messages (no)"
        );
        assert_eq!(
            d(HostConnectorError::NotFound("x".into())),
            "no session `ses_a` on host `h1`"
        );
        assert_eq!(
            d(HostConnectorError::Unreachable("refused".into())),
            "host `h1`: host unreachable: refused"
        );
    }
}
