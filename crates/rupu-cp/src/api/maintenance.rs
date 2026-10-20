//! Housekeeping, through the `AdminOps` port:
//! `POST /api/maintenance/prune` — the CLI's `transcript prune`,
//! `session prune`, `netflow prune` and `cleanup`.
//!
//! Local-only: it deletes from this control plane's own `RUPU_HOME`.

use crate::{
    admin_ops::{self, HostQuery, PruneKind, PruneRequest},
    error::ApiResult,
    state::AppState,
};
use axum::{
    extract::{Query, State},
    routing::post,
    Json, Router,
};
use serde::Deserialize;

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

/// `POST /api/maintenance/prune` — `{kind, dry_run, would_remove}` for a
/// dry run (the default), `{kind, dry_run, removed}` otherwise; the rows
/// are the CLI's report rows. 400 for a bad `older_than`.
async fn prune(
    State(s): State<AppState>,
    Query(q): Query<HostQuery>,
    Json(body): Json<PruneBody>,
) -> ApiResult<Json<serde_json::Value>> {
    admin_ops::local_only(q.host.as_deref(), "pruning")?;
    if !body.dry_run {
        crate::api::config::require_writable_to(&s, "pruning")?;
    }
    let ops = admin_ops::ops(&s, "pruning")?;
    Ok(Json(
        ops.prune(PruneRequest {
            kind: body.kind,
            older_than: body.older_than,
            dry_run: body.dry_run,
        })
        .await?,
    ))
}
