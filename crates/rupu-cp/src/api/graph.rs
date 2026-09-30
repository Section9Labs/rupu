//! Workflow → [`StepDag`] DTO mapper **and** the `GET /api/runs/:id/graph`
//! route that assembles the full run-graph response.
//!
//! The mapper half is a pure, sync, infallible transformation; the route
//! half does the I/O and error-mapping.

use crate::{
    api::run_resolve::{resolve_run_location, RunLocation},
    api::runs::{
        resolve_host, run_not_found_or_internal, synthesize_unpersisted_run, RunDetailQuery,
    },
    error::{ApiError, ApiResult},
    host::connector::HostConnectorError,
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use rupu_orchestrator::{
    executor::Event, runs::RunStore, workflow_edges, workflow_has_explicit_edges, Workflow,
};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader};

// ── Route ────────────────────────────────────────────────────────────────

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/runs/:id/graph", get(run_graph))
}

/// Proxy `GET /api/runs/:id/graph` to a resolved host. Shared by the
/// explicit `?host=` branch and the resolver's [`RunLocation::Host`] branch.
async fn run_graph_from_host(
    s: &AppState,
    host_id: &str,
    id: &str,
) -> ApiResult<serde_json::Value> {
    let conn = resolve_host(s, host_id)?;
    // SSH/Tunnel/Bucket runs are mirrored into our own RunStore, so the
    // graph is built from local artifacts — those transports have no
    // generic-GET surface to proxy to. Reaching for the wire here is what
    // used to 500 the whole run-detail page with "invalid: proxy_get_json
    // is not supported for ssh hosts".
    if conn.serves_runs_from_local_mirror() {
        return build_run_graph_json(&s.run_store, &s.pricing, id);
    }
    conn.proxy_get_json(&format!("/api/runs/{id}/graph"))
        .await
        .map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            HostConnectorError::Unreachable(m) => {
                ApiError::internal(format!("host {host_id} unreachable: {m}"))
            }
            other => ApiError::internal(other.to_string()),
        })
}

/// Build the full run-graph response (`{run, workflow, step_results, units,
/// usage}`) for a run in `store`. Shared by the `Global` and `ProjectLocal`
/// branches of `run_graph`.
fn build_run_graph_json(
    store: &RunStore,
    pricing: &rupu_config::PricingConfig,
    id: &str,
) -> ApiResult<serde_json::Value> {
    // 1. Verify the run exists (gives us the RunRecord too).
    let run = store
        .load(id)
        .map_err(|e| run_not_found_or_internal(id, e))?;

    // 2. Load the workflow YAML snapshot saved at run-start.
    let yaml = store
        .read_workflow_snapshot(id)
        .map_err(|e| ApiError::internal(e.to_string()))?;

    // 3. Parse the snapshot and build the DAG DTO.
    //
    // A bare agent run (`rupu run <agent>`) has no workflow, so its snapshot
    // is empty and `workflow_name` is `agent:<name>`. Parsing an empty
    // document as a `Workflow` fails with "missing field `name`", which used
    // to 500 the whole run-detail page (the frontend derives the run record
    // from this endpoint). Synthesize a single-node DAG instead so the page
    // renders. The `yaml.trim().is_empty()` arm is a defensive fallback for
    // any empty snapshot, not only the `agent:` prefix.
    let dag = if run.workflow_name.starts_with("agent:") || yaml.trim().is_empty() {
        agent_run_dag(&run.workflow_name)
    } else {
        let wf = Workflow::parse(&yaml).map_err(|e| ApiError::internal(e.to_string()))?;
        to_step_dag(&wf)
    };

    // 4. Step results and unit checkpoints — missing files = empty vecs.
    let step_results = store.read_step_results(id).unwrap_or_default();
    let checkpoints = store.read_unit_checkpoints(id).unwrap_or_default();

    // 5. Merge in units that exist only in the event stream.
    //
    // A panel step's panelist + fixer runs are emitted as `UnitStarted`
    // events carrying their `transcript_path`, but — unlike `for_each`
    // fan-out units — they are NOT persisted to `unit_checkpoints.jsonl`.
    // For a completed run the checkpoint file therefore has no panel units,
    // so their transcripts become unreachable on reload. Fold the
    // events-derived units into the response so the graph can surface them.
    //
    // Precedence: durable checkpoints WIN (they are the terminal record).
    // We only synthesize units for `(step_id, index)` pairs not already
    // present in the checkpoints.
    //
    // The same single pass over events.jsonl also folds the agent identities
    // (`AgentStarted` / `DispatchStarted`: codename, agent, provider, model),
    // so the client doesn't depend on its bounded live-event window to name
    // units — at 1000+ units that window drops the early `agent_started`s.
    let EventFold {
        units,
        step_identities,
        unit_identities,
        subrun_identities,
    } = merge_event_units(id, store, checkpoints);

    // 6. Token/cost rollup for the run-detail header breakdown.
    let usage = crate::usage::summarize_run(store, id, pricing);

    Ok(serde_json::json!({
        "run": run,
        "workflow": dag,
        "step_results": step_results,
        "units": units,
        "usage": usage,
        "step_identities": step_identities,
        "unit_identities": unit_identities,
        "subrun_identities": subrun_identities,
    }))
}

/// `GET /api/runs/:id/graph[?host=<id>]` — DAG + step statuses + unit list for
/// the given run.
///
/// An explicit `?host=<remote-id>` takes precedence over the resolver
/// (unchanged proxy behavior). Otherwise dispatches on
/// [`resolve_run_location`]: `Global`/`ProjectLocal` build the graph from the
/// resolved store; `Host` proxies; `Unpersisted` has no workflow snapshot to
/// parse, so it returns a single-node/failed graph (mirrors the existing
/// bare-agent-run fallback) so RunDetail still renders; `NotFound` → 404.
async fn run_graph(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RunDetailQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    if let Some(host_id) = q.host.as_deref().filter(|h| *h != "local") {
        return run_graph_from_host(&s, host_id, &id).await.map(Json);
    }

    match resolve_run_location(&s, &id).await {
        RunLocation::Global => build_run_graph_json(&s.run_store, &s.pricing, &id).map(Json),
        RunLocation::ProjectLocal { path } => {
            let store = RunStore::new(path.join(".rupu").join("runs"));
            build_run_graph_json(&store, &s.pricing, &id).map(Json)
        }
        RunLocation::Host { host_id } => run_graph_from_host(&s, &host_id, &id).await.map(Json),
        RunLocation::Unpersisted {
            cycle_id,
            status,
            failure,
            workflow_name,
            issue_ref,
            ..
        } => {
            let run = synthesize_unpersisted_run(
                &id,
                &cycle_id,
                status,
                &failure,
                &workflow_name,
                issue_ref.as_deref(),
            );
            let dag = unpersisted_run_dag(&workflow_name);
            Ok(Json(serde_json::json!({
                "run": run,
                "workflow": dag,
                "step_results": Vec::<serde_json::Value>::new(),
                "units": Vec::<serde_json::Value>::new(),
                "usage": crate::usage::UsageSummary::default(),
                "step_identities": serde_json::Map::new(),
                "unit_identities": serde_json::Map::new(),
                "subrun_identities": serde_json::Map::new(),
            })))
        }
        RunLocation::NotFound => Err(ApiError::not_found(format!("run {id} not found"))),
    }
}

/// Build the `units` response array: durable checkpoints first (these win),
/// then any units that exist only in `events.jsonl` (panel panelist/fixer
/// runs). Each element keeps the [`UnitCheckpoint`] field shape so the
/// frontend reads them uniformly.
/// An agent instance's identity as folded from `events.jsonl`. Absent fields
/// are omitted on the wire.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
struct Identity {
    #[serde(skip_serializing_if = "Option::is_none")]
    codename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
}

impl Identity {
    /// Overlay `later` field-by-field: a later event wins where it carries a
    /// value; an absent field keeps what an earlier event recorded.
    fn overlay(&mut self, later: Identity) {
        fn pick(slot: &mut Option<String>, v: Option<String>) {
            if let Some(v) = v.filter(|v| !v.is_empty()) {
                *slot = Some(v);
            }
        }
        pick(&mut self.codename, later.codename);
        pick(&mut self.agent, later.agent);
        pick(&mut self.provider, later.provider);
        pick(&mut self.model, later.model);
    }
}

/// Everything [`merge_event_units`] folds out of one pass over `events.jsonl`.
///
/// - `units`: checkpoints + events-only units, each carrying its
///   `AgentStarted` identity (`agent`/`provider`/`model`, and `codename`
///   when the unit record had none).
/// - `step_identities`: `{step_id: identity}` from step-level `AgentStarted`
///   (no `unit_index`).
/// - `unit_identities`: `{step_id: {unit_index: identity}}` from unit-level
///   `AgentStarted` — also covers `parallel:` sub-steps, which have no unit
///   record for the fold into `units` to land on.
/// - `subrun_identities`: `{sub_run_id: identity}` from `DispatchStarted`.
struct EventFold {
    units: Vec<serde_json::Value>,
    step_identities: BTreeMap<String, Identity>,
    unit_identities: BTreeMap<String, BTreeMap<usize, Identity>>,
    subrun_identities: BTreeMap<String, Identity>,
}

fn merge_event_units(
    id: &str,
    store: &RunStore,
    checkpoints: Vec<rupu_orchestrator::runs::UnitCheckpoint>,
) -> EventFold {
    // Track every (step_id, index) already covered — checkpoints first.
    let mut seen: HashSet<(String, usize)> = checkpoints
        .iter()
        .map(|c| (c.step_id.clone(), c.index))
        .collect();

    // Serialize the durable checkpoints (terminal records win).
    let mut out: Vec<serde_json::Value> = checkpoints
        .iter()
        .filter_map(|c| serde_json::to_value(c).ok())
        .collect();

    let mut step_identities: BTreeMap<String, Identity> = BTreeMap::new();
    let mut unit_identities: BTreeMap<String, BTreeMap<usize, Identity>> = BTreeMap::new();
    let mut subrun_identities: BTreeMap<String, Identity> = BTreeMap::new();

    // Read and parse the event stream; tolerate a missing/garbled file.
    let path = store.events_path(id);
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => {
            return EventFold {
                units: out,
                step_identities,
                unit_identities,
                subrun_identities,
            }
        }
    };

    // Synthesized (events-only) units, keyed by (step_id, index) so a later
    // `UnitCompleted` can patch the `success` flag of an earlier `UnitStarted`.
    // `order` preserves first-seen order for a stable response.
    let mut synthesized: std::collections::HashMap<(String, usize), usize> =
        std::collections::HashMap::new();
    let mut events_only: Vec<serde_json::Value> = Vec::new();

    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Event>(&line) else {
            continue;
        };
        match event {
            Event::UnitStarted {
                step_id,
                index,
                unit_key,
                transcript_path,
                codename,
                ..
            } => {
                let key = (step_id.clone(), index);
                if seen.contains(&key) {
                    continue; // checkpoint or earlier started already covers it
                }
                seen.insert(key.clone());
                synthesized.insert(key, events_only.len());
                let mut unit = serde_json::json!({
                    "step_id": step_id,
                    "index": index,
                    "item": unit_key,
                    "transcript_path": transcript_path.to_string_lossy(),
                    "success": serde_json::Value::Null,
                });
                if let Some(c) = codename {
                    unit["codename"] = serde_json::Value::String(c);
                }
                events_only.push(unit);
            }
            Event::UnitCompleted {
                step_id,
                index,
                success,
                ..
            } => {
                if let Some(&pos) = synthesized.get(&(step_id, index)) {
                    if let Some(obj) = events_only[pos].as_object_mut() {
                        obj.insert("success".into(), serde_json::Value::Bool(success));
                    }
                }
            }
            Event::AgentStarted {
                step_id,
                unit_index,
                codename,
                agent,
                provider,
                model,
                ..
            } => {
                let ident = Identity {
                    codename,
                    agent: Some(agent),
                    provider,
                    model,
                };
                let slot = match unit_index {
                    None => step_identities.entry(step_id).or_default(),
                    Some(i) => unit_identities
                        .entry(step_id)
                        .or_default()
                        .entry(i)
                        .or_default(),
                };
                slot.overlay(ident);
            }
            Event::DispatchStarted {
                sub_run_id,
                agent,
                codename,
                provider,
                model,
                ..
            } => {
                subrun_identities
                    .entry(sub_run_id)
                    .or_default()
                    .overlay(Identity {
                        codename,
                        agent,
                        provider,
                        model,
                    });
            }
            _ => {}
        }
    }

    out.extend(events_only);

    // Fold unit-level identities onto their unit records. The unit record's
    // own `codename` (checkpoint / unit_started) stays authoritative.
    for unit in &mut out {
        let Some(obj) = unit.as_object_mut() else {
            continue;
        };
        let (Some(step_id), Some(index)) = (
            obj.get("step_id").and_then(|v| v.as_str()),
            obj.get("index").and_then(|v| v.as_u64()),
        ) else {
            continue;
        };
        let Some(ident) = unit_identities
            .get(step_id)
            .and_then(|m| m.get(&(index as usize)))
        else {
            continue;
        };
        let has_codename = obj
            .get("codename")
            .and_then(|v| v.as_str())
            .is_some_and(|c| !c.is_empty());
        if !has_codename {
            if let Some(c) = &ident.codename {
                obj.insert("codename".into(), c.clone().into());
            }
        }
        for (k, v) in [
            ("agent", &ident.agent),
            ("provider", &ident.provider),
            ("model", &ident.model),
        ] {
            if let Some(v) = v {
                obj.insert(k.into(), v.clone().into());
            }
        }
    }

    EventFold {
        units: out,
        step_identities,
        unit_identities,
        subrun_identities,
    }
}

// ── DTOs ────────────────────────────────────────────────────────────────

/// Top-level response envelope for the step-DAG endpoint.
#[derive(Debug, Serialize)]
pub struct StepDag {
    pub steps: Vec<StepNodeDto>,
    /// The DAG's real control/data edges, so the run graph forks where the
    /// workflow forks (`split`/`join`/`branch`/`next`/`depends_on`) instead of
    /// fabricating a linear chain. Mirrors the web editor's `deriveEdges`:
    /// [`workflow_edges`] (explicit ∪ inferred data-ref) in graph mode, or the
    /// legacy consecutive-pair chain for an edge-free legacy workflow. Empty
    /// for a single-node agent/unpersisted run.
    pub edges: Vec<EdgeDto>,
}

/// One directed edge in the step DAG (`from` → `to`, both step ids).
#[derive(Debug, Serialize)]
pub struct EdgeDto {
    pub from: String,
    pub to: String,
}

/// One node in the step DAG.  The `kind` field drives how the UI renders
/// the node; optional fields are `None` when not relevant to the kind.
#[derive(Debug, Serialize)]
pub struct StepNodeDto {
    /// Matches the `id:` in the workflow YAML.
    pub id: String,
    /// `"step"` | `"for_each"` | `"parallel"` | `"panel"` | `"branch"` |
    /// `"split"` | `"join"` | `"action"` | `"run"` | `"gate"` — precedence:
    /// parallel > panel > branch > split > join > gate > run > action >
    /// for_each > step. `branch`/`split`/`join` are orchestration-only nodes
    /// whose fork lives in [`StepDag::edges`].
    pub kind: String,
    /// Agent name for linear / `for_each` steps.
    pub agent: Option<String>,
    /// The `for_each:` minijinja expression, when this is a fan-out step.
    pub for_each: Option<String>,
    /// Sub-steps, populated for `parallel` kind only.
    pub parallel: Option<Vec<SubStepDto>>,
    /// Panelist agent names, populated for `panel` kind only.
    pub panelists: Option<Vec<String>>,
    /// Gate configuration, populated when the panel step has a `gate:`.
    pub gate: Option<GateDto>,
    /// Connector action tool name (`action:` on the step), populated only
    /// for `kind == "action"`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub action: Option<String>,
    /// Approval-gate configuration, populated only for `kind == "gate"`
    /// (a standalone `approval:` gate NODE — see
    /// [`rupu_orchestrator::is_approval_gate`]). Distinct from [`GateDto`],
    /// which is the panel step's iteration-loop gate.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub approval_gate: Option<ApprovalGateDto>,
}

/// Mirrors [`rupu_orchestrator::SubStep`] — one branch inside a
/// `parallel:` block.
#[derive(Debug, Serialize)]
pub struct SubStepDto {
    pub id: String,
    /// Agent name.  `SubStep.agent` is a plain `String` (not `Option`).
    pub agent: String,
}

/// Mirrors [`rupu_orchestrator::PanelGate`].
#[derive(Debug, Serialize)]
pub struct GateDto {
    pub max_iterations: u32,
    /// Lowercase severity string, e.g. `"high"`.
    pub until_severity: String,
    /// Agent name of the fixer dispatched between panel iterations.
    pub fix_with: String,
}

/// Approval-gate configuration for a `kind == "gate"` node — mirrors the
/// relevant subset of [`rupu_orchestrator::Approval`]. Distinct from
/// [`GateDto`] (the panel step's iteration-loop gate); this is the
/// standalone `approval:` gate NODE (see
/// [`rupu_orchestrator::is_approval_gate`]).
#[derive(Debug, Serialize)]
pub struct ApprovalGateDto {
    /// Whether the gate has an `auto_approve:` expression configured (the
    /// expression itself isn't exposed — the UI only needs to know one
    /// exists).
    pub auto_approve: bool,
    /// Whether the gate has one or more `on_reject:` cleanup steps.
    pub has_on_reject: bool,
    pub timeout_seconds: Option<u64>,
}

// ── Mapper ───────────────────────────────────────────────────────────────

/// Synthesize a single-node [`StepDag`] for a bare agent run (no workflow).
///
/// An agent run's `workflow_name` is `agent:<name>`; there is no workflow
/// snapshot to parse. The node is a linear `step` carrying the agent name so
/// the run-detail graph shows the agent instead of failing to parse an empty
/// document.
pub fn agent_run_dag(workflow_name: &str) -> StepDag {
    let agent = workflow_name.strip_prefix("agent:").map(str::to_string);
    StepDag {
        steps: vec![StepNodeDto {
            id: "agent".to_string(),
            kind: "step".to_string(),
            agent,
            for_each: None,
            parallel: None,
            panelists: None,
            gate: None,
            action: None,
            approval_gate: None,
        }],
        edges: vec![],
    }
}

/// Synthesize a single-node [`StepDag`] for a [`RunLocation::Unpersisted`]
/// run — an autoflow dispatch that failed before/without ever writing a
/// workflow snapshot, so there is nothing to parse. Mirrors
/// [`agent_run_dag`]'s fallback shape: one `step` node, labeled with the
/// workflow name, so RunDetail still renders a graph instead of erroring.
pub fn unpersisted_run_dag(workflow_name: &str) -> StepDag {
    StepDag {
        steps: vec![StepNodeDto {
            id: "run".to_string(),
            kind: "step".to_string(),
            agent: Some(workflow_name.to_string()),
            for_each: None,
            parallel: None,
            panelists: None,
            gate: None,
            action: None,
            approval_gate: None,
        }],
        edges: vec![],
    }
}

/// Convert a parsed [`Workflow`] into a slim [`StepDag`] DTO.
///
/// The mapping is purely functional — no I/O, no fallibility.
pub fn to_step_dag(wf: &Workflow) -> StepDag {
    let steps = wf.steps.iter().map(map_step).collect();
    StepDag {
        steps,
        edges: render_edges(wf),
    }
}

/// The edges the run graph should render, matching the web editor's
/// `deriveEdges` so the two views agree:
///
/// * **Graph mode** (the workflow declares any explicit edge — `next` /
///   `split` / `join` / `depends_on`, or a loop): [`workflow_edges`], which is
///   the explicit control edges unioned with inferred `steps.X` data-ref edges
///   and `branch` then/else arms.
/// * **Legacy mode** (an edge-free workflow authored before non-linear
///   orchestration): the pre-existing consecutive-pair chain, unioned with the
///   same data-ref / branch-arm edges `workflow_edges` already produces — so an
///   old linear workflow still renders as the chain it always did.
fn render_edges(wf: &Workflow) -> Vec<EdgeDto> {
    // `workflow_edges` gives explicit ∪ data-ref ∪ branch-arm edges. In legacy
    // mode it therefore emits data-ref/branch edges but NO chain, so add the
    // consecutive-pair chain; the shared set dedups any overlap.
    let mut set: std::collections::BTreeSet<(String, String)> =
        workflow_edges(wf).into_iter().collect();
    if !workflow_has_explicit_edges(wf) {
        for pair in wf.steps.windows(2) {
            set.insert((pair[0].id.clone(), pair[1].id.clone()));
        }
    }
    set.into_iter()
        .map(|(from, to)| EdgeDto { from, to })
        .collect()
}

/// A [`StepNodeDto`] for an orchestration-only node (`split` / `join` /
/// `branch`): it carries no agent/action/for_each work of its own — its shape
/// in the graph is entirely its edges, so every optional is `None`.
fn orch_node(id: &str, kind: &str) -> StepNodeDto {
    StepNodeDto {
        id: id.to_string(),
        kind: kind.to_string(),
        agent: None,
        for_each: None,
        parallel: None,
        panelists: None,
        gate: None,
        action: None,
        approval_gate: None,
    }
}

fn map_step(step: &rupu_orchestrator::Step) -> StepNodeDto {
    // Kind precedence: parallel > panel > branch > split > join > gate >
    // run > action > for_each > step (mirrors the web editor). The
    // orchestration nodes (branch/split/join) carry no agent/action work; the
    // fork itself lives in the edges (see `render_edges`).
    if let Some(subs) = &step.parallel {
        let parallel = subs
            .iter()
            .map(|s| SubStepDto {
                id: s.id.clone(),
                agent: s.agent.clone(),
            })
            .collect();
        return StepNodeDto {
            id: step.id.clone(),
            kind: "parallel".to_string(),
            agent: None,
            for_each: None,
            parallel: Some(parallel),
            panelists: None,
            gate: None,
            action: None,
            approval_gate: None,
        };
    }

    if let Some(panel) = &step.panel {
        let gate = panel.gate.as_ref().map(|g| GateDto {
            max_iterations: g.max_iterations,
            until_severity: g
                .until_no_findings_at_severity_or_above
                .as_str()
                .to_string(),
            fix_with: g.fix_with.clone(),
        });
        return StepNodeDto {
            id: step.id.clone(),
            kind: "panel".to_string(),
            agent: None,
            for_each: None,
            parallel: None,
            panelists: Some(panel.panelists.clone()),
            gate,
            action: None,
            approval_gate: None,
        };
    }

    // Orchestration-only nodes: the bifurcation lives in the edges.
    if step.branch.is_some() {
        return orch_node(&step.id, "branch");
    }
    if step.split.is_some() {
        return orch_node(&step.id, "split");
    }
    if step.join.is_some() {
        return orch_node(&step.id, "join");
    }

    if rupu_orchestrator::is_approval_gate(step) {
        let approval_gate = step.approval.as_ref().map(|a| ApprovalGateDto {
            auto_approve: a.auto_approve.is_some(),
            has_on_reject: !a.on_reject.is_empty(),
            timeout_seconds: a.timeout_seconds,
        });
        return StepNodeDto {
            id: step.id.clone(),
            kind: "gate".to_string(),
            agent: None,
            for_each: None,
            parallel: None,
            panelists: None,
            gate: None,
            action: None,
            approval_gate,
        };
    }

    // Checked before `for_each`: a `for_each:` + `run:` step is a Run
    // node whose units fan out, matching the runner's StepKind precedence.
    if step.run.is_some() {
        return StepNodeDto {
            id: step.id.clone(),
            kind: "run".to_string(),
            agent: None,
            for_each: step.for_each.clone(),
            parallel: None,
            panelists: None,
            gate: None,
            action: step.run.as_ref().map(|r| r.cmd.clone()),
            approval_gate: None,
        };
    }

    if step.action.is_some() {
        return StepNodeDto {
            id: step.id.clone(),
            kind: "action".to_string(),
            agent: None,
            for_each: None,
            parallel: None,
            panelists: None,
            gate: None,
            action: step.action.clone(),
            approval_gate: None,
        };
    }

    if step.for_each.is_some() {
        return StepNodeDto {
            id: step.id.clone(),
            kind: "for_each".to_string(),
            agent: step.agent.clone(),
            for_each: step.for_each.clone(),
            parallel: None,
            panelists: None,
            gate: None,
            action: None,
            approval_gate: None,
        };
    }

    // Plain linear step.
    StepNodeDto {
        id: step.id.clone(),
        kind: "step".to_string(),
        agent: step.agent.clone(),
        for_each: None,
        parallel: None,
        panelists: None,
        gate: None,
        action: None,
        approval_gate: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_only_unit_carries_unit_started_codename() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        std::fs::create_dir_all(store.events_path("run_x").parent().unwrap()).unwrap();
        let ev = |codename: Option<&str>, index: usize| {
            serde_json::to_string(&Event::UnitStarted {
                run_id: "run_x".into(),
                step_id: "s".into(),
                index,
                unit_key: "a.rs".into(),
                agent: Some("heron".into()),
                transcript_path: "/t/u.jsonl".into(),
                host: None,
                codename: codename.map(str::to_string),
            })
            .unwrap()
        };
        std::fs::write(
            store.events_path("run_x"),
            format!("{}\n{}\n", ev(Some("jade-reef/hedgehog#1"), 0), ev(None, 1)),
        )
        .unwrap();
        let units = merge_event_units("run_x", &store, Vec::new()).units;
        assert_eq!(units[0]["codename"], "jade-reef/hedgehog#1");
        assert!(units[1].get("codename").is_none());
    }

    /// `AgentStarted` / `DispatchStarted` identities are folded server-side
    /// from the WHOLE events.jsonl — onto unit records by (step_id,
    /// unit_index), into `step_identities` (no unit_index), `unit_identities`
    /// and `subrun_identities` — so the client needn't hold every event.
    #[test]
    fn event_fold_collects_agent_and_dispatch_identities() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        std::fs::create_dir_all(store.events_path("run_x").parent().unwrap()).unwrap();
        let agent_started = |step: &str, unit: Option<usize>, codename: Option<&str>, provider: Option<&str>| {
            serde_json::to_string(&Event::AgentStarted {
                run_id: "run_x".into(),
                step_id: step.into(),
                unit_index: unit,
                codename: codename.map(str::to_string),
                agent: "sec-reviewer".into(),
                provider: provider.map(str::to_string),
                model: provider.map(|_| "claude-sonnet-4-6".to_string()),
                agent_run_id: "ar".into(),
                transcript_path: "/t/a.jsonl".into(),
            })
            .unwrap()
        };
        let unit_started = serde_json::to_string(&Event::UnitStarted {
            run_id: "run_x".into(),
            step_id: "fan".into(),
            index: 3,
            unit_key: "crates/db".into(),
            agent: Some("sec-reviewer".into()),
            transcript_path: "/t/u3.jsonl".into(),
            host: None,
            codename: None,
        })
        .unwrap();
        let dispatch = serde_json::to_string(&Event::DispatchStarted {
            run_id: "run_x".into(),
            sub_run_id: "sub_1".into(),
            agent: Some("helper".into()),
            transcript_path: "/t/s.jsonl".into(),
            codename: Some("jade-reef/heron>owl#1".into()),
            provider: Some("openai".into()),
            model: Some("gpt-5".into()),
        })
        .unwrap();
        let lines = [
            agent_started("lint", None, Some("jade-reef/heron"), Some("anthropic")),
            // a later step-level event without provider keeps the earlier one
            agent_started("lint", None, None, None),
            unit_started,
            agent_started("fan", Some(3), Some("jade-reef/lynx#3"), Some("anthropic")),
            // parallel sub-step: identity with no unit record
            agent_started("par", Some(1), Some("jade-reef/heron.b"), Some("anthropic")),
            dispatch,
        ];
        std::fs::write(store.events_path("run_x"), lines.join("\n") + "\n").unwrap();

        let fold = merge_event_units("run_x", &store, Vec::new());

        // unit record picks up agent/provider/model and (absent) codename
        let u = &fold.units[0];
        assert_eq!(u["step_id"], "fan");
        assert_eq!(u["codename"], "jade-reef/lynx#3");
        assert_eq!(u["agent"], "sec-reviewer");
        assert_eq!(u["provider"], "anthropic");
        assert_eq!(u["model"], "claude-sonnet-4-6");

        let step = &fold.step_identities["lint"];
        assert_eq!(step.codename.as_deref(), Some("jade-reef/heron"));
        assert_eq!(step.provider.as_deref(), Some("anthropic"));
        assert_eq!(step.model.as_deref(), Some("claude-sonnet-4-6"));

        let par = &fold.unit_identities["par"][&1];
        assert_eq!(par.codename.as_deref(), Some("jade-reef/heron.b"));

        let sub = &fold.subrun_identities["sub_1"];
        assert_eq!(sub.codename.as_deref(), Some("jade-reef/heron>owl#1"));
        assert_eq!(sub.agent.as_deref(), Some("helper"));
        assert_eq!(sub.provider.as_deref(), Some("openai"));
        assert_eq!(sub.model.as_deref(), Some("gpt-5"));

        // Wire shape: unit_identities keys are stringified indices.
        let v = serde_json::to_value(&fold.unit_identities).unwrap();
        assert_eq!(v["par"]["1"]["codename"], "jade-reef/heron.b");
    }

    #[test]
    fn unit_record_codename_beats_agent_started_codename() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        std::fs::create_dir_all(store.events_path("run_x").parent().unwrap()).unwrap();
        let started = serde_json::to_string(&Event::UnitStarted {
            run_id: "run_x".into(),
            step_id: "fan".into(),
            index: 0,
            unit_key: "a".into(),
            agent: None,
            transcript_path: "/t/u.jsonl".into(),
            host: None,
            codename: Some("jade-reef/lynx#0".into()),
        })
        .unwrap();
        let agent = serde_json::to_string(&Event::AgentStarted {
            run_id: "run_x".into(),
            step_id: "fan".into(),
            unit_index: Some(0),
            codename: Some("other/name".into()),
            agent: "a".into(),
            provider: None,
            model: None,
            agent_run_id: "ar".into(),
            transcript_path: "/t/a.jsonl".into(),
        })
        .unwrap();
        std::fs::write(store.events_path("run_x"), format!("{started}\n{agent}\n")).unwrap();
        let fold = merge_event_units("run_x", &store, Vec::new());
        assert_eq!(fold.units[0]["codename"], "jade-reef/lynx#0");
        assert_eq!(fold.units[0]["agent"], "a");
        assert!(fold.units[0].get("provider").is_none());
    }

    /// Build a `RunRecord` from JSON — optional fields fill via serde defaults,
    /// mirroring the on-disk `run.json` shape.
    fn run_record(id: &str, workflow_name: &str) -> rupu_orchestrator::runs::RunRecord {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "workflow_name": workflow_name,
            "status": "completed",
            "inputs": {},
            "workspace_id": "ws_1",
            "workspace_path": "/tmp/proj",
            "transcript_dir": "/tmp/proj/.rupu/transcripts",
            "started_at": "2026-06-30T21:07:19Z",
        }))
        .expect("run record from json")
    }

    /// Fake connector for a transport whose runs live in the coordinator's
    /// own mirror (SSH / Tunnel / Bucket). `proxy_get_json` panics: reaching
    /// for the wire here is the bug this test guards against.
    struct MirrorBackedConnector;

    #[async_trait::async_trait]
    impl crate::host::connector::HostConnector for MirrorBackedConnector {
        fn serves_runs_from_local_mirror(&self) -> bool {
            true
        }
        async fn info(&self) -> Result<crate::host::connector::HostInfo, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn launch_run(
            &self,
            _req: crate::launcher::LaunchRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn launch_agent(
            &self,
            _req: crate::agent_launcher::AgentLaunchRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn start_session(
            &self,
            _req: crate::session_starter::SessionStartRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn send_session_turn(
            &self,
            _req: crate::session_sender::SendMessageRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn list_runs(
            &self,
            _params: crate::host::connector::RunListQuery,
        ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn get_run(&self, _run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn approve_run(&self, _run_id: &str, _mode: &str) -> Result<(), HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn reject_run(
            &self,
            _run_id: &str,
            _reason: Option<&str>,
        ) -> Result<(), HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn cancel_run(&self, _run_id: &str) -> Result<(), HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn stream_run_events(
            &self,
            _run_id: &str,
        ) -> Result<crate::host::connector::EventByteStream, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn get_transcript(
            &self,
            _path: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn proxy_get_json(
            &self,
            path_and_query: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            panic!("run graph must read the local mirror, not proxy {path_and_query}");
        }
    }

    /// Register `host_id` as a host resolving to `conn`. A `Local` transport
    /// under a non-`"local"` id is the registry's injection seam — see the
    /// same trick in `api::runs`'s `get_run_host_proxies`.
    fn state_with_host(
        tmp: &tempfile::TempDir,
        host_id: &str,
        conn: std::sync::Arc<dyn crate::host::connector::HostConnector>,
    ) -> AppState {
        let host_store = rupu_workspace::HostStore {
            root: tmp.path().join("hosts"),
        };
        host_store
            .save(&rupu_workspace::Host {
                id: host_id.into(),
                name: host_id.into(),
                transport: rupu_workspace::HostTransport::Local,
                token_hash: None,
                created_at: chrono::Utc::now().to_rfc3339(),
                last_seen_at: None,
            })
            .unwrap();
        let registry =
            std::sync::Arc::new(crate::host::registry::HostRegistry::new(host_store, conn));
        AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
        .with_workspace_dir(tmp.path().to_path_buf())
        .with_hosts(registry)
    }

    /// Regression: `?host=<ssh id>` used to proxy a generic GET at a
    /// transport that has none, 500ing the whole run-detail page with
    /// "invalid: proxy_get_json is not supported for ssh hosts". The run's
    /// artifacts are already in our mirror — build from those.
    #[tokio::test]
    async fn run_graph_reads_the_mirror_for_a_transport_that_cannot_proxy() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = state_with_host(&tmp, "host_ssh", std::sync::Arc::new(MirrorBackedConnector));
        s.run_store
            .create(run_record("run_mirrored", "agent:ariadne"), "")
            .unwrap();

        let resp = run_graph(
            State(s),
            Path("run_mirrored".to_string()),
            Query(RunDetailQuery {
                host: Some("host_ssh".to_string()),
            }),
        )
        .await
        .expect("a mirrored run must render from the local store, not 500");

        assert_eq!(resp.0["run"]["id"], serde_json::json!("run_mirrored"));
    }

    #[test]
    fn map_step_gate_step_yields_gate_kind() {
        let step: rupu_orchestrator::Step = serde_json::from_value(serde_json::json!({
            "id": "approve",
            "approval": {
                "required": true,
                "auto_approve": "false",
                "timeout_seconds": 3600,
                "on_reject": [
                    {"id": "cleanup", "agent": "cleanup-agent", "prompt": "clean up"}
                ],
            },
        }))
        .expect("gate step from json");

        let dto = map_step(&step);
        assert_eq!(dto.kind, "gate");
        assert_eq!(dto.action, None);
        let gate = dto
            .approval_gate
            .expect("approval_gate populated for a gate node");
        assert!(gate.auto_approve);
        assert!(gate.has_on_reject);
        assert_eq!(gate.timeout_seconds, Some(3600));
    }

    #[test]
    fn map_step_action_step_yields_action_kind() {
        let step: rupu_orchestrator::Step = serde_json::from_value(serde_json::json!({
            "id": "create_pr",
            "action": "scm.prs.create",
        }))
        .expect("action step from json");

        let dto = map_step(&step);
        assert_eq!(dto.kind, "action");
        assert_eq!(dto.action.as_deref(), Some("scm.prs.create"));
        assert!(dto.approval_gate.is_none());
    }

    #[test]
    fn map_step_orchestration_nodes_yield_split_join_branch_kinds() {
        let split: rupu_orchestrator::Step =
            serde_json::from_value(serde_json::json!({ "id": "fork", "split": ["a", "b"] }))
                .expect("split step from json");
        assert_eq!(map_step(&split).kind, "split");

        let join: rupu_orchestrator::Step =
            serde_json::from_value(serde_json::json!({ "id": "j", "join": {} }))
                .expect("join step from json");
        assert_eq!(map_step(&join).kind, "join");

        let branch: rupu_orchestrator::Step = serde_json::from_value(serde_json::json!({
            "id": "br",
            "branch": { "condition": "{{ steps.x.success }}", "then": ["a"], "else": ["b"] },
        }))
        .expect("branch step from json");
        assert_eq!(map_step(&branch).kind, "branch");
    }

    #[test]
    fn render_edges_graph_mode_forks_on_split_and_rejoins() {
        // A split step must produce TWO diverging edges and the join TWO
        // converging ones — a real bifurcation, which is the whole fix. In
        // graph mode there is NO consecutive-pair chain: edges come only from
        // the declared topology.
        let wf = rupu_orchestrator::Workflow::parse(
            r#"
name: forky
steps:
  - id: start
    agent: a
    prompt: go
    next: [fork]
  - id: fork
    split: [left, right]
  - id: left
    agent: l
    prompt: go
    next: [join]
  - id: right
    agent: r
    prompt: go
    next: [join]
  - id: join
    join: {}
"#,
        )
        .expect("forky workflow parses");
        let dag = to_step_dag(&wf);
        let has = |from: &str, to: &str| dag.edges.iter().any(|e| e.from == from && e.to == to);
        assert!(has("start", "fork"));
        assert!(has("fork", "left"));
        assert!(has("fork", "right"));
        assert!(has("left", "join"));
        assert!(has("right", "join"));
        assert_eq!(
            dag.edges.iter().filter(|e| e.from == "fork").count(),
            2,
            "the split forks to exactly its two targets"
        );
        // The fork/join nodes report their orchestration kind.
        let kind = |id: &str| {
            dag.steps
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.kind.as_str())
        };
        assert_eq!(kind("fork"), Some("split"));
        assert_eq!(kind("join"), Some("join"));
    }

    #[test]
    fn render_edges_legacy_linear_workflow_is_the_consecutive_chain() {
        // An edge-free workflow (no next/split/join/depends_on) keeps the
        // pre-existing linear chain, so old linear workflows render unchanged.
        let wf = rupu_orchestrator::Workflow::parse(
            r#"
name: linear
steps:
  - id: a
    agent: x
    prompt: go
  - id: b
    agent: y
    prompt: go
  - id: c
    agent: z
    prompt: go
"#,
        )
        .expect("linear workflow parses");
        let pairs: Vec<(String, String)> = to_step_dag(&wf)
            .edges
            .into_iter()
            .map(|e| (e.from, e.to))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("a".to_string(), "b".to_string()),
                ("b".to_string(), "c".to_string()),
            ]
        );
    }

    #[test]
    fn agent_run_dag_extracts_agent_name() {
        let dag = agent_run_dag("agent:oracle-assessor-glm");
        assert_eq!(dag.steps.len(), 1);
        assert_eq!(dag.steps[0].kind, "step");
        assert_eq!(dag.steps[0].agent.as_deref(), Some("oracle-assessor-glm"));
    }

    /// Regression: a bare agent run has an empty workflow snapshot; the graph
    /// endpoint must NOT 500 with "missing field `name`" (it used to, which
    /// killed the whole run-detail page). It returns a single-node DAG.
    #[tokio::test]
    async fn run_graph_for_agent_run_returns_single_node_not_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
        .with_workspace_dir(tmp.path().to_path_buf());
        // Agent run: empty snapshot, exactly as `rupu run <agent>` persists.
        s.run_store
            .create(run_record("run_agent", "agent:oracle-assessor-glm"), "")
            .unwrap();

        let resp = run_graph(
            State(s),
            Path("run_agent".to_string()),
            Query(RunDetailQuery { host: None }),
        )
        .await
        .expect("agent-run graph must not error");

        let body = resp.0;
        let steps = body["workflow"]["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0]["kind"], "step");
        assert_eq!(steps[0]["agent"], "oracle-assessor-glm");
        assert_eq!(body["run"]["id"], "run_agent");
    }

    /// A real workflow run still parses its snapshot into the full DAG.
    #[tokio::test]
    async fn run_graph_for_workflow_run_parses_snapshot() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
        .with_workspace_dir(tmp.path().to_path_buf());
        s.run_store
            .create(
                run_record("run_wf", "my-flow"),
                "name: my-flow\nsteps:\n  - id: s1\n    agent: alpha\n    prompt: go\n",
            )
            .unwrap();

        let resp = run_graph(
            State(s),
            Path("run_wf".to_string()),
            Query(RunDetailQuery { host: None }),
        )
        .await
        .expect("workflow-run graph must not error");

        let steps = resp.0["workflow"]["steps"].as_array().unwrap().clone();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0]["id"], "s1");
        assert_eq!(steps[0]["agent"], "alpha");
    }
}
