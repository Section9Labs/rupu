use crate::state::AppState;
use axum::{
    extract::Request,
    http::{header::AUTHORIZATION, StatusCode},
    middleware::{from_fn_with_state, Next},
    response::Response,
    routing::get,
    Router,
};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tower_http::trace::TraceLayer;

async fn healthz() -> &'static str {
    "ok"
}

/// Bearer-token guard for the `/api/*` surface.
///
/// When a token is configured the request must carry
/// `Authorization: Bearer <token>` or it is rejected with `401`. The token is
/// compared in constant time. When no token is configured this middleware is
/// never installed (the API stays open — Phase-1 localhost posture).
async fn require_bearer(
    axum::extract::State(token): axum::extract::State<Arc<String>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let presented = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match presented {
        Some(p) if bool::from(p.as_bytes().ct_eq(token.as_bytes())) => Ok(next.run(req).await),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

/// Build the control-plane router.
///
/// `token`: when `Some`, every `/api/*` route requires
/// `Authorization: Bearer <token>` (constant-time compared) and otherwise
/// returns `401`. `/healthz` and the static UI / SPA fallback remain open
/// regardless — on Phase-1 localhost the token guards the API from other
/// local processes while the browser app loads without a header.
pub fn router(state: AppState, token: Option<String>) -> Router {
    let api = Router::new()
        .merge(crate::api::runs::routes())
        .merge(crate::api::agents::routes())
        .merge(crate::api::workflows::routes())
        .merge(crate::api::sessions::routes())
        .merge(crate::api::workers::routes())
        .merge(crate::api::coverage::routes())
        .merge(crate::api::dashboard::routes())
        .merge(crate::api::events::routes());

    let api = match token {
        Some(t) => api.layer(from_fn_with_state(Arc::new(t), require_bearer)),
        None => api,
    };

    Router::new()
        .route("/healthz", get(healthz))
        .merge(api)
        // Registered routes above match first; anything else (incl. client-side
        // routes like `/runs/abc`) falls through to the embedded SPA.
        .fallback(crate::embed::static_handler)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
