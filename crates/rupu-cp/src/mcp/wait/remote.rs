//! The remote sources of `wait`. A run on a registered host is read from the
//! host's event stream (`stream_run_events_after`, resumed from the cursor):
//! its events feed `run_events`, its `end` frame fires `run_settled`. Its
//! gates are polled through `get_run`, and a remote message space through
//! `list_messages` from the id of the last message seen (a string cursor).
//!
//! Requests go out in rounds: the first at the call's start, then one every
//! [`REMOTE_POLL`] (or at once when an `end` frame needs its run's status).
//! A round is an owned future the wait loop polls beside its other sources,
//! so a slow host delays nothing else; within it, each host's requests go
//! one after another (at most one in flight per host), and only one round is
//! in flight at a time. A stream is its host's long-lived connection, not a
//! poll; one that ends without its `end` frame is reopened from its cursor at
//! the next round, silently when the reopen succeeds. The first request for a
//! source failing fails the call; a later failure is retried next round and
//! reported in the result's `warnings`. The first round may run up to
//! [`FIRST_ROUND_BUDGET`] past a shorter timeout (a `timeout_ms: 0` peek), and
//! holds back a result something else fired by at most that.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use futures_util::Stream;
use rupu_fleet::{LogLine, MessageKind};
use rupu_runtime::services::operator::{MessagesView, ViewQuery};
use serde_json::{json, Value};
use tokio::time::Instant;

use super::parse::{Condition, Kind};
use super::{Got, CATCH_UP_MAX};
use crate::host::connector::{EventByteStream, HostConnector, HostConnectorError};
use crate::host::messages::MessageRoute;
use crate::sse::{parse_run_frames, RunFrame};
use crate::state::AppState;

/// How often a remote run's gates and a remote message space are polled, and
/// an ended stream is reopened.
pub(super) const REMOTE_POLL: Duration = Duration::from_secs(2);
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
}

/// A remote run whose gates are polled.
struct GateSrc {
    host: usize,
    run: String,
    source: String,
}

/// A remote message space, polled.
struct MsgSrc {
    host: usize,
    root: String,
    source: String,
    /// The kinds its conditions want; every kind when one wants all.
    kinds: Option<Vec<MessageKind>>,
}

/// The remote side of one call.
pub(super) struct Remote {
    hosts: Vec<Host>,
    runs: Vec<RunSrc>,
    gates: Vec<GateSrc>,
    msgs: Vec<MsgSrc>,
    /// Where the next look for a frame starts, so no stream starves another.
    turn: usize,
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
pub(super) enum RoundKind {
    /// Reopen streams, poll gates and spaces, and ask the statuses owed.
    All,
    /// Only the statuses owed.
    Status,
}

/// A round in flight: its answers, once every host has given them.
pub(super) type Round = Pin<Box<dyn Future<Output = Vec<Answer>> + Send>>;

/// Why a request gave no answer.
pub(super) enum Fail {
    Conn(HostConnectorError),
    /// The call's timeout came first.
    Late,
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
    pub(super) fn new(state: &AppState, watch: &[Condition]) -> Result<Self, String> {
        let mut r = Remote {
            hosts: Vec::new(),
            runs: Vec::new(),
            gates: Vec::new(),
            msgs: Vec::new(),
            turn: 0,
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
                        conn,
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
                        }),
                    }
                }
                Kind::Gate => {
                    if !r.gates.iter().any(|s| s.source == c.source) {
                        r.gates.push(GateSrc {
                            host,
                            run: c.target.clone(),
                            source: c.source.clone(),
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
                        }),
                    }
                }
                // The other kinds take no `host`.
                _ => {}
            }
        }
        Ok(r)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    /// An `end` frame left `run_settled` conditions waiting for a status.
    pub(super) fn owes_status(&self) -> bool {
        self.runs.iter().any(|s| s.needs_status)
    }

    /// The next round's requests as an owned future, each bounded by
    /// `deadline`; `None` when there is nothing to ask.
    pub(super) fn round(
        &self,
        watch: &[Condition],
        got: &Got,
        deadline: Instant,
        kind: RoundKind,
    ) -> Option<Round> {
        let mut asks: Vec<Vec<Ask>> = self.hosts.iter().map(|_| Vec::new()).collect();
        for (src, s) in self.runs.iter().enumerate() {
            if s.needs_status {
                asks[s.host].push(Ask::Status {
                    src,
                    run: s.run.clone(),
                });
            } else if kind == RoundKind::All && s.stream.is_none() && !s.ended {
                let after = if s.collects {
                    got.cursor.get(&s.source).copied().unwrap_or(0)
                } else {
                    NO_EVENTS
                };
                asks[s.host].push(Ask::Open {
                    src,
                    run: s.run.clone(),
                    after,
                });
            }
        }
        if kind == RoundKind::All {
            for (src, s) in self.gates.iter().enumerate() {
                let pending = watch.iter().enumerate().any(|(i, c)| {
                    c.source == s.source
                        && matches!(c.kind, Kind::Gate)
                        && !got.fired.contains_key(&i)
                });
                if pending {
                    asks[s.host].push(Ask::Gate {
                        src,
                        run: s.run.clone(),
                    });
                }
            }
            for (src, s) in self.msgs.iter().enumerate() {
                asks[s.host].push(Ask::Msgs {
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
        if asks.iter().all(Vec::is_empty) {
            return None;
        }
        // Hosts side by side; each host's requests one after another.
        let hosts: Vec<_> = self
            .hosts
            .iter()
            .zip(asks)
            .map(|(h, asks)| ask_host(Arc::clone(&h.conn), asks, deadline))
            .collect();
        Some(Box::pin(async move {
            futures_util::future::join_all(hosts)
                .await
                .into_iter()
                .flatten()
                .collect()
        }))
    }

    /// Take a round's answers. On the `first` round any failure is the call's
    /// error; later, a warning.
    pub(super) fn apply(
        &mut self,
        answers: Vec<Answer>,
        watch: &[Condition],
        got: &mut Got,
        first: bool,
    ) -> Result<(), String> {
        for answer in answers {
            match answer {
                Answer::Open(i, r) => match r {
                    Ok(bytes) => self.runs[i].stream = Some(Box::pin(parse_run_frames(bytes))),
                    Err(f) => {
                        let s = &self.runs[i];
                        let host = &self.hosts[s.host].id;
                        failed(got, first, &s.source, f, |e| {
                            describe(host, "stream run events", "run", &s.run, e)
                        })?
                    }
                },
                Answer::Gate(i, r) => {
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
                            failed(got, first, &s.source, f, |e| {
                                describe(host, "read runs", "run", &s.run, e)
                            })?
                        }
                    }
                }
                Answer::Msgs(i, r) => {
                    let s = &self.msgs[i];
                    match r {
                        Ok(view) => take_messages(watch, &s.source, view, got),
                        Err(f) => {
                            let host = &self.hosts[s.host].id;
                            let noun = MessageRoute::of_id(&s.root).map_or("run", |r| r.noun());
                            failed(got, first, &s.source, f, |e| {
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
                            let _ = failed(got, false, &s.source, f, |e| {
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

    /// The first round, once something else fired: its answers (any failure
    /// the call's error) if they come by `cut`; else it is dropped, so a slow
    /// host holds the result back by at most [`FIRST_ROUND_BUDGET`], and every
    /// remote source is named in `warnings`.
    pub(super) async fn settle_first(
        &mut self,
        round: Option<Round>,
        cut: Instant,
        watch: &[Condition],
        got: &mut Got,
    ) -> Result<(), String> {
        let Some(round) = round else { return Ok(()) };
        match tokio::time::timeout_at(cut, round).await {
            Ok(answers) => self.apply(answers, watch, got, true),
            Err(_) => {
                let sources = self
                    .runs
                    .iter()
                    .map(|s| &s.source)
                    .chain(self.gates.iter().map(|s| &s.source))
                    .chain(self.msgs.iter().map(|s| &s.source));
                for source in sources {
                    got.warnings.entry(source.clone()).or_insert_with(|| {
                        format!("no first answer within {} s", FIRST_ROUND_BUDGET.as_secs())
                    });
                }
                Ok(())
            }
        }
    }

    /// The next frame of any open stream, with the index of its source;
    /// `None` when that stream ended. Pending while no stream is open.
    pub(super) fn next_frame(
        &mut self,
    ) -> impl Future<Output = (usize, Option<io::Result<RunFrame>>)> + Send + '_ {
        std::future::poll_fn(move |cx| {
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
        })
    }

    /// Take what source `i`'s stream yielded: an event its `run_events`
    /// conditions want; its `end`, which owes its `run_settled` conditions a
    /// status ([`Self::owes_status`]); or its end without one, broken or not,
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
        if let Some(round) = self.round(watch, got, deadline, RoundKind::Status) {
            let answers = round.await;
            // Never an error past the first round.
            let _ = self.apply(answers, watch, got, false);
        }
    }
}

/// `asks` against one host, one after another, each bounded by `deadline`.
async fn ask_host(conn: Arc<dyn HostConnector>, asks: Vec<Ask>, deadline: Instant) -> Vec<Answer> {
    let mut out = Vec::with_capacity(asks.len());
    for ask in asks {
        out.push(match ask {
            Ask::Open { src, run, after } => Answer::Open(
                src,
                bounded(deadline, conn.stream_run_events_after(&run, after)).await,
            ),
            Ask::Gate { src, run } => {
                Answer::Gate(src, bounded(deadline, conn.get_run(&run)).await)
            }
            Ask::Msgs { src, root, q } => {
                Answer::Msgs(src, bounded(deadline, conn.list_messages(&root, q)).await)
            }
            Ask::Status { src, run } => {
                Answer::Status(src, bounded(deadline, conn.get_run(&run)).await)
            }
        });
    }
    out
}

async fn bounded<T>(
    deadline: Instant,
    request: impl Future<Output = Result<T, HostConnectorError>>,
) -> Result<T, Fail> {
    match tokio::time::timeout_at(deadline, request).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(Fail::Conn(e)),
        Err(_) => Err(Fail::Late),
    }
}

/// A request for `source` that gave no answer: the call's error on the first
/// round, else a warning. A request the call's timeout cut short is only a
/// warning.
fn failed(
    got: &mut Got,
    first: bool,
    source: &str,
    f: Fail,
    describe: impl FnOnce(HostConnectorError) -> String,
) -> Result<(), String> {
    let msg = match f {
        Fail::Late => "no answer before the call's timeout".to_string(),
        Fail::Conn(e) if first => return Err(describe(e)),
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
fn gates_of(reply: &Value) -> Vec<Value> {
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
