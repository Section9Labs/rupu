//! `GET /api/runs/:id/board[?host=]` — a workflow or standalone run's board
//! (`rupu_runtime::services::board::BoardView`): the claims its message
//! space's journal recorded (or, on a board older than the journal, the live
//! ones) and its directives with their retractions. The same view
//! `rupu __board` prints and `GET /api/agentiflows/:id/board` serves.
//!
//! `?host=<id>` reads a remote run's board through that host's connector
//! ([`HostConnector::run_board`]). Only a `run_…` id is served here.
//!
//! [`HostConnector::run_board`]: crate::host::connector::HostConnector::run_board

use std::path::Path;

use axum::extract::{Path as AxPath, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use rupu_runtime::services::board::{board_view, BoardView};
use rupu_runtime::services::operator::{MessageError, MessageErrorCode};
use serde::Deserialize;

use crate::error::{ApiError, ApiResult};
use crate::host::messages::{self, MessageRoute};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/runs/:id/board", get(get_run_board))
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub(crate) struct BoardQuery {
    /// When present and not `local`, read the board of the run on that host.
    #[serde(default)]
    host: Option<String>,
}

/// The board of run `id` on this host (a `run_…` id only). Blocking file
/// reads.
pub fn local_run_board(global: &Path, id: &str) -> Result<BoardView, MessageError> {
    if MessageRoute::of_id(id) != Some(MessageRoute::Runs) {
        return Err(MessageRoute::Runs.not_found(id));
    }
    let (_, root) = messages::root(global, id)?;
    board_view(&root.space, chrono::Utc::now()).map_err(|e| {
        MessageError::new(
            MessageErrorCode::Internal,
            format!("run `{id}`: the board is unreadable: {e}"),
        )
    })
}

async fn get_run_board(
    State(s): State<AppState>,
    AxPath(id): AxPath<String>,
    Query(q): Query<BoardQuery>,
) -> ApiResult<Json<BoardView>> {
    if let Some(host) = q.host.as_deref().filter(|h| *h != "local") {
        if MessageRoute::of_id(&id) != Some(MessageRoute::Runs) {
            return Err(ApiError::not_found(format!("no run `{id}`")));
        }
        let conn = crate::api::runs::resolve_host(&s, host)?;
        return conn
            .run_board(&id)
            .await
            .map(Json)
            .map_err(|e| crate::api::runs::host_read_error(host, e));
    }
    let global = s.global_dir.clone();
    tokio::task::spawn_blocking(move || local_run_board(&global, &id))
        .await
        .map_err(|e| ApiError::internal(format!("reading a run's board panicked: {e}")))?
        .map(Json)
        .map_err(|e| match e.code {
            MessageErrorCode::NotFound => ApiError::not_found(e.message),
            _ => ApiError::internal(e.message),
        })
}
