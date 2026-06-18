use crate::state::AppState;
use axum::{routing::get, Router};
use tower_http::trace::TraceLayer;

async fn healthz() -> &'static str {
    "ok"
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .merge(crate::api::runs::routes())
        .merge(crate::api::agents::routes())
        .merge(crate::api::workflows::routes())
        .merge(crate::api::sessions::routes())
        .merge(crate::api::workers::routes())
        .merge(crate::api::coverage::routes())
        .merge(crate::api::dashboard::routes())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
