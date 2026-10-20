//! The `wait` tool: an MCP client can't read SSE, so it long-polls. A call
//! checks the run state it watches first (a settled run, a parked gate fires
//! at once), then tails the run's `events.jsonl` and message spaces' logs
//! from a cursor until a condition fires, a watched stream yields a line, or
//! the timeout.
//!
//! Cursor ids are the line numbers the CP's SSE streams send as `id:`
//! ([`NumberedTail`]), so an agent mixing `wait` with the streams never sees a
//! gap or a repeat. A run on a registered host is read from the host's event
//! stream, and its gates and a remote message space are polled ([`remote`]).

mod findings;
mod local;
mod parse;
mod remote;
mod turns;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt as _;
use rupu_fleet::LogLine;
use rupu_mcp::{CallCx, Progress, ToolDef, ToolHints, ToolResult, ToolSource};
use rupu_orchestrator::executor::Event;
use serde_json::{json, Value};
use tokio::time::Instant;

use crate::state::AppState;
use crate::transcript_tail::{count_numbered, NumberedTail};

use local::{catch_up, locate, matching, Feed, Line, Located, Tail};
use parse::{input_schema, parse, Condition, CursorValue, Kind, Request};
use remote::{Remote, RemoteWake, FIRST_ROUND_BUDGET};

/// The timeout when a call names none.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(30_000);
/// The longest a call waits, whatever it asks for.
pub const MAX_TIMEOUT: Duration = Duration::from_millis(55_000);
/// The most lines one catch-up read returns per source; the cursor then
/// stops at the last of them, so the next call reads on.
const CATCH_UP_MAX: usize = 1000;
/// How often the state conditions are re-checked with no line arriving.
const RECHECK_EVERY: Duration = Duration::from_secs(1);
/// The least time between two re-checks a line triggers.
const RECHECK_MIN_GAP: Duration = Duration::from_millis(200);
/// How long a run or session turn still appearing is looked for before the
/// call says it doesn't exist.
pub(crate) const APPEAR_GRACE: Duration = Duration::from_secs(10);
/// How often a call reports progress to a client that asked for it.
pub(crate) const PROGRESS_EVERY: Duration = Duration::from_secs(5);

/// `timeout_ms` as the call's timeout: [`DEFAULT_TIMEOUT`] when absent,
/// never past [`MAX_TIMEOUT`].
pub fn clamp_timeout(ms: Option<u64>) -> Duration {
    ms.map_or(DEFAULT_TIMEOUT, |ms| {
        Duration::from_millis(ms).min(MAX_TIMEOUT)
    })
}

/// The `wait` tool's definition (what [`WaitSource`] lists).
pub fn tool_def() -> ToolDef {
    ToolDef {
        name: "wait".into(),
        description: DESCRIPTION.into(),
        input_schema: input_schema(),
        hints: ToolHints {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
    }
}

pub struct WaitSource {
    state: AppState,
}

impl WaitSource {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

const DESCRIPTION: &str =
    "Wait for a run to settle, a gate to park, new run, agentiflow or message \
lines, a standalone agent run to finish, a session turn to end or new finding records to be recorded \
(a tag change on an older finding is not one). Returns at once when a watched \
state already holds, with two exceptions: a remote `run_settled` fires once the host's event \
stream ends, about 2 s after the run's last event; and a call watching a remote host waits up to \
5 s from its start for that host's first answer, even when something already fired. Otherwise \
it returns when a condition fires, a watched stream has new lines, or the timeout. For \
`session_turn` pass the `run` id `sessions.send` returned; without it the call waits \
for the session's active turn, or the next one if it is idle. An error in any one condition (a \
run, session or turn that doesn't appear within 10 s) fails the whole call. Pass the returned \
`cursor` back to resume where the last call left off. With `host`, a run's events come from that \
host's event stream and its gates and a message space are polled every 2 s (10 s over SSH); a host \
that stops answering after its first answer is retried and named in `warnings`.";

#[async_trait]
impl ToolSource for WaitSource {
    fn tools(&self) -> Vec<ToolDef> {
        vec![tool_def()]
    }

    async fn call(&self, name: &str, args: Value, cx: &CallCx) -> Option<ToolResult> {
        if name != "wait" {
            return None;
        }
        Some(match parse(args) {
            Ok(req) => self.wait(req, cx.progress.as_ref()).await,
            Err(e) => ToolResult::Error(e),
        })
    }
}

impl WaitSource {
    async fn wait(&self, mut req: Request, progress: Option<&Progress>) -> ToolResult {
        let began = Instant::now();
        let deadline = began + req.timeout;
        if let Err(e) = findings::validate(&self.state, &req.watch).await {
            return ToolResult::Error(e);
        }
        // Every local run condition's run exists, wherever it lives.
        let located = match locate(&self.state, &req.watch).await {
            Ok(l) => l,
            Err(e) => return ToolResult::Error(e),
        };
        // A run a registered host has is watched there, as if `host` named it.
        for c in &mut req.watch {
            if c.host.is_none()
                && matches!(c.kind, Kind::Settled | Kind::Gate | Kind::Events { .. })
            {
                if let Some(Located::Host(h)) = located.get(&c.target) {
                    c.on_host(h.clone());
                }
            }
        }
        // A string cursor value names a remote message space, and a remote
        // message space's cursor is a string.
        let remote_spaces: HashSet<&str> = req
            .watch
            .iter()
            .filter(|c| c.host.is_some() && matches!(c.kind, Kind::Messages { .. }))
            .map(|c| c.source.as_str())
            .collect();
        for (k, v) in &req.cursor {
            match (v, remote_spaces.contains(k.as_str())) {
                (CursorValue::Id(_), false) => {
                    return ToolResult::Error(format!(
                        "`cursor.{k}` must be a non-negative integer: a string cursor is only for a remote host's message space"
                    ))
                }
                (CursorValue::Line(_), true) => {
                    return ToolResult::Error(format!(
                        "`cursor.{k}` must be a string: a remote message space's cursor is the id of the last message seen"
                    ))
                }
                _ => {}
            }
        }
        let mut remote = match Remote::new(&self.state, &req.watch) {
            Ok(r) => r,
            Err(e) => return ToolResult::Error(e),
        };

        // Each local source's feed.
        let mut feeds: Vec<(String, Feed)> = Vec::new();
        for c in &req.watch {
            if c.host.is_some() || feeds.iter().any(|(s, _)| *s == c.source) {
                continue;
            }
            let feed = match &c.kind {
                Kind::Messages { .. } => {
                    let global = self.state.global_dir.clone();
                    let id = c.target.clone();
                    match tokio::task::spawn_blocking(move || {
                        crate::host::messages::root(&global, &id).map(|(_, r)| r.log_path())
                    })
                    .await
                    {
                        Ok(Ok(path)) => Feed::Log(path),
                        Ok(Err(e)) => return ToolResult::Error(e.message),
                        Err(e) => {
                            return ToolResult::Error(format!("resolving `{}`: {e}", c.target))
                        }
                    }
                }
                Kind::Agentiflow { .. } => {
                    let dir =
                        rupu_agentiflow::agentiflow_dir(&self.state.global_dir).join(&c.target);
                    if !dir.is_dir() {
                        return ToolResult::Error(format!("no agentiflow `{}`", c.target));
                    }
                    Feed::Flow(dir.join("events.jsonl"))
                }
                // Watched by re-checking their state; nothing to tail.
                Kind::AgentRunEnd | Kind::SessionTurn { .. } | Kind::Findings { .. } => continue,
                _ => match located.get(&c.target) {
                    Some(Located::Store(store)) => Feed::Run(store.events_path(&c.target)),
                    // Nothing was ever written: only `run_settled` can be
                    // watching it, and it fires from the state check.
                    _ => continue,
                },
            };
            feeds.push((c.source.clone(), feed));
        }

        // Which conditions read each source, and whether any collects.
        let mut by_source: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, c) in req.watch.iter().enumerate() {
            by_source.entry(c.source.as_str()).or_default().push(i);
        }
        let collects: Vec<bool> = feeds
            .iter()
            .map(|(s, _)| {
                by_source[s.as_str()]
                    .iter()
                    .any(|&i| req.watch[i].kind.collects())
            })
            .collect();
        let has_state = req.watch.iter().any(|c| !c.kind.collects());

        let mut got = Got::default();
        for (k, v) in &req.cursor {
            match v {
                CursorValue::Line(n) => {
                    got.cursor.insert(k.clone(), *n);
                }
                CursorValue::Id(id) => {
                    got.ids.insert(k.clone(), id.clone());
                }
            }
        }

        // State first, then what is already on disk past the cursor: a run
        // seen settled has written its last line, so the catch-up after it
        // has every event. Either returns at once. The first remote round
        // (every remote source's first answer, its state included, and any
        // failure the call's error) goes out beside them; it may outlast a
        // shorter timeout until `first_by`, so a `timeout_ms: 0` call still
        // peeks. Once something fired, it is waited for only until
        // `first_cut`.
        let mut started = turns::Turns::new();
        let mut ledgers = findings::Findings::default();
        let first_cut = Instant::now() + FIRST_ROUND_BUDGET;
        let first_by = deadline.max(first_cut);
        remote.start(&req.watch, &got, first_by);
        if let Err(e) = self
            .check(&req.watch, &located, (&mut started, &mut ledgers), &mut got)
            .await
        {
            return ToolResult::Error(e);
        }
        if let Err(e) = catch_up(&req.watch, &feeds, &collects, &by_source, &mut got).await {
            return ToolResult::Error(e);
        }
        if got.any() {
            if let Err(e) = remote
                .settle_first(first_cut, deadline, &req.watch, &mut got)
                .await
            {
                return ToolResult::Error(e);
            }
            remote.drain(&req.watch, &mut got, deadline).await;
            return got.finish(&req.watch);
        }
        // Otherwise the first rounds stay in flight in the loop below, beside
        // the tails.

        let mut tails: Vec<Tail> = Vec::new();
        for (i, (source, feed)) in feeds.iter().enumerate() {
            let after = if collects[i] {
                got.cursor.get(source).copied().unwrap_or(0)
            } else {
                // Watched for run state only: a line just prompts a
                // re-check, so the tail starts past what is there now.
                let p = feed.path().to_path_buf();
                match tokio::task::spawn_blocking(move || count_numbered(&p)).await {
                    Ok(Ok(n)) => n,
                    Ok(Err(e)) => return ToolResult::Error(format!("reading {source}: {e}")),
                    Err(e) => return ToolResult::Error(format!("reading {source}: {e}")),
                }
            };
            tails.push(match feed {
                Feed::Run(p) => {
                    Box::pin(NumberedTail::<Event>::open(p, after).map(move |l| (i, Line::Run(l))))
                }
                Feed::Log(p) => Box::pin(
                    NumberedTail::<LogLine>::open(p, after).map(move |l| (i, Line::Msg(l))),
                ),
                Feed::Flow(p) => {
                    Box::pin(NumberedTail::<Value>::open(p, after).map(move |l| (i, Line::Flow(l))))
                }
            });
        }
        // Conditions that tail nothing (a run or turn watched by state) leave
        // no stream to end the wait.
        let tailing = !tails.is_empty();
        let mut lines = futures_util::stream::select_all(tails);

        let mut reporter = progress.map(|p| Reporter::new(p, began, &req));
        let mut next_progress = Instant::now();
        let mut next_check = Instant::now() + RECHECK_EVERY;
        let mut last_check = Instant::now();
        // The last first round landed past the deadline (a `timeout_ms: 0`
        // peek): the streams it opened are read for what the host already
        // has, as a local peek's catch-up reads what is on disk.
        let mut peeked = false;
        loop {
            // Each idle host's round that is due goes out; the hosts' rounds
            // are polled beside the other sources, so a slow host holds up
            // nothing else.
            remote.schedule(&req.watch, &got, deadline);
            // Past the deadline, only a first round's answer is waited for.
            let end = if remote.first_pending() {
                first_by
            } else {
                deadline
            };
            // Progress stops at the deadline: it never passes `total`.
            let reporting = Instant::now() < deadline;
            if let (true, Some(r)) = (reporting, reporter.as_mut()) {
                if Instant::now() >= next_progress {
                    r.report();
                    next_progress = Instant::now() + PROGRESS_EVERY;
                }
            }
            let mut wake_at = match remote.next_poll_at(deadline) {
                Some(at) => next_check.min(at).min(end),
                None => next_check.min(end),
            };
            if let Some(at) =
                progress_wake(reporter.is_some() && reporting, next_progress, deadline)
            {
                wake_at = wake_at.min(at);
            }
            let wake = tokio::select! {
                line = async {
                    if tailing {
                        lines.next().await
                    } else {
                        std::future::pending().await
                    }
                } => Wake::Line(line),
                remote_wake = remote.next() => Wake::Remote(remote_wake),
                _ = tokio::time::sleep_until(wake_at) => Wake::Tick,
            };
            match wake {
                Wake::Line(line) => {
                    let Some((src, line)) = line else { break };
                    if !line.send() {
                        continue;
                    }
                    let source = feeds[src].0.as_str();
                    if collects[src] {
                        got.cursor.insert(source.to_string(), line.id());
                    }
                    if got.take(&req.watch, &by_source[source], source, &line) {
                        break;
                    }
                    if has_state && last_check.elapsed() >= RECHECK_MIN_GAP {
                        last_check = Instant::now();
                        if let Err(e) = self
                            .check(&req.watch, &located, (&mut started, &mut ledgers), &mut got)
                            .await
                        {
                            return ToolResult::Error(e);
                        }
                        if got.any() {
                            break;
                        }
                    }
                }
                Wake::Remote(RemoteWake::Frame(src, frame)) => {
                    remote.on_frame(src, frame, &req.watch, &mut got);
                    if got.any() {
                        break;
                    }
                }
                Wake::Remote(RemoteWake::Round(host, answers)) => {
                    let first_was_pending = remote.first_pending();
                    if let Err(e) = remote.landed(host, answers, &req.watch, &mut got) {
                        return ToolResult::Error(e);
                    }
                    if got.any() {
                        break;
                    }
                    if Instant::now() >= deadline && !remote.first_pending() {
                        peeked = first_was_pending;
                        break;
                    }
                }
                Wake::Tick => {
                    if Instant::now() >= end {
                        break;
                    }
                    if Instant::now() >= next_check {
                        if has_state {
                            last_check = Instant::now();
                            if let Err(e) = self
                                .check(&req.watch, &located, (&mut started, &mut ledgers), &mut got)
                                .await
                            {
                                return ToolResult::Error(e);
                            }
                            if got.any() {
                                break;
                            }
                        }
                        next_check = Instant::now() + RECHECK_EVERY;
                    }
                }
            }
        }
        drop(lines);
        // Something fired before every host's first round answered.
        if let Err(e) = remote
            .settle_first(first_cut, deadline, &req.watch, &mut got)
            .await
        {
            return ToolResult::Error(e);
        }

        // Something fired: return the lines on disk past the cursor with it,
        // rather than racing the tails' next poll for them.
        if got.any() {
            if let Err(e) = catch_up(&req.watch, &feeds, &collects, &by_source, &mut got).await {
                return ToolResult::Error(e);
            }
            remote.drain(&req.watch, &mut got, deadline).await;
        } else if peeked {
            // The deadline has passed: bounded like the first round.
            remote.drain(&req.watch, &mut got, first_by).await;
        }
        got.finish(&req.watch)
    }
}

/// When the next progress report should wake the loop: only while reports
/// are still being made (before the deadline) and the next is due before it.
/// A due time the loop wouldn't act on would wake it again at once.
fn progress_wake(reporting: bool, next: Instant, deadline: Instant) -> Option<Instant> {
    (reporting && next < deadline).then_some(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_report_due_past_the_loop_top_deadline_does_not_wake_it() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(1);
        assert_eq!(progress_wake(true, now, deadline), Some(now));
        // Not reporting (the loop top is past the deadline): the stale due
        // time must not become the wake target.
        assert_eq!(progress_wake(false, now, deadline), None);
        assert_eq!(progress_wake(true, deadline, deadline), None);
    }
}

/// Reports a waiting call's progress: elapsed milliseconds out of the
/// timeout, naming the kinds watched. Holds the call's `Progress` only for
/// the call.
struct Reporter<'a> {
    progress: &'a Progress,
    began: Instant,
    total_ms: f64,
    message: String,
    last: f64,
}

impl<'a> Reporter<'a> {
    fn new(progress: &'a Progress, began: Instant, req: &Request) -> Self {
        let mut kinds: Vec<&str> = Vec::new();
        for c in &req.watch {
            let n = c.kind.name();
            if !kinds.contains(&n) {
                kinds.push(n);
            }
        }
        Self {
            progress,
            began,
            total_ms: req.timeout.as_millis() as f64,
            message: format!("waiting: {}", kinds.join(", ")),
            last: -1.0,
        }
    }

    /// One report; `progress` grows strictly with each.
    fn report(&mut self) {
        let elapsed = (self.began.elapsed().as_millis() as f64).max(self.last + 1.0);
        self.last = elapsed;
        self.progress
            .report(elapsed, Some(self.total_ms), Some(&self.message));
    }
}

/// What woke the wait loop.
// Moved once per wake-up; boxing the line would allocate for every one.
#[allow(clippy::large_enum_variant)]
enum Wake {
    Line(Option<(usize, Line)>),
    Remote(RemoteWake),
    Tick,
}

impl WaitSource {
    /// The state conditions, then the `findings` ledgers.
    async fn check(
        &self,
        watch: &[Condition],
        located: &HashMap<String, Located>,
        (turns, ledgers): (&mut turns::Turns, &mut findings::Findings),
        got: &mut Got,
    ) -> Result<(), String> {
        self.check_state(watch, located, turns, &mut got.fired)
            .await?;
        findings::poll(&self.state, watch, ledgers, got).await
    }
}

/// What a call has found so far.
#[derive(Default)]
struct Got {
    /// State conditions that fired, with their detail.
    fired: BTreeMap<usize, Value>,
    /// `{source, id, event}` per line returned.
    events: Vec<Value>,
    /// Lines taken per collecting condition.
    counts: BTreeMap<usize, u64>,
    /// Per source: the last line id read.
    cursor: BTreeMap<String, u64>,
    /// Per remote message space: the id of the last message read.
    ids: BTreeMap<String, String>,
    /// Per source: the last failure of a remote request after its first
    /// answer.
    warnings: BTreeMap<String, String>,
}

impl Got {
    fn any(&self) -> bool {
        !self.fired.is_empty() || !self.events.is_empty()
    }

    /// Keep `line` of `source` when a condition among `conds` takes it.
    fn take(&mut self, watch: &[Condition], conds: &[usize], source: &str, line: &Line) -> bool {
        match matching(watch, conds, line, &mut self.counts) {
            Some(event) => {
                self.events
                    .push(json!({"source": source, "id": line.id(), "event": event}));
                true
            }
            None => false,
        }
    }

    fn finish(mut self, watch: &[Condition]) -> ToolResult {
        for (i, n) in std::mem::take(&mut self.counts) {
            self.fired.insert(i, json!({ "count": n }));
        }
        let timed_out = self.fired.is_empty();
        let fired: Vec<Value> = self
            .fired
            .into_iter()
            .map(|(i, detail)| json!({"index": i, "kind": watch[i].kind.name(), "detail": detail}))
            .collect();
        let mut cursor: serde_json::Map<String, Value> = self
            .cursor
            .into_iter()
            .map(|(k, n)| (k, json!(n)))
            .collect();
        cursor.extend(self.ids.into_iter().map(|(k, id)| (k, json!(id))));
        let mut out = json!({
            "fired": fired,
            "events": self.events,
            "cursor": cursor,
            "timed_out": timed_out,
        });
        if !self.warnings.is_empty() {
            let warnings: Vec<String> = self
                .warnings
                .into_iter()
                .map(|(source, e)| format!("{source}: {e}"))
                .collect();
            out["warnings"] = json!(warnings);
        }
        ToolResult::Json(out)
    }
}
