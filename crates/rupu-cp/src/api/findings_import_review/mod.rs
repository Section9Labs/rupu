//! Bounded immutable finding-report review. Operator HTTP only.
//! The legacy import endpoint and model tool retain their existing behavior.
mod discovery;
mod engine;
mod execute;
mod model;
mod store;

use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header::CACHE_CONTROL, HeaderValue},
    routing::{get, post},
    Json, Router,
};
use model::*;
use serde::Deserialize;

pub const FEATURE: &str = "findings.import_review.v1";
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Local {
    host: Option<String>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/findings/import/preview", post(prepare_preview))
        .route("/api/findings/import/commit", post(commit))
        .route("/api/findings/import/operations/:id", get(operation))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(axum::middleware::map_response(
            |mut response: axum::response::Response| async move {
                response
                    .headers_mut()
                    .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
                response
            },
        ))
}
fn gate(s: &AppState, q: &Local, write: bool) -> ApiResult<()> {
    super::fleet::require_local(q.host.as_deref(), "reviewed finding import")?;
    if write {
        super::config::require_writable_to(s, "reviewed finding import")?;
    }
    Ok(())
}
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(|_| {
        ApiError::internal(
            "import task ended without a receipt; check the operation before taking another action",
        )
    })?
}
async fn prepare_preview(
    State(s): State<AppState>,
    Query(q): Query<Local>,
    Json(body): Json<PreviewRequest>,
) -> ApiResult<Json<Preview>> {
    gate(&s, &q, true)?;
    blocking(move || prepare_at(&s.global_dir, body))
        .await
        .map(Json)
}
fn prepare_at(global: &std::path::Path, body: PreviewRequest) -> ApiResult<Preview> {
    let store = store::Store::open(global).map_err(engine::io_error)?;
    let now = chrono::Utc::now().timestamp();
    store
        .cleanup_expired_reviews(now as u64)
        .map_err(engine::io_error)?;
    // Discover/report-read once; every commit uses the stored semantic snapshot.
    let files = discovery::discover_reports(&body.files, body.directory.as_deref())
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let review_id = ulid::Ulid::new().to_string();
    let stage = store
        .create_review(&review_id, (now + REVIEW_SECONDS) as u64)
        .map_err(engine::io_error)?;
    for (ordinal, file) in files.iter().enumerate() {
        store
            .write_review_input(&review_id, ordinal, &file.bytes)
            .map_err(engine::io_error)?;
    }
    let manifest = engine::prepare(
        global,
        body,
        review_id.clone(),
        now,
        &stage.join("artifacts"),
        &files,
    )?;
    let digest = engine::digest(&manifest)?;
    let response = engine::preview(&manifest, digest.clone())?;
    store
        .save_review(&review_id, &digest, &manifest)
        .map_err(engine::io_error)?;
    Ok(response)
}
async fn commit(
    State(s): State<AppState>,
    Query(q): Query<Local>,
    Json(body): Json<CommitRequest>,
) -> ApiResult<Json<Operation>> {
    gate(&s, &q, true)?;
    blocking(move || execute::commit_at(&s.global_dir, body))
        .await
        .map(Json)
}
async fn operation(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<Local>,
) -> ApiResult<Json<Operation>> {
    gate(&s, &q, false)?;
    blocking(move || execute::read_operation(&s.global_dir, &id))
        .await
        .map(Json)
}

#[cfg(test)]
mod tests;
