use crate::{
    api::sessions::collect_sessions,
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{extract::State, routing::get, Json, Router};
use chrono::{DateTime, Utc};
use rupu_coverage::discover_targets;
use rupu_orchestrator::runs::RunStatus;
use rupu_workspace::worker_store::WorkerStore;
use rupu_workspace::WorkspaceStore;
use serde::Serialize;
use std::collections::HashMap;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/dashboard", get(get_dashboard))
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct RunsSummary {
    total: usize,
    by_status: HashMap<&'static str, usize>,
}

#[derive(Serialize)]
struct RecentRun {
    id: String,
    workflow_name: String,
    status: &'static str,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    usage: crate::usage::UsageSummary,
}

#[derive(Serialize)]
struct SessionsSummary {
    total: usize,
    active: usize,
    archived: usize,
}

#[derive(Serialize)]
struct WorkersSummary {
    total: usize,
}

#[derive(Serialize)]
struct CoverageSummary {
    targets: usize,
    assertions: usize,
}

#[derive(Serialize)]
struct DashboardResponse {
    runs: RunsSummary,
    recent_runs: Vec<RecentRun>,
    sessions: SessionsSummary,
    workers: WorkersSummary,
    coverage: CoverageSummary,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

async fn get_dashboard(State(s): State<AppState>) -> ApiResult<Json<DashboardResponse>> {
    // --- runs ----------------------------------------------------------------
    let all_runs = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?;

    // All six RunStatus variants must always be present (even when 0).
    let statuses: &[RunStatus] = &[
        RunStatus::Pending,
        RunStatus::Running,
        RunStatus::Completed,
        RunStatus::Failed,
        RunStatus::AwaitingApproval,
        RunStatus::Rejected,
    ];
    let mut by_status: HashMap<&'static str, usize> = statuses
        .iter()
        .map(|s| (s.as_str(), 0_usize))
        .collect();
    for run in &all_runs {
        *by_status.entry(run.status.as_str()).or_insert(0) += 1;
    }

    let runs_summary = RunsSummary {
        total: all_runs.len(),
        by_status,
    };

    // --- recent_runs (top 10 sorted descending by started_at) ---------------
    // Defensive sort so this is correct regardless of RunStore::list() ordering.
    let mut runs_sorted = all_runs.iter().collect::<Vec<_>>();
    runs_sorted.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    let recent_runs: Vec<RecentRun> = runs_sorted
        .into_iter()
        .take(10)
        .map(|r| RecentRun {
            id: r.id.clone(),
            workflow_name: r.workflow_name.clone(),
            status: r.status.as_str(),
            started_at: r.started_at,
            finished_at: r.finished_at,
            usage: crate::usage::summarize_run(&s.run_store, &r.id, &s.pricing),
        })
        .collect();

    // --- sessions ------------------------------------------------------------
    let sessions = collect_sessions(&s.global_dir, &s.pricing);
    let active = sessions
        .iter()
        .filter(|v| v.get("scope").and_then(|s| s.as_str()) == Some("active"))
        .count();
    let archived = sessions
        .iter()
        .filter(|v| v.get("scope").and_then(|s| s.as_str()) == Some("archived"))
        .count();
    let sessions_summary = SessionsSummary {
        total: sessions.len(),
        active,
        archived,
    };

    // --- workers -------------------------------------------------------------
    let worker_store = WorkerStore {
        root: s.global_dir.join("autoflows").join("workers"),
    };
    let worker_count = worker_store
        .list()
        .map(|w| w.len())
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "dashboard: failed to list workers; using 0");
            0
        });

    // --- coverage ------------------------------------------------------------
    // Coverage lives per-PROJECT under each registered workspace's
    // `<path>/.rupu/coverage/`. Aggregate target/assertion counts across the
    // registry (cheap: discover only, no audit). A missing registry → zeros.
    let workspace_store = WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    };
    let mut cov_targets = 0usize;
    let mut cov_assertions = 0usize;
    for w in workspace_store.list().unwrap_or_default() {
        match discover_targets(std::path::Path::new(&w.path)) {
            Ok(targets) => {
                cov_assertions += targets.iter().map(|t| t.assertion_lines).sum::<usize>();
                cov_targets += targets.len();
            }
            Err(e) => {
                tracing::warn!(ws_id = %w.id, error = %e, "dashboard: failed to discover coverage targets; skipping workspace");
            }
        }
    }

    Ok(Json(DashboardResponse {
        runs: runs_summary,
        recent_runs,
        sessions: sessions_summary,
        workers: WorkersSummary {
            total: worker_count,
        },
        coverage: CoverageSummary {
            targets: cov_targets,
            assertions: cov_assertions,
        },
    }))
}
