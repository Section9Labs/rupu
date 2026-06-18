use crate::state::AppState;
use axum::{routing::get, Router};
use tower_http::trace::TraceLayer;

async fn healthz() -> &'static str {
    "ok"
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
