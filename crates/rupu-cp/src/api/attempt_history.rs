//! Read-only recorded workflow attempts. No transcript reads or execution actions.
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use rupu_orchestrator::runs::AttemptRecord;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[path = "attempt_snapshot.rs"]
mod snapshot;

pub(crate) const CAPABILITY: &str = "workflow.attempt_history.v1";

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/runs/:id/attempts", get(attempts))
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
pub(super) struct Filters {
    pub step_id: Option<String>,
    pub unit_index: Option<usize>,
    pub sub_id: Option<String>,
}

impl Filters {
    pub fn matches(&self, row: &AttemptRecord) -> bool {
        self.step_id.as_ref().is_none_or(|v| v == &row.step_id)
            && self.unit_index.is_none_or(|v| Some(v) == row.unit_index)
            && self
                .sub_id
                .as_ref()
                .is_none_or(|v| Some(v) == row.sub_id.as_ref())
    }
}

#[derive(Default, Deserialize, schemars::JsonSchema)]
pub(crate) struct AttemptsQuery {
    /// Omit or use `local`; connect directly to the owning control plane.
    host: Option<String>,
    /// Exact step ID; when supplied, must contain 1..1024 bytes.
    step_id: Option<String>,
    /// Exact fan-out unit index.
    unit_index: Option<usize>,
    /// Exact parallel sub-step ID; when supplied, must contain 1..1024 bytes.
    sub_id: Option<String>,
    /// Page size, default 50; clamped to 1..200.
    limit: Option<usize>,
    /// Opaque continuation from this owner, run, and unchanged filters.
    cursor: Option<String>,
}

#[derive(Serialize)]
struct AttemptRow {
    ledger_line: usize,
    step_id: String,
    unit_index: Option<usize>,
    sub_id: Option<String>,
    agent_run_id: String,
    host: Option<String>,
    continued_from: Option<String>,
    transcript_path: PathBuf,
    started_at: chrono::DateTime<chrono::Utc>,
}

impl AttemptRow {
    fn recorded(ledger_line: usize, row: AttemptRecord) -> Self {
        Self {
            ledger_line,
            step_id: row.step_id,
            unit_index: row.unit_index,
            sub_id: row.sub_id,
            agent_run_id: row.agent_run_id,
            host: row.host,
            continued_from: row.continued_from,
            transcript_path: row.transcript_path,
            started_at: row.started_at,
        }
    }
}

#[derive(Clone, Serialize)]
struct Evidence {
    ledger: &'static str,
    parsing: &'static str,
    execution_history_complete: &'static str,
    malformed_rows: usize,
    unsupported_rows: usize,
    oversized_rows: usize,
    incomplete_tail_bytes: usize,
    snapshot_bytes: usize,
}

#[derive(Serialize)]
struct AttemptsResponse {
    schema_version: u8,
    workflow_run_id: String,
    owner_host_id: &'static str,
    snapshot: String,
    rows: Vec<AttemptRow>,
    next_cursor: Option<String>,
    recorded_matching_total: Option<usize>,
    evidence: Evidence,
}

async fn attempts(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<AttemptsQuery>,
) -> ApiResult<Json<AttemptsResponse>> {
    super::runs::validate_id(&id)?;
    if q.host.as_deref().is_some_and(|h| h != "local") {
        return Err(ApiError::not_available(
            "attempt history requires a direct connection to the owning host",
        ));
    }
    let filters = Filters {
        step_id: q.step_id,
        unit_index: q.unit_index,
        sub_id: q.sub_id,
    };
    if [&filters.step_id, &filters.sub_id]
        .into_iter()
        .flatten()
        .any(|s| s.is_empty() || s.len() > 1024)
    {
        return Err(ApiError::bad_request(
            "attempt filters must contain 1..1024 bytes",
        ));
    }
    if q.cursor.as_ref().is_some_and(|c| c.len() > 1024) {
        return Err(ApiError::bad_request("invalid attempt cursor"));
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    super::runs::blocking(move || {
        let store = super::run_resolve::local_recorded_store(&s, &id)?.ok_or_else(|| {
            ApiError::not_found(format!("recorded run {id} not found on local host"))
        })?;
        let record = store
            .load(&id)
            .map_err(|e| super::runs::run_not_found_or_internal(&id, e))?;
        if record.workflow_name.starts_with("agent:") {
            return Err(ApiError::bad_request(
                "attempt history requires a workflow run",
            ));
        }
        let owner = snapshot::Owner {
            global_dir: s
                .global_dir
                .canonicalize()
                .map_err(|e| ApiError::internal(e.to_string()))?,
            run_store: s.run_store.root.clone(),
        };
        snapshot::page(
            &owner,
            &store.root,
            &id,
            filters,
            limit,
            q.cursor.as_deref(),
        )
    })
    .await
    .map(Json)
}

#[cfg(test)]
#[path = "attempt_history_tests.rs"]
mod tests;
