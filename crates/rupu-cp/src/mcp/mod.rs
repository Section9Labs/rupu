//! The operator MCP server inside `cp serve`: `/mcp` ([`endpoint`]) over the
//! tool sources [`build_server`] assembles.

pub mod endpoint;
pub mod operator_source;
pub mod ops;

use crate::state::AppState;
use rupu_mcp::{McpServer, ServeError, ToolFilter, ToolSource};
use std::sync::Arc;

/// What `cp serve` hands the MCP side.
pub struct McpOpts {
    /// The `rupu-tools` catalog as a source (built by the CLI, which owns the
    /// SCM registry and findings options); `None` serves no catalog tools.
    pub catalog: Option<Arc<dyn ToolSource>>,
    /// `--tools` entries; empty = every tool.
    pub tools: Vec<String>,
}

/// The operator source + the wait source + `opts.catalog`, filtered by
/// `opts.tools`.
///
/// The wait source (Task 8) joins `sources` here.
pub fn build_server(state: &AppState, opts: &McpOpts) -> Result<Arc<McpServer>, ServeError> {
    let mut sources: Vec<Arc<dyn ToolSource>> = vec![Arc::new(
        operator_source::OperatorSource::new(inner_api(state)),
    )];
    if let Some(catalog) = &opts.catalog {
        sources.push(Arc::clone(catalog));
    }
    let filter = ToolFilter::parse(&opts.tools);
    Ok(Arc::new(McpServer::new(sources, &filter)?))
}

/// `api_routes().with_state(state)` with the unsafe-segment guard applied
/// (no token layer): what in-process calls go through.
pub(crate) fn inner_api(state: &AppState) -> axum::Router {
    crate::server::api_routes()
        .with_state(state.clone())
        .layer(axum::middleware::from_fn(
            crate::path_guard::reject_unsafe_segments,
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt as _;

    #[tokio::test]
    async fn inner_api_serves_a_get_route_without_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().to_path_buf(), Default::default());
        let resp = inner_api(&state)
            .oneshot(
                http::Request::builder()
                    .uri("/api/tools")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }
}
