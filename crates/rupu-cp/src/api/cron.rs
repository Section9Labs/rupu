//! Cron and polled-event triggers, through the `AdminOps` port:
//! - `GET  /api/cron`        — `rupu cron list`: cron-triggered workflows and
//!   when each fires next.
//! - `GET  /api/cron/events` — `rupu cron events`: event-triggered workflows,
//!   the poll sources and their cursors.
//! - `POST /api/cron/tick`   — the tick `cp serve`'s cron loop runs, now.
//!
//! Local-only: they read and drive this control plane's own `RUPU_HOME`.

use crate::{
    admin_ops::{self, CronTickOutcome, CronTickRequest, HostQuery},
    error::ApiResult,
    state::AppState,
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/cron", get(cron_list))
        .route("/api/cron/events", get(cron_events))
        .route("/api/cron/tick", post(cron_tick))
}

/// `GET /api/cron` — the `cron_list` report: per cron-triggered workflow its
/// `schedule`, `next_utc` and `in_seconds` (both null when the schedule
/// can't be computed).
async fn cron_list(
    State(s): State<AppState>,
    Query(q): Query<HostQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    admin_ops::local_only(q.host.as_deref(), "listing cron workflows")?;
    let ops = admin_ops::ops(&s, "listing cron workflows")?;
    Ok(Json(ops.cron_list().await?))
}

/// `GET /api/cron/events` — the `cron_events` report: per event-triggered
/// workflow its `event`, the configured `sources` and the latest `cursor`.
async fn cron_events(
    State(s): State<AppState>,
    Query(q): Query<HostQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    admin_ops::local_only(q.host.as_deref(), "listing event-triggered workflows")?;
    let ops = admin_ops::ops(&s, "listing event-triggered workflows")?;
    Ok(Json(ops.cron_events().await?))
}

/// `POST /api/cron/tick` body.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct CronTickBody {
    /// Report what would fire; fire nothing and advance no state.
    #[serde(default)]
    dry_run: bool,
    /// Only the cron-scheduled tier: skip event polling.
    #[serde(default)]
    skip_events: bool,
    /// Only the event-polling tier: skip cron-scheduled fires.
    #[serde(default)]
    only_events: bool,
}

/// `POST /api/cron/tick` — run the tick `cp serve`'s cron loop runs, now.
///
/// Single-flight with that loop: 409 while a tick (the loop's or another
/// request's) is in progress. A real tick fires workflow runs, which can
/// take as long as those runs do, so it runs in the background and the
/// answer is 202 `{started: true}`. A `dry_run` answers 200 with its
/// report (`would_fire`) once it has looked.
async fn cron_tick(
    State(s): State<AppState>,
    Query(q): Query<HostQuery>,
    Json(body): Json<CronTickBody>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    admin_ops::local_only(q.host.as_deref(), "a cron tick")?;
    if body.skip_events && body.only_events {
        return Err(crate::error::ApiError::bad_request(
            "`skip_events` and `only_events` exclude each other",
        ));
    }
    if !body.dry_run {
        crate::api::config::require_writable_to(&s, "a cron tick")?;
    }
    let ops = admin_ops::ops(&s, "a cron tick")?;
    let outcome = ops
        .cron_tick(CronTickRequest {
            dry_run: body.dry_run,
            skip_events: body.skip_events,
            only_events: body.only_events,
        })
        .await?;
    Ok(match outcome {
        CronTickOutcome::Started => (
            StatusCode::ACCEPTED,
            Json(json!({ "started": true, "dry_run": false })),
        ),
        CronTickOutcome::DryRun(report) => (StatusCode::OK, Json(report)),
    })
}
