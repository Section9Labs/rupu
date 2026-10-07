use crate::state::AppState;
use axum::{http::HeaderName, middleware::from_fn_with_state, routing::get, Router};
use std::sync::Arc;
use tower_http::trace::TraceLayer;

/// Liveness probe. The body stays the bare `ok` every existing consumer
/// compares against; the serving PID and crate version ride along as
/// response headers so a second `rupu cp serve` that loses the bind race
/// can report which process already owns the port (`rupu_cp::bind`).
async fn healthz() -> ([(HeaderName, String); 2], &'static str) {
    (
        [
            (
                HeaderName::from_static(crate::HEALTHZ_PID_HEADER),
                std::process::id().to_string(),
            ),
            (
                HeaderName::from_static(crate::HEALTHZ_VERSION_HEADER),
                env!("CARGO_PKG_VERSION").to_string(),
            ),
        ],
        "ok",
    )
}

/// Build the control-plane router.
///
/// `token`: when `Some`, every `/api/*` route requires the token — as
/// `Authorization: Bearer <token>` or as the browser's token cookie, which a
/// page load with `?token=<token>` sets ([`crate::auth`]) — and otherwise
/// returns `401`. `/healthz`, the node WS endpoint (enrollment-token gated)
/// and the static UI / SPA fallback stay open: the pages hold no data, every
/// byte of it comes from `/api/*`.
pub fn router(state: AppState, token: Option<String>) -> Router {
    let api = Router::new()
        .merge(crate::api::agentiflows::routes())
        .merge(crate::api::assets::routes())
        .merge(crate::api::autoflows::routes())
        .merge(crate::api::autoflow_claims::routes())
        .merge(crate::api::projects::routes())
        .merge(crate::api::customers::routes())
        .merge(crate::api::launch_preview::routes())
        .merge(crate::api::config::routes())
        .merge(crate::api::runs::routes())
        .merge(crate::api::agents::routes())
        .merge(crate::api::workflows::routes())
        .merge(crate::api::sessions::routes())
        .merge(crate::api::source::routes())
        .merge(crate::api::code::routes())
        .merge(crate::api::transcript::routes())
        .merge(crate::api::transcripts::routes())
        .merge(crate::api::usage::routes())
        .merge(crate::api::usage_outliers::routes())
        .merge(crate::api::workers::routes())
        .merge(crate::api::coverage::routes())
        .merge(crate::api::findings::routes())
        .merge(crate::api::dashboard::routes())
        .merge(crate::api::events::routes())
        .merge(crate::api::graph::routes())
        .merge(crate::api::netflow::routes())
        .merge(crate::api::run_streams::routes())
        .merge(crate::api::repos::routes())
        .merge(crate::api::models::routes())
        .merge(crate::api::fs::routes())
        .merge(crate::api::host_info::routes())
        .merge(crate::api::hosts::routes())
        .merge(crate::api::tools::routes())
        .merge(crate::api::workspace::routes());

    let token = token.map(crate::auth::Token::new);
    let api = match &token {
        Some(t) => api.layer(from_fn_with_state(
            Arc::clone(t),
            crate::auth::require_token,
        )),
        None => api,
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        // Node WS endpoint sits OUTSIDE the bearer layer — it is token-gated
        // by the Hello frame's enrollment token, not the API bearer.  Mounting
        // it here (same level as /healthz) keeps it reachable even when a
        // bearer token is configured.
        .merge(crate::node::server::routes())
        .merge(api)
        // Registered routes above match first. Anything else is handled by the
        // embedded-UI fallback: an unmatched `/api` or `/api/*` path is a JSON
        // 404 (never the SPA — that made every missing endpoint look present
        // on an older CP), and every other path (incl. client-side routes like
        // `/runs/abc`) falls through to the embedded SPA.
        .fallback(crate::embed::static_handler);
    let app = match token {
        Some(t) => app.layer(from_fn_with_state(t, crate::auth::bootstrap_cookie)),
        None => app,
    };
    app.layer(axum::middleware::from_fn(
        crate::path_guard::reject_unsafe_segments,
    ))
    .layer(TraceLayer::new_for_http())
    .with_state(state)
}
