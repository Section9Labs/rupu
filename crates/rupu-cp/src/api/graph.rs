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
use rupu_orchestrator::{executor::Event, runs::RunStore, Workflow};
use serde::Serialize;
use std::collections::HashSet;
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
    let units = merge_event_units(id, store, checkpoints);

    // 6. Token/cost rollup for the run-detail header breakdown.
    let usage = crate::usage::summarize_run(store, id, pricing);

    Ok(serde_json::json!({
        "run": run,
        "workflow": dag,
        "step_results": step_results,
        "units": units,
        "usage": usage,
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
            entity,
        } => {
            let run = synthesize_unpersisted_run(
                &id,
                &cycle_id,
                status,
                &failure,
                &workflow_name,
                entity.as_deref(),
            );
            let dag = unpersisted_run_dag(&workflow_name);
            Ok(Json(serde_json::json!({
                "run": run,
                "workflow": dag,
                "step_results": Vec::<serde_json::Value>::new(),
                "units": Vec::<serde_json::Value>::new(),
                "usage": crate::usage::UsageSummary::default(),
            })))
        }
        RunLocation::NotFound => Err(ApiError::not_found(format!("run {id} not found"))),
    }
}

/// Build the `units` response array: durable checkpoints first (these win),
/// then any units that exist only in `events.jsonl` (panel panelist/fixer
/// runs). Each element keeps the [`UnitCheckpoint`] field shape so the
/// frontend reads them uniformly.
fn merge_event_units(
    id: &str,
    store: &RunStore,
    checkpoints: Vec<rupu_orchestrator::runs::UnitCheckpoint>,
) -> Vec<serde_json::Value> {
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

    // Read and parse the event stream; tolerate a missing/garbled file.
    let path = store.events_path(id);
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return out,
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
                ..
            } => {
                let key = (step_id.clone(), index);
                if seen.contains(&key) {
                    continue; // checkpoint or earlier started already covers it
                }
                seen.insert(key.clone());
                synthesized.insert(key, events_only.len());
                events_only.push(serde_json::json!({
                    "step_id": step_id,
                    "index": index,
                    "item": unit_key,
                    "transcript_path": transcript_path.to_string_lossy(),
                    "success": serde_json::Value::Null,
                }));
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
            _ => {}
        }
    }

    out.extend(events_only);
    out
}

// ── DTOs ────────────────────────────────────────────────────────────────

/// Top-level response envelope for the step-DAG endpoint.
#[derive(Debug, Serialize)]
pub struct StepDag {
    pub steps: Vec<StepNodeDto>,
}

/// One node in the step DAG.  The `kind` field drives how the UI renders
/// the node; optional fields are `None` when not relevant to the kind.
#[derive(Debug, Serialize)]
pub struct StepNodeDto {
    /// Matches the `id:` in the workflow YAML.
    pub id: String,
    /// `"step"` | `"for_each"` | `"parallel"` | `"panel"` — precedence:
    /// parallel > panel > for_each > step.
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
        }],
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
        }],
    }
}

/// Convert a parsed [`Workflow`] into a slim [`StepDag`] DTO.
///
/// The mapping is purely functional — no I/O, no fallibility.
pub fn to_step_dag(wf: &Workflow) -> StepDag {
    let steps = wf.steps.iter().map(map_step).collect();
    StepDag { steps }
}

fn map_step(step: &rupu_orchestrator::Step) -> StepNodeDto {
    // Kind precedence: parallel > panel > for_each > step
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
