//! Pure mapper: [`rupu_orchestrator::Workflow`] → [`StepDag`] DTO.
//!
//! Used by `GET /api/runs/:id/graph` (Task 2) to send a slim, serialisable
//! representation of the workflow's step graph to the control-plane UI.
//! This module has zero I/O or async concerns — it is a deterministic
//! transformation of an already-parsed [`Workflow`].

use rupu_orchestrator::Workflow;
use serde::Serialize;

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
            until_severity: g.until_no_findings_at_severity_or_above.as_str().to_string(),
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
