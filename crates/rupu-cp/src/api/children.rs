//! `GET /api/runs/:id/children` (spec W7 §3.5): every child a run launched,
//! of every kind, read from the children ledger (`<runs>/<id>/children.jsonl`)
//! of the run and of each of its steps' agent runs, and down to their own
//! children. Runs recorded before the ledger existed still show their
//! sub-agents, read from the `sub/` directories they left (D20: no
//! migration), marked `legacy`.

use std::collections::{BTreeSet, VecDeque};
use std::path::Path;

use axum::extract::{Path as AxPath, State};
use axum::routing::get;
use axum::{Json, Router};
use rupu_launch::ledger::{self, ChildEntry};
use rupu_orchestrator::runs::RunStore;
use rupu_tools::{ChildState, ChildTokens};
use serde::Serialize;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/runs/:id/children", get(get_run_children))
}

/// Run directories walked, at most: a guard against a hand-edited tree, far
/// above any real launch tree (depth is capped at 5).
const MAX_RUNS_WALKED: usize = 2000;

/// One child, as the run detail shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChildDto {
    /// `None` for a legacy sub-agent (no ledger, no handle).
    pub handle: Option<String>,
    /// `subagent` | `agent` | `workflow`.
    pub kind: String,
    pub name: String,
    pub run_id: String,
    /// The run that launched it (a step's agent run, for a workflow).
    pub parent_run_id: String,
    pub depth: u32,
    pub codename: Option<String>,
    pub address: Option<String>,
    /// `pending` | `running` | `done` | `failed` | `abandoned` |
    /// `interrupted` (its parent paused or crashed; the run that continues
    /// the parent continues it).
    pub status: String,
    pub success: Option<bool>,
    pub error: Option<String>,
    pub tokens: Option<ChildTokens>,
    pub duration_ms: Option<u64>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub transcript_path: Option<String>,
    /// Read from a `sub/` directory, not the ledger.
    pub legacy: bool,
    /// The agentiflow lead round that launched it: a direct child's own,
    /// a deeper child's its root ancestor's. Absent when nothing recorded
    /// one (not launched by a lead, or a ledger from before rounds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub round: Option<u32>,
    /// A workflow child dispatched from a file its parent wrote with
    /// `workflows.generate`.
    pub generated: bool,
    /// The start of its answer ([`excerpt`]), when it recorded one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_excerpt: Option<String>,
}

/// The most of an answer [`excerpt`] keeps, in chars.
const EXCERPT_CHARS: usize = 240;

/// `output` without its surrounding whitespace, cut to [`EXCERPT_CHARS`]
/// chars (so always on a char boundary) and marked `…` when cut. `None` when
/// nothing is left.
pub(crate) fn excerpt(output: &str) -> Option<String> {
    let output = output.trim();
    if output.is_empty() {
        return None;
    }
    match output.char_indices().nth(EXCERPT_CHARS) {
        Some((cut, _)) => Some(format!("{}…", &output[..cut])),
        None => Some(output.to_string()),
    }
}

#[derive(Debug, Serialize)]
pub struct ChildrenResponse {
    pub children: Vec<ChildDto>,
}

async fn get_run_children(
    State(s): State<AppState>,
    AxPath(id): AxPath<String>,
) -> ApiResult<Json<ChildrenResponse>> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ApiError::bad_request(format!("`{id}` is not a run id")));
    }
    let store = s.run_store.clone();
    let children = tokio::task::spawn_blocking(move || children_of(&store, &id))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(ChildrenResponse { children }))
}

fn from_entry(e: ChildEntry, parent: &str) -> ChildDto {
    let state = e.state();
    let f = e.finished.as_ref();
    ChildDto {
        handle: Some(e.started.handle.clone()),
        kind: e.started.kind.as_str().into(),
        name: e.started.name.clone(),
        run_id: e.started.run_id.clone(),
        parent_run_id: parent.to_string(),
        depth: e.started.depth,
        codename: e.started.codename.clone(),
        address: e.started.address.clone(),
        status: state.as_str().into(),
        success: f.and_then(|f| f.success),
        error: f.and_then(|f| f.error.clone()),
        tokens: f.and_then(|f| f.tokens),
        duration_ms: f.and_then(|f| f.duration_ms),
        started_at: Some(e.started.at.to_rfc3339()),
        finished_at: f.map(|f| f.at.to_rfc3339()),
        transcript_path: e
            .started
            .transcript
            .as_ref()
            .map(|p| p.display().to_string()),
        legacy: false,
        round: e.started.round,
        generated: e.started.request.as_ref().is_some_and(|r| r.generated),
        output_excerpt: f.and_then(|f| f.output.as_deref()).and_then(excerpt),
    }
}

/// A sub-agent from before the ledger: its transcript says how it ended.
fn legacy_subagent(transcript: &Path, sub_run_id: &str, parent: &str, depth: u32) -> ChildDto {
    let summary = rupu_transcript::JsonlReader::summary(transcript).ok();
    let (status, success) = match summary.as_ref().map(|s| s.status) {
        Some(rupu_transcript::RunStatus::Ok) => (ChildState::Done, Some(true)),
        Some(rupu_transcript::RunStatus::Error) => (ChildState::Done, Some(false)),
        _ => (ChildState::Running, None),
    };
    ChildDto {
        handle: None,
        kind: "subagent".into(),
        name: summary
            .as_ref()
            .map(|s| s.agent.clone())
            .unwrap_or_default(),
        run_id: sub_run_id.to_string(),
        parent_run_id: parent.to_string(),
        depth,
        codename: summary.as_ref().and_then(|s| s.codename.clone()),
        address: None,
        status: status.as_str().into(),
        success,
        error: summary.as_ref().and_then(|s| s.error.clone()),
        tokens: None,
        duration_ms: summary.as_ref().map(|s| s.duration_ms),
        started_at: summary.as_ref().map(|s| s.started_at.to_rfc3339()),
        finished_at: None,
        transcript_path: Some(transcript.display().to_string()),
        legacy: true,
        round: None,
        generated: false,
        output_excerpt: None,
    }
}

/// Every child of `run_id` and of its steps' agent runs, and theirs, in
/// launch order per parent (parents breadth-first). A child an attempt took
/// on from an earlier one is listed once, as its latest record says.
pub fn children_of(store: &RunStore, run_id: &str) -> Vec<ChildDto> {
    let mut parents: VecDeque<Parent> = VecDeque::from([Parent::root(run_id)]);
    for rec in store.read_step_results(run_id).unwrap_or_default() {
        parents.push_back(Parent::root(&rec.run_id));
        for item in &rec.items {
            parents.push_back(Parent::root(&item.run_id));
        }
    }
    walk_children(store, parents)
}

/// A flow's direct children from its OWN `children.jsonl` ledger (W7),
/// re-parented to the flow `af_id`. An agentiflow is not a [`RunStore`] run,
/// so its ledger is read here rather than reached through [`children_of`].
pub fn ledger_units(af_dir: &Path, af_id: &str) -> Vec<ChildDto> {
    ledger::read(af_dir)
        .into_iter()
        .map(|e| from_entry(e, af_id))
        .collect()
}

/// `units` — a flow's direct children, already re-parented to it (from its
/// ledger via [`ledger_units`] and/or its legacy `units/` mirrors) — followed
/// by the whole launch tree under each one that is a local `rupu run` (its own
/// `children.jsonl` and legacy `sub/`), in the same shape. Reuses the same
/// breadth-first walk as [`children_of`]. A `units` entry is kept as given; the
/// caller dedups the direct level, the walk dedups the descendants.
pub fn with_descendants(store: &RunStore, units: Vec<ChildDto>) -> Vec<ChildDto> {
    let mut parents: VecDeque<Parent> = VecDeque::new();
    for u in &units {
        parents.push_back(Parent::below(u));
    }
    let mut out = units;
    out.extend(walk_children(store, parents));
    out
}

/// A run whose children [`walk_children`] reads.
struct Parent {
    run_id: String,
    /// The depth its legacy sub-agents are stamped one below.
    depth: u32,
    /// `None`: its children are the walk's direct children, each with its
    /// own round. `Some(round)`: they sit under a direct child, so they
    /// take its round.
    round: Option<Option<u32>>,
}

impl Parent {
    /// A run at the top of the walk.
    fn root(run_id: &str) -> Self {
        Self {
            run_id: run_id.to_string(),
            depth: 0,
            round: None,
        }
    }

    /// `child`, as the parent of what it launched.
    fn below(child: &ChildDto) -> Self {
        Self {
            run_id: child.run_id.clone(),
            depth: child.depth,
            round: Some(child.round),
        }
    }
}

/// Breadth-first over `parents`, collecting every child of every kind, in
/// launch order per parent (parents breadth-first). A child a later attempt
/// took on under the same handle is listed once, as its latest record says.
fn walk_children(store: &RunStore, mut parents: VecDeque<Parent>) -> Vec<ChildDto> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    // Each handle's latest record: where it is in `out`, and when it started.
    let mut latest: std::collections::HashMap<String, (usize, chrono::DateTime<chrono::Utc>)> =
        std::collections::HashMap::new();
    while let Some(Parent {
        run_id: parent,
        depth,
        round,
    }) = parents.pop_front()
    {
        if parent.is_empty() || !seen.insert(parent.clone()) || seen.len() > MAX_RUNS_WALKED {
            continue;
        }
        let ledgered = ledger::read(&store.root.join(&parent));
        let known: BTreeSet<String> = ledgered.iter().map(|e| e.started.run_id.clone()).collect();
        for e in ledgered {
            let at = e.started.at;
            let mut child = from_entry(e, &parent);
            if let Some(round) = round {
                child.round = round;
            }
            parents.push_back(Parent::below(&child));
            if let Some(handle) = child.handle.clone() {
                match latest.get(&handle) {
                    Some(&(_, seen)) if seen >= at => continue,
                    _ => {
                        latest.insert(handle, (out.len(), at));
                    }
                }
            }
            out.push(child);
        }
        for sub in store.sub_run_ids(&parent) {
            if known.contains(&sub) {
                continue;
            }
            let transcript = store.sub_run_transcript_path(&parent, &sub);
            let mut child = legacy_subagent(&transcript, &sub, &parent, depth + 1);
            child.round = round.flatten();
            parents.push_back(Parent::below(&child));
            out.push(child);
        }
    }
    let keep: BTreeSet<usize> = latest.values().map(|&(i, _)| i).collect();
    out.into_iter()
        .enumerate()
        .filter(|(i, c)| c.handle.is_none() || keep.contains(i))
        .map(|(_, c)| c)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_launch::ledger::{Finished, LedgerLine, Started};
    use rupu_tools::ChildKind;

    fn started(handle: &str, kind: ChildKind, run_id: &str, depth: u32) -> LedgerLine {
        LedgerLine::Started(Started {
            codename: Some("jade-reef/heron>lynx#1".into()),
            ..Started::new(handle, kind, "recon", run_id, 1, depth)
        })
    }

    /// A child a resumed step took on from its paused attempt is listed once,
    /// as the attempt that took it on records it, though both attempts are
    /// walked.
    #[test]
    fn a_child_taken_on_by_a_later_attempt_is_listed_once() {
        use rupu_launch::ledger::Continued;
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        std::fs::create_dir_all(store.root.join("run_wf")).unwrap();
        let rows: String = ["step_a1", "step_a2"]
            .iter()
            .map(|a| {
                format!(
                    "{{\"step_id\":\"scan\",\"run_id\":\"{a}\",\"transcript_path\":\"/t\",\"output\":\"\",\"success\":true,\"skipped\":false,\"rendered_prompt\":\"go\",\"finished_at\":\"2026-10-12T00:00:00Z\"}}\n"
                )
            })
            .collect();
        std::fs::write(store.root.join("run_wf").join("step_results.jsonl"), rows).unwrap();
        let (first, second) = (store.root.join("step_a1"), store.root.join("step_a2"));
        ledger::append(&first, &started("ch_1", ChildKind::Agent, "run_c1", 1));
        ledger::append(
            &first,
            &LedgerLine::Finished(Finished::new("ch_1", ChildState::Interrupted, None)),
        );
        ledger::append(
            &first,
            &LedgerLine::Continued(Continued {
                handle: "ch_1".into(),
                by_run: "step_a2".into(),
                at: chrono::Utc::now(),
            }),
        );
        ledger::append(&second, &started("ch_1", ChildKind::Agent, "run_c2", 1));
        ledger::append(
            &second,
            &LedgerLine::Finished(Finished::new("ch_1", ChildState::Done, None)),
        );

        let got: Vec<(String, String, String)> = children_of(&store, "run_wf")
            .into_iter()
            .map(|c| (c.handle.unwrap_or_default(), c.run_id, c.status))
            .collect();
        assert_eq!(
            got,
            [("ch_1".to_string(), "run_c2".to_string(), "done".to_string())]
        );
    }

    /// An agentiflow is not a `RunStore` run: its direct children (the units
    /// the lead launched) live in the flow directory's own `children.jsonl`,
    /// re-parented to the flow, and each unit that is a `rupu run` contributes
    /// its own launch tree below it.
    #[test]
    fn children_under_a_flow_are_its_units_and_their_descendants() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let af_dir = tmp.path().join("agentiflows").join("af_x");
        // The flow's own ledger: two units the lead dispatched.
        ledger::append(&af_dir, &started("u_recon", ChildKind::Agent, "run_recon", 1));
        ledger::append(&af_dir, &started("u_wf", ChildKind::Workflow, "run_wf", 1));
        // The recon unit is a real `rupu run` that dispatched its own
        // sub-agent, recorded under its own run directory.
        ledger::append(
            &store.root.join("run_recon"),
            &started("s_1", ChildKind::Subagent, "run_sub", 2),
        );

        let got = with_descendants(&store, ledger_units(&af_dir, "af_x"));
        let rows: Vec<(&str, &str, &str)> = got
            .iter()
            .map(|c| (c.run_id.as_str(), c.parent_run_id.as_str(), c.kind.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("run_recon", "af_x", "agent"),
                ("run_wf", "af_x", "workflow"),
                ("run_sub", "run_recon", "subagent"),
            ]
        );
    }

    #[test]
    fn a_runs_children_of_every_kind_and_their_own_children_and_legacy_sub_agents() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let root = store.root.join("run_root");
        ledger::append(&root, &started("ch_a", ChildKind::Agent, "run_child", 1));
        ledger::append(&root, &started("ch_w", ChildKind::Workflow, "run_wf", 1));
        ledger::append(
            &root,
            &LedgerLine::Finished(Finished {
                handle: "ch_a".into(),
                status: ChildState::Done,
                success: Some(true),
                output: Some("found it".into()),
                error: None,
                cause: None,
                tokens: Some(ChildTokens {
                    input: 3,
                    output: 4,
                }),
                duration_ms: Some(9),
                at: chrono::Utc::now(),
            }),
        );
        // The agent child's own sub-agent.
        ledger::append(
            &store.root.join("run_child"),
            &started("ch_s", ChildKind::Subagent, "sub_grand", 2),
        );
        // A sub-agent from before the ledger.
        let (legacy, _) = store.create_sub_run("run_root", "old").unwrap();
        // The workflow child, stopped by its parent's pause.
        ledger::append(
            &root,
            &LedgerLine::Finished(Finished {
                handle: "ch_w".into(),
                status: ChildState::Interrupted,
                success: None,
                output: None,
                error: Some("its parent run paused without joining it".into()),
                cause: None,
                tokens: None,
                duration_ms: None,
                at: chrono::Utc::now(),
            }),
        );

        let got = children_of(&store, "run_root");
        let rows: Vec<(&str, &str, &str, &str, bool)> = got
            .iter()
            .map(|c| {
                (
                    c.run_id.as_str(),
                    c.parent_run_id.as_str(),
                    c.kind.as_str(),
                    c.status.as_str(),
                    c.legacy,
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("run_child", "run_root", "agent", "done", false),
                ("run_wf", "run_root", "workflow", "interrupted", false),
                (legacy.as_str(), "run_root", "subagent", "running", true),
                ("sub_grand", "run_child", "subagent", "running", false),
            ]
        );
        assert_eq!(
            got[0].tokens,
            Some(ChildTokens {
                input: 3,
                output: 4
            })
        );
    }
}
