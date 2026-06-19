//! Workflow → [`StepDag`] DTO mapper **and** the `GET /api/runs/:id/graph`
//! route that assembles the full run-graph response.
//!
//! The mapper half is a pure, sync, infallible transformation; the route
//! half does the I/O and error-mapping.

use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use rupu_orchestrator::{executor::Event, RunStoreError, Workflow};
use serde::Serialize;
use std::collections::HashSet;
use std::io::{BufRead, BufReader};

// ── Route ────────────────────────────────────────────────────────────────

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/runs/:id/graph", get(run_graph))
}

async fn run_graph(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // 1. Verify the run exists (gives us the RunRecord too).
    let run = s.run_store.load(&id).map_err(|e| match e {
        RunStoreError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
        other => ApiError::internal(other.to_string()),
    })?;

    // 2. Load the workflow YAML snapshot saved at run-start.
    let yaml = s
        .run_store
        .read_workflow_snapshot(&id)
        .map_err(|e| ApiError::internal(e.to_string()))?;

    // 3. Parse the snapshot and build the DAG DTO.
    let wf = Workflow::parse(&yaml).map_err(|e| ApiError::internal(e.to_string()))?;
    let dag = to_step_dag(&wf);

    // 4. Step results and unit checkpoints — missing files = empty vecs.
    let step_results = s.run_store.read_step_results(&id).unwrap_or_default();
    let checkpoints = s.run_store.read_unit_checkpoints(&id).unwrap_or_default();

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
    let units = merge_event_units(&id, &s, checkpoints);

    Ok(Json(serde_json::json!({
        "run": run,
        "workflow": dag,
        "step_results": step_results,
        "units": units,
    })))
}

/// Build the `units` response array: durable checkpoints first (these win),
/// then any units that exist only in `events.jsonl` (panel panelist/fixer
/// runs). Each element keeps the [`UnitCheckpoint`] field shape so the
/// frontend reads them uniformly.
fn merge_event_units(
    id: &str,
    s: &AppState,
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
    let path = s.run_store.events_path(id);
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
