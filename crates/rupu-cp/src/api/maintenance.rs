//! Housekeeping: `POST /api/maintenance/prune` — the CLI's `transcript
//! prune` (`rupu_orchestrator::archived_transcripts`), `netflow prune`
//! ([`crate::netflow_prune`]), and `session prune` / `cleanup` (through the
//! `AdminOps` port while session records live in the CLI).
//!
//! Local-only: it deletes from this control plane's own `RUPU_HOME`, with
//! the project layer of the directory `cp serve` started in — the same
//! layers the CLI commands read from there.

use crate::{
    admin_ops::{self, HostQuery, PruneKind, PruneRequest},
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Query, State},
    routing::post,
    Json, Router,
};
use rupu_orchestrator::archived_transcripts::{self, PruneError};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/maintenance/prune", post(prune))
}

/// `POST /api/maintenance/prune` body.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct PruneBody {
    /// What to prune: `transcripts` (archived standalone transcripts),
    /// `sessions` (archived sessions), `netflow` (netflow ledgers) or
    /// `cleanup` (archived sessions and transcripts together).
    kind: PruneKind,
    /// Retention cutoff such as `30d`, `12h` or `1w`; absent = the
    /// command's default (`[storage]` retention, netflow's `30d`).
    #[serde(default)]
    older_than: Option<String>,
    /// Report what would be removed; remove nothing. Defaults to `true`:
    /// pass `false` to delete.
    #[serde(default = "default_true")]
    dry_run: bool,
}

fn default_true() -> bool {
    true
}

/// `POST /api/maintenance/prune` — `{kind, dry_run, would_remove, errors}`
/// for a dry run (the default), `{kind, dry_run, removed, errors}`
/// otherwise; the rows are the CLI report's rows and `errors` the partial
/// failures netflow prune met. 400 for a bad `older_than`, 500 for a bad
/// configured retention.
async fn prune(
    State(s): State<AppState>,
    Query(q): Query<HostQuery>,
    Json(body): Json<PruneBody>,
) -> ApiResult<Json<Value>> {
    admin_ops::local_only(q.host.as_deref(), "pruning")?;
    if !body.dry_run {
        crate::api::config::require_writable_to(&s, "pruning")?;
    }
    let (kind, dry_run) = (body.kind, body.dry_run);
    let global = s.global_dir.clone();
    let project_root = rupu_workspace::project_root_for(&s.workspace_dir)
        .map_err(|e| ApiError::internal(format!("resolving the project root: {e}")))?;
    let older_than = body.older_than.clone();
    let (report, errors) = match kind {
        PruneKind::Transcripts => {
            let report = blocking(move || {
                archived_transcripts::prune_archived_transcripts(
                    &global,
                    project_root.as_deref(),
                    older_than.as_deref(),
                    dry_run,
                )
            })
            .await?
            .map_err(|e| match e {
                PruneError::InvalidRetention(e) => ApiError::bad_request(e.to_string()),
                PruneError::Config(m) => ApiError::internal(format!("config: {m}")),
                PruneError::Failed(m) => ApiError::internal(m),
            })?;
            (json!(report), Vec::new())
        }
        PruneKind::Netflow => {
            let done = blocking(move || {
                crate::netflow_prune::prune_netflow(
                    &global,
                    project_root.as_deref(),
                    older_than.as_deref(),
                    dry_run,
                )
            })
            .await?
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
            (json!(done.report), done.problems)
        }
        PruneKind::Sessions | PruneKind::Cleanup => {
            let ops = admin_ops::ops(&s, "pruning sessions")?;
            let report = ops
                .prune_sessions(PruneRequest {
                    kind,
                    older_than: body.older_than,
                    dry_run,
                })
                .await?;
            (report, Vec::new())
        }
    };
    Ok(Json(prune_answer(kind, dry_run, report, errors)))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(format!("prune task: {e}")))
}

/// `{kind, dry_run, would_remove | removed, errors}`: the report's rows
/// under the key that says whether anything was deleted.
fn prune_answer(kind: PruneKind, dry_run: bool, report: Value, errors: Vec<String>) -> Value {
    let rows = report.get("rows").cloned().unwrap_or_else(|| json!([]));
    let key = if dry_run { "would_remove" } else { "removed" };
    let mut out = serde_json::Map::new();
    out.insert("kind".into(), json!(kind.as_str()));
    out.insert("dry_run".into(), json!(dry_run));
    out.insert(key.into(), rows);
    out.insert("errors".into(), json!(errors));
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prune_answer_names_its_rows_by_what_happened() {
        let report = json!({"kind": "session_prune", "version": 1, "rows": [{"session_id": "s"}]});
        assert_eq!(
            prune_answer(PruneKind::Sessions, true, report.clone(), vec![]),
            json!({"kind": "sessions", "dry_run": true, "would_remove": [{"session_id": "s"}], "errors": []})
        );
        assert_eq!(
            prune_answer(PruneKind::Sessions, false, report, vec!["x".into()]),
            json!({"kind": "sessions", "dry_run": false, "removed": [{"session_id": "s"}], "errors": ["x"]})
        );
    }
}
