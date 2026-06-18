use crate::state::AppState;
use axum::{routing::get, Router};

async fn healthz() -> &'static str {
    "ok"
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .with_state(state)
}
