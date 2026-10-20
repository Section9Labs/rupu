//! The `wait` tool: an MCP client can't read SSE, so it long-polls. A call
//! checks the run state it watches first (a settled run, a parked gate fires
//! at once), then tails the run's `events.jsonl` and message spaces' logs
//! from a cursor until a condition fires, a watched stream yields a line, or
//! the timeout.
//!
//! Cursor ids are the line numbers the CP's SSE streams send as `id:`
//! ([`NumberedTail`]), so an agent mixing `wait` with the streams never sees a
//! gap or a repeat.

mod local;
mod parse;
mod turns;

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt as _;
use rupu_fleet::LogLine;
use rupu_mcp::{CallCx, ToolDef, ToolHints, ToolResult, ToolSource};
use rupu_orchestrator::executor::Event;
use serde_json::{json, Value};
use tokio::time::Instant;

use crate::state::AppState;
use crate::transcript_tail::{count_numbered, NumberedTail};

use local::{catch_up, locate, matching, Feed, Line, Located, Tail};
use parse::{input_schema, parse, Condition, CursorValue, Kind, Request};

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
lines, a standalone agent run to finish or a session turn to end. Returns at once when a watched \
state already holds; otherwise when a condition fires, a watched stream has new lines, or the \
timeout. For `session_turn` pass the `run` id `sessions.send` returned; without it the call waits \
for the session's active turn, or the next one if it is idle. An error in any one condition (a \
run, session or turn that doesn't appear within 10 s) fails the whole call. Pass the returned \
`cursor` back to resume where the last call left off.";

#[async_trait]
impl ToolSource for WaitSource {
    fn tools(&self) -> Vec<ToolDef> {
        vec![tool_def()]
    }

    async fn call(&self, name: &str, args: Value, _cx: &CallCx) -> Option<ToolResult> {
        if name != "wait" {
            return None;
        }
        Some(match parse(args) {
            Ok(req) => self.wait(req).await,
            Err(e) => ToolResult::Error(e),
        })
    }
}

impl WaitSource {
    async fn wait(&self, req: Request) -> ToolResult {
        // Every run condition's run exists, wherever it lives.
        let located = match locate(&self.state, &req.watch).await {
            Ok(l) => l,
            Err(e) => return ToolResult::Error(e),
        };
        // A string cursor value names a remote message space; every source
        // here is local.
        if let Some((k, _)) = req
            .cursor
            .iter()
            .find(|(_, v)| matches!(v, CursorValue::Id(_)))
        {
            return ToolResult::Error(format!(
                "`cursor.{k}` must be a non-negative integer: a string cursor is only for a remote host's message space"
            ));
        }

        // Each source's feed.
        let mut feeds: Vec<(String, Feed)> = Vec::new();
        for c in &req.watch {
            if feeds.iter().any(|(s, _)| *s == c.source) {
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
                Kind::AgentRunEnd | Kind::SessionTurn { .. } => continue,
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

        let mut got = Got {
            cursor: req
                .cursor
                .iter()
                .filter_map(|(k, v)| match v {
                    CursorValue::Line(n) => Some((k.clone(), *n)),
                    CursorValue::Id(_) => None,
                })
                .collect(),
            ..Got::default()
        };

        // State first, then what is already on disk past the cursor: a run
        // seen settled has written its last line, so the catch-up after it
        // has every event. Either returns at once.
        let mut started = turns::Turns::new();
        if let Err(e) = self
            .check_state(&req.watch, &located, &mut started, &mut got.fired)
            .await
        {
            return ToolResult::Error(e);
        }
        if let Err(e) = catch_up(&req.watch, &feeds, &collects, &by_source, &mut got).await {
            return ToolResult::Error(e);
        }
        if got.any() {
            return got.finish(&req.watch);
        }

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

        let deadline = Instant::now() + req.timeout;
        let mut next_check = Instant::now() + RECHECK_EVERY;
        let mut last_check = Instant::now();
        loop {
            tokio::select! {
                line = async {
                    if tailing {
                        lines.next().await
                    } else {
                        std::future::pending().await
                    }
                } => {
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
                            .check_state(&req.watch, &located, &mut started, &mut got.fired)
                            .await
                        {
                            return ToolResult::Error(e);
                        }
                        if !got.fired.is_empty() {
                            break;
                        }
                    }
                }
                _ = tokio::time::sleep_until(next_check.min(deadline)) => {
                    if Instant::now() >= deadline {
                        break;
                    }
                    if has_state {
                        last_check = Instant::now();
                        if let Err(e) = self
                            .check_state(&req.watch, &located, &mut started, &mut got.fired)
                            .await
                        {
                            return ToolResult::Error(e);
                        }
                        if !got.fired.is_empty() {
                            break;
                        }
                    }
                    next_check = Instant::now() + RECHECK_EVERY;
                }
            }
        }
        drop(lines);

        // Something fired: return the lines on disk past the cursor with it,
        // rather than racing the tails' next poll for them.
        if got.any() {
            if let Err(e) = catch_up(&req.watch, &feeds, &collects, &by_source, &mut got).await {
                return ToolResult::Error(e);
            }
        }
        got.finish(&req.watch)
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
    cursor: BTreeMap<String, u64>,
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
        ToolResult::Json(json!({
            "fired": fired,
            "events": self.events,
            "cursor": self.cursor,
            "timed_out": timed_out,
        }))
    }
}
