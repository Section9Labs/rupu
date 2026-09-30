//! `GET /api/usage/outliers` — runs that cost far more than their workflow
//! normally does.
//!
//! Baseline is PER WORKFLOW, not global. An absolute threshold would flag an
//! expensive-by-design workflow forever and never flag a cheap one that
//! regressed 10x — the opposite of useful. Standalone agent runs and session
//! turns are candidates too (spec 2026-09-29 §2), baselined per `(kind,
//! agent)` — a standalone run against that agent's other standalone runs, a
//! session turn against that agent's other turns — never mixed into a
//! workflow's baseline. Each transcript is counted once, attributed workflow
//! run > session > standalone (`crate::api::usage::local_sources`).
//!
//! Local-only for now: this endpoint does not fan out across hosts the way
//! `/api/usage` and `/api/dashboard` do. A remote host's runs are invisible
//! to it. Wiring in host fan-out is a follow-up (see `?host=` handling in
//! `crate::api::usage`) — flagged explicitly rather than silently omitted.

#![deny(clippy::all)]

use crate::usage_sources::SourceKind;
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Runs costing at least this many times their workflow's median are
/// flagged. 3x is a deliberately loose default — a genuine spike, not noise
/// from ordinary run-to-run variance.
const DEFAULT_THRESHOLD: f64 = 3.0;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/usage/outliers", get(get_usage_outliers))
}

#[derive(Debug, Deserialize)]
struct OutliersQuery {
    since: Option<String>,
    until: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RunCost {
    pub run_id: String,
    pub kind: SourceKind,
    /// `""` for a standalone agent run or session turn.
    pub workflow_name: String,
    /// The agent a standalone run / session turn ran; `""` for a workflow run.
    pub agent: String,
    /// `None` = unpriced. NOT zero — we do not know what it cost.
    pub cost_usd: Option<f64>,
    pub started_at: DateTime<Utc>,
}

impl RunCost {
    /// The population this run is baselined against: its workflow, or — for
    /// a standalone run / session turn — its agent within that kind.
    fn baseline_key(&self) -> (SourceKind, &str) {
        match self.kind {
            SourceKind::Workflow => (self.kind, self.workflow_name.as_str()),
            SourceKind::Agent | SourceKind::Session => (self.kind, self.agent.as_str()),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct OutlierRun {
    pub run_id: String,
    /// `"workflow"` | `"agent"` | `"session"` (additive).
    pub kind: &'static str,
    /// `""` for a standalone agent run or session turn.
    pub workflow_name: String,
    /// The baseline agent of a standalone run / session turn (additive;
    /// absent for a workflow run).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub cost_usd: f64,
    pub baseline_usd: f64,
    pub ratio: f64,
    pub started_at: DateTime<Utc>,
}

/// A workflow needs at least this many priced runs before it has a baseline.
/// Below it, every new workflow's first run would look anomalous.
const MIN_BASELINE_RUNS: usize = 3;

/// Median — robust to the very outliers we are hunting, unlike a mean, which
/// a single 100x spike drags upward until it stops flagging anything.
fn median(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = xs.len();
    if n == 0 {
        return 0.0;
    }
    if n % 2 == 1 {
        xs[n / 2]
    } else {
        (xs[n / 2 - 1] + xs[n / 2]) / 2.0
    }
}

/// Find runs costing more than `threshold`x their baseline population's
/// median (see [`RunCost::baseline_key`]).
pub fn find_outliers(runs: &[RunCost], threshold: f64) -> Vec<OutlierRun> {
    use std::collections::HashMap;

    let mut by_wf: HashMap<(SourceKind, &str), Vec<&RunCost>> = HashMap::new();
    for r in runs {
        // Unpriced runs contribute to neither the baseline nor the results:
        // None means unknown, and averaging unknown as 0 would drag every
        // baseline down and manufacture outliers.
        if r.cost_usd.is_some() {
            by_wf.entry(r.baseline_key()).or_default().push(r);
        }
    }

    let mut out = Vec::new();
    for wf_runs in by_wf.values() {
        if wf_runs.len() < MIN_BASELINE_RUNS {
            continue;
        }
        let baseline = median(wf_runs.iter().filter_map(|r| r.cost_usd).collect());
        if baseline <= 0.0 {
            continue;
        }
        for r in wf_runs {
            let cost = r.cost_usd.unwrap_or(0.0);
            let ratio = cost / baseline;
            if ratio >= threshold {
                out.push(OutlierRun {
                    run_id: r.run_id.clone(),
                    kind: r.kind.as_str(),
                    workflow_name: r.workflow_name.clone(),
                    agent: (r.kind != SourceKind::Workflow).then(|| r.agent.clone()),
                    cost_usd: cost,
                    baseline_usd: baseline,
                    ratio,
                    started_at: r.started_at,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        b.ratio
            .partial_cmp(&a.ratio)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// Build this host's `RunCost`s for every spend source started within
/// `[since, until]` — workflow runs, standalone agent runs and session turns
/// — from the SAME source set and rows `/api/usage`'s summary uses
/// (`crate::api::usage::local_sources`, priced by `crate::usage::summarize`)
/// — no second cost computation to drift out of sync with what `cost_usd`
/// means everywhere else in the CP.
///
/// The window itself is resolved via `crate::api::usage::resolve_window`
/// (Task W1) — reused, not re-derived, so this endpoint can't drift from
/// `/api/usage`'s defaulting (`since` absent → now − 30 days; `until` absent →
/// now) or its unparseable-bound → 400 behavior. NOTE: narrowing the window
/// legitimately changes which runs feed the per-workflow median baseline
/// below, so it can change which runs are flagged as outliers — that is
/// intended, not a bug.
async fn get_usage_outliers(
    State(s): State<AppState>,
    Query(q): Query<OutliersQuery>,
) -> ApiResult<Json<Vec<OutlierRun>>> {
    let (since, until) =
        crate::api::usage::resolve_window(q.since.as_deref(), q.until.as_deref(), Utc::now())
            .map_err(ApiError::bad_request)?;

    let (sources, _) = crate::api::usage::local_sources(&s, since, until, None).await?;
    let run_costs: Vec<RunCost> = sources
        .into_iter()
        .map(|src| RunCost {
            cost_usd: crate::usage::summarize(&src.rows, &s.pricing).cost_usd,
            run_id: src.id,
            kind: src.kind,
            workflow_name: src.workflow,
            agent: src.agent,
            started_at: src.started_at,
        })
        .collect();

    Ok(Json(find_outliers(&run_costs, DEFAULT_THRESHOLD)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_fixtures(runs: Vec<(&str, &str, f64)>) -> Vec<RunCost> {
        runs.into_iter()
            .map(|(workflow_name, run_id, cost)| RunCost {
                run_id: run_id.into(),
                kind: SourceKind::Workflow,
                workflow_name: workflow_name.into(),
                agent: String::new(),
                cost_usd: Some(cost),
                started_at: chrono::Utc::now(),
            })
            .collect()
    }

    fn standalone(kind: SourceKind, agent: &str, run_id: &str, cost: f64) -> RunCost {
        RunCost {
            run_id: run_id.into(),
            kind,
            workflow_name: String::new(),
            agent: agent.into(),
            cost_usd: Some(cost),
            started_at: chrono::Utc::now(),
        }
    }

    /// Standalone runs are baselined per agent, session turns per agent
    /// separately, and neither mixes with a workflow's population — even one
    /// whose name equals the agent's.
    #[test]
    fn standalone_and_session_runs_are_baselined_per_kind_and_agent() {
        let mut runs = to_fixtures(vec![
            ("solo", "w1", 10.0),
            ("solo", "w2", 10.0),
            ("solo", "w3", 10.0),
        ]);
        runs.extend([
            standalone(SourceKind::Agent, "solo", "a1", 1.0),
            standalone(SourceKind::Agent, "solo", "a2", 1.0),
            standalone(SourceKind::Agent, "solo", "a3", 1.0),
            standalone(SourceKind::Agent, "solo", "a_spike", 5.0),
            // Two session turns: no baseline of their own, and never
            // judged against the standalone runs.
            standalone(SourceKind::Session, "solo", "t1", 1.0),
            standalone(SourceKind::Session, "solo", "t2", 50.0),
        ]);
        let out = find_outliers(&runs, 3.0);
        let ids: Vec<_> = out.iter().map(|o| o.run_id.as_str()).collect();
        assert_eq!(ids, vec!["a_spike"]);
        assert_eq!(out[0].kind, "agent");
        assert_eq!(out[0].agent.as_deref(), Some("solo"));
        assert_eq!(out[0].workflow_name, "");
    }

    #[test]
    fn outlier_is_relative_to_its_own_workflow_baseline() {
        // A workflow that normally costs $1 spiking to $10 is an outlier. A
        // workflow that always costs $10 is not — an absolute threshold would
        // flag it forever.
        let runs = vec![
            ("cheap-wf", "r1", 1.0),
            ("cheap-wf", "r2", 1.0),
            ("cheap-wf", "r3", 1.0),
            ("cheap-wf", "spike", 10.0),
            ("pricey-wf", "p1", 10.0),
            ("pricey-wf", "p2", 10.0),
            ("pricey-wf", "p3", 10.0),
        ];
        let out = find_outliers(&to_fixtures(runs), 3.0);
        let ids: Vec<_> = out.iter().map(|o| o.run_id.as_str()).collect();
        assert_eq!(ids, vec!["spike"]);
    }

    #[test]
    fn a_workflow_with_too_few_runs_yields_no_outliers() {
        // One run is not a baseline. Flagging it would make every new workflow
        // look anomalous on its first run.
        let out = find_outliers(&to_fixtures(vec![("new-wf", "r1", 99.0)]), 3.0);
        assert!(out.is_empty());
    }

    #[test]
    fn unpriced_runs_are_not_outliers() {
        // cost_usd: None means "we don't know", not "free". It must not be
        // treated as 0 and it must not be flagged.
        let out = find_outliers(
            &[RunCost {
                run_id: "r1".into(),
                kind: SourceKind::Workflow,
                workflow_name: "wf".into(),
                agent: String::new(),
                cost_usd: None,
                started_at: chrono::Utc::now(),
            }],
            3.0,
        );
        assert!(out.is_empty());
    }
}
