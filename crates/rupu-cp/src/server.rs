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
        .merge(crate::api::events::routes())
        // Registered routes above match first; anything else (incl. client-side
        // routes like `/runs/abc`) falls through to the embedded SPA.
        .fallback(crate::embed::static_handler)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
