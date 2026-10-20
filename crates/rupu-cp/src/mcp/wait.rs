//! The `wait` tool: an MCP client can't read SSE, so it long-polls. A call
//! checks the run state it watches first (a settled run, a parked gate fires
//! at once), then tails the run's `events.jsonl` and message spaces' logs
//! from a cursor until a condition fires, a watched stream yields a line, or
//! the timeout.
//!
//! Cursor ids are the line numbers the CP's SSE streams send as `id:`
//! ([`NumberedTail`]), so an agent mixing `wait` with the streams never sees a
//! gap or a repeat.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{Stream, StreamExt as _};
use rupu_fleet::{LogLine, MessageKind};
use rupu_mcp::{CallCx, ToolDef, ToolHints, ToolResult, ToolSource};
use rupu_orchestrator::executor::Event;
use rupu_orchestrator::runs::{RunStore, RunStoreError};
use serde_json::{json, Map, Value};
use tokio::time::Instant;

use crate::state::AppState;
use crate::transcript_tail::{read_numbered, NumberedLine, NumberedTail};

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

/// `timeout_ms` as the call's timeout: [`DEFAULT_TIMEOUT`] when absent,
/// never past [`MAX_TIMEOUT`].
pub fn clamp_timeout(ms: Option<u64>) -> Duration {
    ms.map_or(DEFAULT_TIMEOUT, |ms| {
        Duration::from_millis(ms).min(MAX_TIMEOUT)
    })
}

pub struct WaitSource {
    state: AppState,
}

impl WaitSource {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Settled,
    Gate,
    Events { types: Option<Vec<String>> },
    Messages { kinds: Option<Vec<MessageKind>> },
}

impl Kind {
    fn name(&self) -> &'static str {
        match self {
            Kind::Settled => "run_settled",
            Kind::Gate => "run_gate",
            Kind::Events { .. } => "run_events",
            Kind::Messages { .. } => "messages",
        }
    }

    /// Whether it collects lines (rather than watching run state).
    fn collects(&self) -> bool {
        matches!(self, Kind::Events { .. } | Kind::Messages { .. })
    }
}

#[derive(Debug, Clone)]
struct Condition {
    kind: Kind,
    /// The run id, or the message root id.
    target: String,
    /// `run:<id>` or `messages:<root>`.
    source: String,
}

struct Request {
    watch: Vec<Condition>,
    cursor: BTreeMap<String, u64>,
    timeout: Duration,
}

fn input_schema() -> Value {
    let host = json!({"type": "string", "description": "`local` (the default); remote hosts are not served yet."});
    let run = json!({"type": "string", "description": "A workflow run id."});
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["watch"],
        "properties": {
            "watch": {
                "type": "array",
                "minItems": 1,
                "description": "What to wait for. The call returns when any condition fires.",
                "items": {"oneOf": [
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["kind", "run"],
                        "properties": {
                            "kind": {"type": "string", "enum": ["run_settled", "run_gate"],
                                "description": "`run_settled`: the run ended and nothing will write to it again. `run_gate`: the run has a gate waiting for approval."},
                            "run": run,
                            "host": host,
                        }
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["kind", "run"],
                        "properties": {
                            "kind": {"type": "string", "enum": ["run_events"],
                                "description": "New lines of the run's event log."},
                            "run": run,
                            "types": {"type": "array", "items": {"type": "string"},
                                "description": "Only these event types (`step_started`, `run_completed`, …)."},
                            "host": host,
                        }
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["kind", "root"],
                        "properties": {
                            "kind": {"type": "string", "enum": ["messages"],
                                "description": "New messages in a message space."},
                            "root": {"type": "string",
                                "description": "A run, session or agentiflow id."},
                            "kinds": {"type": "array", "items": {"type": "string",
                                "enum": ["post", "direct", "broadcast", "directive", "retract", "operator"]},
                                "description": "Only these message kinds."},
                            "host": host,
                        }
                    }
                ]}
            },
            "cursor": {
                "type": "object",
                "additionalProperties": {"type": "integer", "minimum": 0},
                "description": "The `cursor` a previous call returned: per source, the last line id already seen (the id the SSE streams use)."
            },
            "timeout_ms": {
                "type": "integer",
                "minimum": 0,
                "description": "How long to wait (default 30000; a longer one is clamped to 55000). A timeout returns `timed_out: true`, not an error."
            }
        }
    })
}

const DESCRIPTION: &str = "Wait for a run to settle, a gate to park, new run events or new \
messages. Returns at once when a watched state already holds; otherwise when a condition \
fires, a watched stream has new lines, or the timeout. Pass the returned `cursor` back to \
resume where the last call left off.";

#[async_trait]
impl ToolSource for WaitSource {
    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "wait".into(),
            description: DESCRIPTION.into(),
            input_schema: input_schema(),
            hints: ToolHints {
                read_only: true,
                destructive: false,
                idempotent: true,
                open_world: false,
            },
        }]
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

/// An object's entries without `null`s (agents send `null` for "absent"),
/// refusing a key outside `allowed`.
fn entries(v: Value, allowed: &[&str], what: &str) -> Result<Map<String, Value>, String> {
    let mut o = match v {
        Value::Object(o) => o,
        Value::Null => Map::new(),
        _ if what.is_empty() => return Err("arguments must be an object".into()),
        _ => return Err(format!("{what} must be an object")),
    };
    o.retain(|_, v| !v.is_null());
    if let Some(k) = o.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(if what.is_empty() {
            format!("unknown argument `{k}`")
        } else {
            format!("{what}: unknown argument `{k}`")
        });
    }
    Ok(o)
}

fn strings(v: Value, key: &str, at: &str) -> Result<Vec<String>, String> {
    let Value::Array(items) = v else {
        return Err(format!("{at}: `{key}` must be an array of strings"));
    };
    items
        .into_iter()
        .map(|i| match i {
            Value::String(s) => Ok(s),
            _ => Err(format!("{at}: `{key}` must be an array of strings")),
        })
        .collect()
}

fn parse(args: Value) -> Result<Request, String> {
    let mut o = entries(args, &["watch", "cursor", "timeout_ms"], "")?;
    let watch = match o.remove("watch") {
        Some(Value::Array(w)) if !w.is_empty() => w,
        _ => return Err("`watch` must be a non-empty array of conditions".into()),
    };
    let watch = watch
        .into_iter()
        .enumerate()
        .map(|(i, c)| condition(i, c))
        .collect::<Result<Vec<_>, _>>()?;
    let cursor = match o.remove("cursor") {
        None => BTreeMap::new(),
        Some(Value::Object(c)) => c
            .into_iter()
            .map(|(k, v)| match v.as_u64() {
                Some(n) => Ok((k, n)),
                None => Err(format!("`cursor.{k}` must be a non-negative integer")),
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("`cursor` must be an object".into()),
    };
    let timeout = match o.remove("timeout_ms") {
        None => None,
        Some(v) => Some(
            v.as_u64()
                .ok_or("`timeout_ms` must be a non-negative integer")?,
        ),
    };
    Ok(Request {
        watch,
        cursor,
        timeout: clamp_timeout(timeout),
    })
}

fn condition(i: usize, c: Value) -> Result<Condition, String> {
    let at = format!("watch[{i}]");
    let kind = c
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            format!("{at}: `kind` must be one of run_settled, run_gate, run_events, messages")
        })?
        .to_string();
    let allowed: &[&str] = match kind.as_str() {
        "run_settled" | "run_gate" => &["kind", "run", "host"],
        "run_events" => &["kind", "run", "types", "host"],
        "messages" => &["kind", "root", "kinds", "host"],
        other => {
            return Err(format!(
                "{at}: unknown kind `{other}` (run_settled, run_gate, run_events, messages)"
            ))
        }
    };
    let what = format!("{at} (`{kind}`)");
    let mut o = entries(c, allowed, &what)?;
    match o.remove("host") {
        None => {}
        Some(Value::String(h)) if h == "local" => {}
        Some(Value::String(_)) => {
            return Err("wait on a remote host is not built yet (Plan 2)".into());
        }
        Some(_) => return Err(format!("{what}: `host` must be a string")),
    }
    let target_key = if kind == "messages" { "root" } else { "run" };
    let target = match o.remove(target_key) {
        Some(Value::String(s)) if !s.is_empty() => s,
        _ => return Err(format!("{what}: missing required argument `{target_key}`")),
    };
    if let Some(why) = crate::path_guard::unsafe_segment(&target) {
        return Err(format!("{what}: `{target}` is not a valid id ({why})"));
    }
    let kind = match kind.as_str() {
        "run_settled" => Kind::Settled,
        "run_gate" => Kind::Gate,
        "run_events" => Kind::Events {
            types: o
                .remove("types")
                .map(|v| {
                    let types = strings(v, "types", &what)?;
                    match types.iter().find(|t| !is_event_type(t)) {
                        Some(t) => Err(format!("{what}: unknown event type `{t}`")),
                        None => Ok(types),
                    }
                })
                .transpose()?,
        },
        _ => Kind::Messages {
            kinds: o
                .remove("kinds")
                .map(|v| {
                    strings(v, "kinds", &what)?
                        .into_iter()
                        .map(|k| {
                            serde_json::from_value::<MessageKind>(Value::String(k.clone()))
                                .map_err(|_| format!("{what}: unknown message kind `{k}`"))
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?,
        },
    };
    let source = match kind {
        Kind::Messages { .. } => format!("messages:{target}"),
        _ => format!("run:{target}"),
    };
    Ok(Condition {
        kind,
        target,
        source,
    })
}

/// Whether `t` is one of [`Event`]'s `type` tags: an unknown tag reads as
/// [`Event::Unknown`], a known one as its variant (or fails on its missing
/// fields).
fn is_event_type(t: &str) -> bool {
    !matches!(
        serde_json::from_value::<Event>(json!({ "type": t })),
        Ok(Event::Unknown)
    )
}

/// One line of a watched source.
enum Line {
    Run(NumberedLine<Event>),
    Msg(NumberedLine<LogLine>),
}

impl Line {
    fn id(&self) -> u64 {
        match self {
            Line::Run(l) => l.id,
            Line::Msg(l) => l.id,
        }
    }

    fn send(&self) -> bool {
        match self {
            Line::Run(l) => l.send,
            Line::Msg(l) => l.send,
        }
    }
}

type Tail = Pin<Box<dyn Stream<Item = (usize, Line)> + Send>>;

/// Where a source's lines come from.
enum Feed {
    Run(PathBuf),
    Log(PathBuf),
}

impl WaitSource {
    async fn wait(&self, req: Request) -> ToolResult {
        let store = Arc::clone(&self.state.run_store);

        // Every target exists; each source's feed.
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
                _ => {
                    let (s, id) = (Arc::clone(&store), c.target.clone());
                    match tokio::task::spawn_blocking(move || s.load(&id)).await {
                        Ok(Ok(_)) => Feed::Run(store.events_path(&c.target)),
                        Ok(Err(RunStoreError::NotFound(_))) => {
                            return ToolResult::Error(format!("no run `{}`", c.target))
                        }
                        Ok(Err(e)) => return ToolResult::Error(format!("run `{}`: {e}", c.target)),
                        Err(e) => return ToolResult::Error(format!("loading `{}`: {e}", c.target)),
                    }
                }
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
            cursor: req.cursor.clone(),
            ..Got::default()
        };

        // State first, then what is already on disk past the cursor: a run
        // seen settled has written its last line, so the catch-up after it
        // has every event. Either returns at once.
        self.check_state(&req.watch, &mut got.fired).await;
        if let Err(e) = catch_up(&req.watch, &feeds, &collects, &by_source, &mut got).await {
            return ToolResult::Error(e);
        }
        if got.any() {
            return got.finish(&req.watch);
        }

        let mut tails: Vec<Tail> = Vec::new();
        for (i, (source, feed)) in feeds.iter().enumerate() {
            let after = got.cursor.get(source).copied().unwrap_or(0);
            tails.push(match feed {
                Feed::Run(p) => {
                    Box::pin(NumberedTail::<Event>::open(p, after).map(move |l| (i, Line::Run(l))))
                }
                Feed::Log(p) => Box::pin(
                    NumberedTail::<LogLine>::open(p, after).map(move |l| (i, Line::Msg(l))),
                ),
            });
        }
        let mut lines = futures_util::stream::select_all(tails);

        let deadline = Instant::now() + req.timeout;
        let mut next_check = Instant::now() + RECHECK_EVERY;
        let mut last_check = Instant::now();
        loop {
            tokio::select! {
                line = lines.next() => {
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
                        self.check_state(&req.watch, &mut got.fired).await;
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
                        self.check_state(&req.watch, &mut got.fired).await;
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

    /// Record each state condition (`run_settled`, `run_gate`) that holds now
    /// in `fired`, with its detail.
    async fn check_state(&self, watch: &[Condition], fired: &mut BTreeMap<usize, Value>) {
        for (i, c) in watch.iter().enumerate() {
            if fired.contains_key(&i) {
                continue;
            }
            match c.kind {
                Kind::Settled => {
                    let store = Arc::clone(&self.state.run_store);
                    if crate::sse::run_is_settled(Arc::clone(&store), c.target.clone()).await {
                        let status = load(store, &c.target).await.map(|r| r.status);
                        fired.insert(i, json!({ "status": status }));
                    }
                }
                Kind::Gate => {
                    let store = Arc::clone(&self.state.run_store);
                    if let Some(rec) = load(store, &c.target).await {
                        if !rec.awaiting.is_empty() {
                            let gates: Vec<Value> = rec
                                .awaiting
                                .iter()
                                .map(|g| {
                                    json!({
                                        "step_id": g.step_id,
                                        "prompt": g.prompt,
                                        "expires_at": g.expires_at,
                                    })
                                })
                                .collect();
                            fired.insert(i, json!({ "gates": gates }));
                        }
                    }
                }
                Kind::Events { .. } | Kind::Messages { .. } => {}
            }
        }
    }
}

/// `run`'s record, off the runtime; `None` when it can't be read.
async fn load(store: Arc<RunStore>, run: &str) -> Option<rupu_orchestrator::runs::RunRecord> {
    let run = run.to_string();
    tokio::task::spawn_blocking(move || store.load(&run).ok())
        .await
        .ok()
        .flatten()
}

/// The line as JSON when a collecting condition among `conds` takes it,
/// counting it for each one that does.
fn matching(
    watch: &[Condition],
    conds: &[usize],
    line: &Line,
    counts: &mut BTreeMap<usize, u64>,
) -> Option<Value> {
    let event = match line {
        Line::Run(l) => match &l.event {
            Some(Event::Unknown) | None => return None,
            Some(ev) => serde_json::to_value(ev).ok()?,
        },
        Line::Msg(l) => match &l.event {
            Some(LogLine::Msg(_)) => serde_json::to_value(l.event.as_ref()?).ok()?,
            Some(LogLine::Seen { .. }) | None => return None,
        },
    };
    let mut taken = false;
    for &i in conds {
        let takes = match (&watch[i].kind, line) {
            (Kind::Events { types }, Line::Run(_)) => types.as_ref().is_none_or(|t| {
                event
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|ty| t.iter().any(|x| x == ty))
            }),
            (Kind::Messages { kinds }, Line::Msg(l)) => match &l.event {
                Some(LogLine::Msg(m)) => kinds.as_ref().is_none_or(|k| k.contains(&m.kind)),
                _ => false,
            },
            _ => false,
        };
        if takes {
            *counts.entry(i).or_default() += 1;
            taken = true;
        }
    }
    taken.then_some(event)
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

/// Read each collecting source past its cursor ([`read_numbered`], at most
/// [`CATCH_UP_MAX`] lines), keep what its conditions take and move its
/// cursor past what was read.
async fn catch_up(
    watch: &[Condition],
    feeds: &[(String, Feed)],
    collects: &[bool],
    by_source: &HashMap<&str, Vec<usize>>,
    got: &mut Got,
) -> Result<(), String> {
    for (i, (source, feed)) in feeds.iter().enumerate() {
        if !collects[i] {
            continue;
        }
        let after = got.cursor.get(source).copied().unwrap_or(0);
        let read = match feed {
            Feed::Run(p) => {
                let p = p.clone();
                tokio::task::spawn_blocking(move || {
                    read_numbered::<Event>(&p, after, CATCH_UP_MAX)
                        .map(|(l, last)| (l.into_iter().map(Line::Run).collect::<Vec<_>>(), last))
                })
                .await
            }
            Feed::Log(p) => {
                let p = p.clone();
                tokio::task::spawn_blocking(move || {
                    read_numbered::<LogLine>(&p, after, CATCH_UP_MAX)
                        .map(|(l, last)| (l.into_iter().map(Line::Msg).collect::<Vec<_>>(), last))
                })
                .await
            }
        };
        let (lines, last) = match read {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(format!("reading {source}: {e}")),
            Err(e) => return Err(format!("reading {source}: {e}")),
        };
        for line in &lines {
            got.take(watch, &by_source[source.as_str()], source, line);
        }
        if last > after {
            got.cursor.insert(source.clone(), last);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_condition_names_its_source() {
        let r = parse(json!({"watch": [
            {"kind": "run_gate", "run": "run_a"},
            {"kind": "messages", "root": "ses_b", "kinds": ["operator", "direct"]}
        ]}))
        .unwrap();
        assert_eq!(r.watch[0].source, "run:run_a");
        assert_eq!(r.watch[1].source, "messages:ses_b");
        assert_eq!(
            r.watch[1].kind,
            Kind::Messages {
                kinds: Some(vec![MessageKind::Operator, MessageKind::Direct])
            }
        );
        assert_eq!(r.timeout, DEFAULT_TIMEOUT);
    }

    #[test]
    fn the_schema_leaves_the_timeout_cap_to_the_server() {
        let schema = input_schema();
        let t = &schema["properties"]["timeout_ms"];
        assert_eq!(t["type"], "integer");
        assert!(t.get("maximum").is_none(), "{t}");
    }

    #[test]
    fn event_types_are_events_own_tags() {
        for t in [
            "run_started",
            "step_started",
            "run_completed",
            "attempt_resumed",
        ] {
            assert!(is_event_type(t), "{t}");
        }
        for t in ["step_warped", "unknown", "StepStarted", ""] {
            assert!(!is_event_type(t), "{t}");
        }
    }

    #[test]
    fn null_fields_are_absent() {
        let r = parse(json!({"watch": [{"kind": "run_events", "run": "run_a",
            "types": null, "host": null}], "cursor": null, "timeout_ms": null}))
        .unwrap();
        assert_eq!(r.watch[0].kind, Kind::Events { types: None });
        assert!(r.cursor.is_empty());
    }
}
