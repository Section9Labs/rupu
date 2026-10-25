use crate::state::AppState;
use axum::{
    http::{HeaderName, StatusCode},
    middleware::from_fn_with_state,
    routing::get,
    Router,
};
use std::sync::Arc;
use tower_http::{
    classify::ServerErrorsFailureClass,
    trace::{DefaultMakeSpan, OnFailure, TraceLayer},
};
use tracing::Level;

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

/// Every `/api/*` route, no auth layer: the in-process MCP caller's router.
pub(crate) fn api_routes() -> Router<AppState> {
    Router::new()
        .merge(crate::api::agentiflows::routes())
        .merge(crate::api::agentiflow_defs::routes())
        .merge(crate::api::messages::routes())
        .merge(crate::api::children::routes())
        .merge(crate::api::board::routes())
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
        .merge(crate::api::query::routes())
        .merge(crate::api::dashboard::routes())
        .merge(crate::api::events::routes())
        .merge(crate::api::graph::routes())
        .merge(crate::api::netflow::routes())
        .merge(crate::api::run_streams::routes())
        .merge(crate::api::autoflow_runs::routes())
        .merge(crate::api::repos::routes())
        .merge(crate::api::models::routes())
        .merge(crate::api::fs::routes())
        .merge(crate::api::host_info::routes())
        .merge(crate::api::hosts::routes())
        .merge(crate::api::skills::routes())
        .merge(crate::api::fleet::routes())
        .merge(crate::api::engagement_profiles::routes())
        .merge(crate::api::tools::routes())
        .merge(crate::api::workspace::routes())
        .merge(crate::api::scm::routes())
        .merge(crate::api::auth_status::routes())
        .merge(crate::api::cron::routes())
        .merge(crate::api::maintenance::routes())
}

/// What `router_with` serves besides `/api/*` and `/healthz`.
pub struct RouterOpts {
    /// Serve the embedded web UI (the SPA fallback). `false` = `--no-web`:
    /// an unmatched path is a plain 404.
    pub web: bool,
    /// The `/mcp` endpoint's server. `None` = `--no-mcp`: `/mcp` is a JSON 404.
    pub mcp: Option<Arc<rupu_mcp::McpServer>>,
}

/// Build the control-plane router: the web UI and no MCP.
///
/// `token`: see [`router_with`].
pub fn router(state: AppState, token: Option<String>) -> Router {
    router_with(
        state,
        token,
        RouterOpts {
            web: true,
            mcp: None,
        },
    )
}

async fn mcp_off() -> crate::error::ApiError {
    crate::error::ApiError(
        axum::http::StatusCode::NOT_FOUND,
        "MCP is off (--no-mcp)".into(),
    )
}

/// Build the control-plane router.
///
/// `token`: when `Some`, every `/api/*` route and `/mcp` requires the token —
/// as `Authorization: Bearer <token>` or as the browser's token cookie, which
/// a page load with `?token=<token>` sets ([`crate::auth`]) — and otherwise
/// returns `401`. `/healthz`, the node WS endpoint (enrollment-token gated)
/// and the static UI / SPA fallback stay open: the pages hold no data, every
/// byte of it comes from `/api/*`.
pub fn router_with(state: AppState, token: Option<String>, opts: RouterOpts) -> Router {
    let loopback = state
        .bind
        .parse::<std::net::SocketAddr>()
        .is_ok_and(|a| a.ip().is_loopback());
    let mcp = match opts.mcp {
        Some(server) => crate::mcp::endpoint::routes(server, loopback),
        // Mounted explicitly so the SPA fallback never answers `/mcp`.
        None => Router::new().route("/mcp", axum::routing::any(mcp_off)),
    };
    let api = api_routes().merge(mcp);

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
        .merge(api);
    // Registered routes above match first. Anything else is handled by the
    // embedded-UI fallback: an unmatched `/api` or `/api/*` path is a JSON
    // 404 (never the SPA — that made every missing endpoint look present
    // on an older CP), and every other path (incl. client-side routes like
    // `/runs/abc`) falls through to the embedded SPA. Without the web UI
    // (`--no-web`) every other path is a plain 404.
    let app = if opts.web {
        app.fallback(crate::embed::static_handler)
    } else {
        app.fallback(crate::embed::no_web_handler)
    };
    let app = match token {
        Some(t) => app.layer(from_fn_with_state(t, crate::auth::bootstrap_cookie)),
        None => app,
    };
    app.layer(axum::middleware::from_fn(
        crate::path_guard::reject_unsafe_segments,
    ))
    .layer(
        TraceLayer::new_for_http()
            // The span is what names the request on a failure line; at
            // tower-http's default (DEBUG) `cp serve`'s `info` filter drops
            // it, and a failed response logged with no route or host.
            .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
            .on_failure(HostAwareOnFailure),
    )
    .with_state(state)
}

/// The level a failed response is logged at. A 501 or a 502 is this
/// server's honest answer that the call can't be served from here — a
/// remote host that is down or too old, an adapter only `rupu cp serve`
/// installs (`docs/cp-api.md`'s status table) — a state the caller is told
/// and the web shows as offline or unavailable, not this server's fault: a
/// warning, beside the handler's own. Every other failure (a 500, a
/// transport error) is this server's, and stays an error.
fn failure_level(class: &ServerErrorsFailureClass) -> Level {
    match class {
        ServerErrorsFailureClass::StatusCode(code)
            if *code == StatusCode::NOT_IMPLEMENTED || *code == StatusCode::BAD_GATEWAY =>
        {
            Level::WARN
        }
        _ => Level::ERROR,
    }
}

/// tower-http's default failure line, at [`failure_level`] instead of
/// always ERROR; it is emitted inside the request span, so it names the
/// method and uri (host id included).
#[derive(Clone, Copy)]
struct HostAwareOnFailure;

impl OnFailure<ServerErrorsFailureClass> for HostAwareOnFailure {
    fn on_failure(
        &mut self,
        class: ServerErrorsFailureClass,
        latency: std::time::Duration,
        _span: &tracing::Span,
    ) {
        let latency_ms = latency.as_millis();
        match failure_level(&class) {
            Level::WARN => tracing::warn!(classification = %class, latency_ms, "response failed"),
            _ => tracing::error!(classification = %class, latency_ms, "response failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A remote host's answer (can't serve it, can't be reached) is a
    /// warning; this server's own failure is an error.
    #[test]
    fn a_remote_hosts_answer_warns_and_this_servers_fault_errors() {
        use ServerErrorsFailureClass as Class;
        assert_eq!(
            failure_level(&Class::StatusCode(StatusCode::NOT_IMPLEMENTED)),
            Level::WARN
        );
        assert_eq!(
            failure_level(&Class::StatusCode(StatusCode::BAD_GATEWAY)),
            Level::WARN
        );
        assert_eq!(
            failure_level(&Class::StatusCode(StatusCode::INTERNAL_SERVER_ERROR)),
            Level::ERROR
        );
        assert_eq!(
            failure_level(&Class::StatusCode(StatusCode::SERVICE_UNAVAILABLE)),
            Level::ERROR
        );
        assert_eq!(failure_level(&Class::Error("io".into())), Level::ERROR);
    }
}
