//! The operator MCP server inside `cp serve`: `/mcp` ([`endpoint`]) over the
//! tool sources [`build_server`] assembles.

pub mod endpoint;
pub mod operator_source;
pub mod ops;
pub mod wait;

use crate::state::AppState;
use async_trait::async_trait;
use rupu_mcp::{CallCx, McpServer, ServeError, ToolDef, ToolFilter, ToolResult, ToolSource};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

/// Marks a request the in-process operator source made for an MCP tool
/// call. It rides as a request extension, which no HTTP client can set, so
/// a handler that finds it knows the call came through `/mcp` (a header
/// could be sent by anyone).
#[derive(Debug, Clone, Copy)]
pub(crate) struct McpClient;

/// A handler's view of [`McpClient`]: `Some` for a call through `/mcp`.
pub(crate) type ViaMcp = Option<axum::Extension<McpClient>>;

/// The actor a local run-control decision records: `mcp` for a call
/// through `/mcp`, else `web`.
pub(crate) fn actor(via: &ViaMcp) -> &'static str {
    if via.is_some() {
        "mcp"
    } else {
        "web"
    }
}

/// What `cp serve` hands the MCP side.
pub struct McpOpts {
    /// The `rupu-tools` catalog as a source (built by the CLI, which owns the
    /// SCM registry and findings options); `None` serves no catalog tools.
    pub catalog: Option<Arc<dyn ToolSource>>,
    /// `--tools` entries; empty = every tool.
    pub tools: Vec<String>,
}

/// The operator source + the wait source + `opts.catalog` ([`sources`]),
/// filtered by `opts.tools`.
pub fn build_server(state: &AppState, opts: &McpOpts) -> Result<Arc<McpServer>, ServeError> {
    let sources = sources(
        Arc::new(operator_source::OperatorSource::new(inner_api(state))),
        Arc::new(wait::WaitSource::new(state.clone())),
        opts.catalog.as_ref(),
    );
    let filter = ToolFilter::parse(&opts.tools);
    Ok(Arc::new(McpServer::new(sources, &filter)?))
}

/// Check `tools` (the `--tools` entries) against the names the server would
/// serve, without an `AppState`, so a bad grant fails before anything starts.
/// The names come from [`sources`], as [`build_server`]'s do.
pub fn validate_tools(
    tools: &[String],
    catalog: Option<&Arc<dyn ToolSource>>,
) -> Result<(), ServeError> {
    let names: Vec<String> = served_names(catalog).into_iter().collect();
    ToolFilter::parse(tools).apply(&names).map(|_| ())
}

/// The tool names the server serves, after shadowing (each once), built
/// without an `AppState` as [`validate_tools`] does.
pub fn served_names(catalog: Option<&Arc<dyn ToolSource>>) -> BTreeSet<String> {
    sources(
        Arc::new(operator_source::OperatorSource::new(axum::Router::new())),
        Arc::new(Listed(vec![wait::tool_def()])),
        catalog,
    )
    .iter()
    .flat_map(|s| s.tools())
    .map(|t| t.name)
    .collect()
}

/// What `/mcp` serves, in precedence order: the operator tools, `wait`,
/// then the catalog. A tool an earlier source serves is left out of every
/// later one (a catalog tool the operator surface has its own version of,
/// e.g. `agents.list`); a catalog tool no operator tool covers is served.
pub(crate) fn sources(
    operator: Arc<dyn ToolSource>,
    wait: Arc<dyn ToolSource>,
    catalog: Option<&Arc<dyn ToolSource>>,
) -> Vec<Arc<dyn ToolSource>> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for source in [Some(operator), Some(wait), catalog.cloned()]
        .into_iter()
        .flatten()
    {
        let names: BTreeSet<String> = source.tools().into_iter().map(|t| t.name).collect();
        let shadowed: BTreeSet<String> = names.intersection(&seen).cloned().collect();
        seen.extend(names);
        out.push(if shadowed.is_empty() {
            source
        } else {
            Arc::new(Shadowed {
                inner: source,
                shadowed,
            })
        });
    }
    out
}

/// A source less the tools an earlier source serves.
struct Shadowed {
    inner: Arc<dyn ToolSource>,
    shadowed: BTreeSet<String>,
}

#[async_trait]
impl ToolSource for Shadowed {
    fn tools(&self) -> Vec<ToolDef> {
        self.inner
            .tools()
            .into_iter()
            .filter(|t| !self.shadowed.contains(&t.name))
            .collect()
    }

    async fn call(&self, name: &str, args: Value, cx: &CallCx) -> Option<ToolResult> {
        if self.shadowed.contains(name) {
            return None;
        }
        self.inner.call(name, args, cx).await
    }
}

/// Tool definitions only, for [`validate_tools`]; never called.
struct Listed(Vec<ToolDef>);

#[async_trait]
impl ToolSource for Listed {
    fn tools(&self) -> Vec<ToolDef> {
        self.0.clone()
    }

    async fn call(&self, _name: &str, _args: Value, _cx: &CallCx) -> Option<ToolResult> {
        None
    }
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
