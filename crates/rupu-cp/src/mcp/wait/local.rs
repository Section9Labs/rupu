//! The local sources of `wait`: run event logs, message logs, and the run
//! state conditions read from a run store.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use futures_util::Stream;
use rupu_fleet::LogLine;
use rupu_orchestrator::executor::Event;
use rupu_orchestrator::runs::{RunStore, RunStoreError};
use serde_json::{json, Value};

use super::parse::{Condition, Kind};
use super::{Got, WaitSource, CATCH_UP_MAX};
use crate::api::run_resolve::{resolve_run_location, RunLocation};
use crate::state::AppState;
use crate::transcript_tail::{read_numbered, NumberedLine};

/// One line of a watched source.
pub(super) enum Line {
    Run(NumberedLine<Event>),
    Msg(NumberedLine<LogLine>),
}

impl Line {
    pub(super) fn id(&self) -> u64 {
        match self {
            Line::Run(l) => l.id,
            Line::Msg(l) => l.id,
        }
    }

    pub(super) fn send(&self) -> bool {
        match self {
            Line::Run(l) => l.send,
            Line::Msg(l) => l.send,
        }
    }
}

pub(super) type Tail = Pin<Box<dyn Stream<Item = (usize, Line)> + Send>>;

/// Where a source's lines come from.
pub(super) enum Feed {
    Run(PathBuf),
    Log(PathBuf),
}

impl Feed {
    pub(super) fn path(&self) -> &std::path::Path {
        match self {
            Feed::Run(p) | Feed::Log(p) => p,
        }
    }
}

/// Where a run condition's run lives, and so which store answers for it.
pub(super) enum Located {
    Store(Arc<RunStore>),
    /// The autoflow history knows the run, but it never persisted anything.
    Unpersisted,
}

/// The run each run condition names, resolved through
/// [`resolve_run_location`] (a project-local run answers like a global one).
/// An error names the run that has none.
pub(super) async fn locate(
    state: &AppState,
    watch: &[Condition],
) -> Result<HashMap<String, Located>, String> {
    let mut found: HashMap<String, Located> = HashMap::new();
    for c in watch {
        if matches!(c.kind, Kind::Messages { .. }) {
            continue;
        }
        let run = c.target.as_str();
        let unpersisted_ok = matches!(c.kind, Kind::Settled);
        let located = match resolve_run_location(state, run).await {
            RunLocation::Global => Located::Store(Arc::clone(&state.run_store)),
            RunLocation::ProjectLocal { path } => {
                Located::Store(Arc::new(RunStore::new(path.join(".rupu").join("runs"))))
            }
            RunLocation::Host { host_id } => {
                return Err(format!(
                "run `{run}` lives on host `{host_id}`: wait on a remote host is not supported yet"
            ))
            }
            RunLocation::Unpersisted { .. } if unpersisted_ok => Located::Unpersisted,
            RunLocation::Unpersisted { .. } => {
                return Err(format!(
                    "run `{run}` never persisted a record, so it has no {} to watch",
                    if matches!(c.kind, Kind::Gate) {
                        "gate"
                    } else {
                        "event log"
                    }
                ))
            }
            RunLocation::NotFound => return Err(format!("no run `{run}`")),
        };
        if let Located::Store(s) = &located {
            let (s, id) = (Arc::clone(s), run.to_string());
            match tokio::task::spawn_blocking(move || s.load(&id)).await {
                Ok(Ok(_)) => {}
                Ok(Err(RunStoreError::NotFound(_))) => return Err(format!("no run `{run}`")),
                Ok(Err(e)) => return Err(format!("run `{run}`: {e}")),
                Err(e) => return Err(format!("loading `{run}`: {e}")),
            }
        }
        // An `Unpersisted` answer must not hide a store a later condition
        // on the same run needs, nor the reverse: the first one stands.
        found.entry(c.target.clone()).or_insert(located);
    }
    Ok(found)
}

impl WaitSource {
    /// Record each state condition (`run_settled`, `run_gate`) that holds now
    /// in `fired`, with its detail.
    pub(super) async fn check_state(
        &self,
        watch: &[Condition],
        located: &HashMap<String, Located>,
        fired: &mut BTreeMap<usize, Value>,
    ) {
        for (i, c) in watch.iter().enumerate() {
            if fired.contains_key(&i) {
                continue;
            }
            let Some(at) = located.get(&c.target) else {
                continue;
            };
            match (&c.kind, at) {
                (Kind::Settled, Located::Unpersisted) => {
                    fired.insert(i, json!({ "status": "failed", "unpersisted": true }));
                }
                (Kind::Settled, Located::Store(store)) => {
                    if crate::sse::run_is_settled(Arc::clone(store), c.target.clone()).await {
                        let status = load(Arc::clone(store), &c.target).await.map(|r| r.status);
                        fired.insert(i, json!({ "status": status }));
                    }
                }
                (Kind::Gate, Located::Store(store)) => {
                    if let Some(rec) = load(Arc::clone(store), &c.target).await {
                        // `awaiting_gates`: a legacy record names its one
                        // gate in `awaiting_step_id` only.
                        let awaiting = rec.awaiting_gates();
                        if !awaiting.is_empty() {
                            let gates: Vec<Value> = awaiting
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
                _ => {}
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
pub(super) fn matching(
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

/// Read each collecting source past its cursor ([`read_numbered`], at most
/// [`CATCH_UP_MAX`] lines), keep what its conditions take and move its
/// cursor past what was read.
pub(super) async fn catch_up(
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
