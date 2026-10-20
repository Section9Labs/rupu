//! The `findings` condition: new records in the `findings.jsonl` ledgers of
//! the scoped workspaces. Polled with the state conditions rather than
//! tailed, because a ledger can appear (a new coverage target) at any time.
//!
//! Rows are built and filtered by the same code `GET /api/findings` uses
//! ([`finding_out`], [`join_workflow_names`], [`resolve_run_scope`],
//! [`query_findings`], [`list_row`]). A tag change on an older finding
//! appends no finding line, so it fires nothing.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use rupu_coverage::{CoveragePaths, FindingRecord};
use serde_json::Value;

use super::parse::{Condition, Kind};
use super::{Got, CATCH_UP_MAX};
use crate::api::findings::{
    finding_out, join_workflow_names, list_row, query_findings, resolve_run_scope, scm_web_hosts,
    store_for, FindingOut,
};
use crate::state::AppState;
use crate::transcript_tail::{count_numbered, read_numbered};

/// One ledger being read.
struct Src {
    ws_id: String,
    ws_path: PathBuf,
    target: String,
    path: PathBuf,
    /// The last line read.
    next: u64,
    /// Lines at or before this one were there when the call began; a
    /// condition without a `run` skips them.
    baseline: u64,
}

/// What a call remembers between its polls.
#[derive(Default)]
pub(super) struct Findings {
    sources: BTreeMap<String, Src>,
    /// Whether the first poll happened: a ledger found later is read from
    /// its first line.
    polled: bool,
}

/// Refuse a `project` that isn't a registered workspace.
pub(super) async fn validate(state: &AppState, watch: &[Condition]) -> Result<(), String> {
    let wanted: Vec<&String> = watch
        .iter()
        .filter_map(|c| match &c.kind {
            Kind::Findings {
                project: Some(p), ..
            } => Some(p),
            _ => None,
        })
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let global = state.global_dir.clone();
    let known: HashSet<String> = tokio::task::spawn_blocking(move || {
        store_for(&global)
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|w| w.id)
            .collect()
    })
    .await
    .map_err(|e| format!("listing projects: {e}"))?;
    match wanted.into_iter().find(|p| !known.contains(*p)) {
        Some(p) => Err(format!("no project `{p}`")),
        None => Ok(()),
    }
}

/// A new line a condition may take.
struct Fresh {
    source: String,
    line: u64,
    out: FindingOut,
}

/// Read what every `findings` condition's ledgers gained since the last poll
/// and add the findings it takes to `got` (events and counts).
pub(super) async fn poll(
    state: &AppState,
    watch: &[Condition],
    memo: &mut Findings,
    got: &mut Got,
) -> Result<(), String> {
    if !watch
        .iter()
        .any(|c| matches!(c.kind, Kind::Findings { .. }))
    {
        return Ok(());
    }
    let (global, store) = (state.global_dir.clone(), state.run_store.clone());
    let watch = watch.to_vec();
    let cursor = got.cursor.clone();
    let mut taken = std::mem::take(memo);
    let done = tokio::task::spawn_blocking(move || {
        let r = poll_blocking(&global, &store, &watch, &mut taken, &cursor);
        (taken, r)
    })
    .await
    .map_err(|e| format!("reading findings: {e}"))?;
    *memo = done.0;
    let (events, cursor, counts) = done.1?;
    got.events.extend(events);
    got.cursor.extend(cursor);
    for (i, n) in counts {
        *got.counts.entry(i).or_default() += n;
    }
    Ok(())
}

/// A `findings` condition: its index in the call, and its filters.
struct Cond<'a> {
    i: usize,
    project: Option<&'a str>,
    run: Option<&'a str>,
    q: Option<&'a str>,
}

type Polled = (Vec<Value>, BTreeMap<String, u64>, BTreeMap<usize, u64>);

fn poll_blocking(
    global: &std::path::Path,
    store: &rupu_orchestrator::runs::RunStore,
    watch: &[Condition],
    memo: &mut Findings,
    cursor_in: &BTreeMap<String, u64>,
) -> Result<Polled, String> {
    let conds: Vec<Cond> = watch
        .iter()
        .enumerate()
        .filter_map(|(i, c)| match &c.kind {
            Kind::Findings { project, run, q } => Some(Cond {
                i,
                project: project.as_deref(),
                run: run.as_deref(),
                q: q.as_deref(),
            }),
            _ => None,
        })
        .collect();
    let covers = |ws: &str| conds.iter().any(|c| c.project.is_none_or(|p| p == ws));
    let any_run = conds.iter().any(|c| c.run.is_some());
    let first = !memo.polled;
    memo.polled = true;
    let mut cursor_out: BTreeMap<String, u64> = BTreeMap::new();

    // Ledgers: the scoped workspaces' target dirs, listed afresh each poll.
    let workspaces = store_for(global).list().unwrap_or_default();
    for w in workspaces.iter().filter(|w| covers(&w.id)) {
        let wp = std::path::Path::new(&w.path);
        let Ok(targets) = rupu_coverage::discover_targets(wp) else {
            continue;
        };
        for t in targets {
            let source = format!("findings:{}/{}", w.id, t.target_id);
            if memo.sources.contains_key(&source) {
                continue;
            }
            let path = CoveragePaths::new(wp, &t.target_id).findings;
            let (next, baseline) = match cursor_in.get(&source) {
                Some(&n) => (n, 0),
                None if !first => (0, 0),
                None => {
                    let end = count_numbered(&path).unwrap_or(0);
                    (if any_run { 0 } else { end }, end)
                }
            };
            cursor_out.insert(source.clone(), next);
            memo.sources.insert(
                source,
                Src {
                    ws_id: w.id.clone(),
                    ws_path: wp.to_path_buf(),
                    target: t.target_id,
                    path,
                    next,
                    baseline,
                },
            );
        }
    }

    // New lines, with the workspace's tag log as it stands now.
    let mut fresh: Vec<Fresh> = Vec::new();
    let mut tag_events: HashMap<PathBuf, Vec<rupu_coverage::TagEvent>> = HashMap::new();
    let mut web_hosts = None;
    for (source, src) in memo.sources.iter_mut() {
        let (lines, last) = read_numbered::<FindingRecord>(&src.path, src.next, CATCH_UP_MAX)
            .map_err(|e| format!("reading {source}: {e}"))?;
        if last > src.next {
            src.next = last;
            cursor_out.insert(source.clone(), last);
        }
        let (ids, mut records): (Vec<u64>, Vec<FindingRecord>) = lines
            .into_iter()
            .filter_map(|l| l.event.map(|r| (l.id, r)))
            .unzip();
        if records.is_empty() {
            continue;
        }
        let events = tag_events.entry(src.ws_path.clone()).or_insert_with(|| {
            rupu_coverage::read_tag_events(&rupu_coverage::TagLog::for_workspace(&src.ws_path))
                .unwrap_or_default()
        });
        rupu_coverage::fold_tags(&mut records, events);
        let hosts = web_hosts.get_or_insert_with(|| scm_web_hosts(global));
        let ws = workspaces.iter().find(|w| w.id == src.ws_id);
        let Some(ws) = ws else { continue };
        for (line, record) in ids.into_iter().zip(records) {
            fresh.push(Fresh {
                source: source.clone(),
                line,
                out: finding_out(ws, &src.target, record, hosts),
            });
        }
    }
    if fresh.is_empty() {
        return Ok((vec![], cursor_out, BTreeMap::new()));
    }
    let mut outs: Vec<FindingOut> = fresh.iter().map(|f| f.out.clone()).collect();
    join_workflow_names(store, &mut outs);
    for (f, o) in fresh.iter_mut().zip(outs) {
        f.out = o;
    }

    // Each condition takes its share.
    let key = |f: &FindingOut| format!("{}/{}/{}", f.ws_id, f.target_id, f.record.id);
    let mut counts: BTreeMap<usize, u64> = BTreeMap::new();
    let mut taken: BTreeMap<(String, u64), Value> = BTreeMap::new();
    for Cond { i, project, run, q } in conds {
        let scope = run.map(|r| resolve_run_scope(global, store, r));
        let mut mine: Vec<&Fresh> = fresh
            .iter()
            .filter(|f| project.is_none_or(|p| p == f.out.ws_id))
            .filter(|f| {
                run.is_some()
                    || memo
                        .sources
                        .get(&f.source)
                        .is_none_or(|s| f.line > s.baseline)
            })
            .filter(|f| {
                scope
                    .as_ref()
                    .is_none_or(|s| s.contains(&f.out.record.declared_by.run_id))
            })
            .collect();
        if let Some(q) = q.filter(|q| !q.trim().is_empty()) {
            let parsed = rupu_coverage::parse_query(q).map_err(|e| e.message)?;
            let kept: HashSet<String> = query_findings(
                global,
                store,
                mine.iter().map(|f| f.out.clone()).collect(),
                &parsed,
            )
            .iter()
            .map(key)
            .collect();
            mine.retain(|f| kept.contains(&key(&f.out)));
        }
        for f in mine {
            *counts.entry(i).or_default() += 1;
            taken
                .entry((f.source.clone(), f.line))
                .or_insert_with(|| list_row(&f.out));
        }
    }
    let events = taken
        .into_iter()
        .map(|((source, line), event)| serde_json::json!({"source": source, "id": line, "event": event}))
        .collect();
    Ok((events, cursor_out, counts))
}
