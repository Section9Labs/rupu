//! The operator MCP server inside `cp serve`: `/mcp` ([`endpoint`]) over the
//! tool sources [`build_server`] assembles, plus the [`resources`] and
//! [`prompts`] an agent learns rupu from.

pub mod endpoint;
pub mod operator_source;
pub mod ops;
pub mod prompts;
pub mod resources;
pub mod wait;

use crate::state::AppState;
use async_trait::async_trait;
use rupu_mcp::{
    CallCx, McpServer, ServeError, Sources, ToolDef, ToolFilter, ToolResult, ToolSource,
};
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
/// filtered by `opts.tools`, with the [`resources`] and [`prompts`]
/// (`--tools` filters tools only).
pub fn build_server(state: &AppState, opts: &McpOpts) -> Result<Arc<McpServer>, ServeError> {
    let tools = sources(
        Arc::new(operator_source::OperatorSource::new(inner_api(state))),
        Arc::new(wait::WaitSource::new(state.clone())),
        opts.catalog.as_ref(),
    );
    let filter = ToolFilter::parse(&opts.tools);
    Ok(Arc::new(McpServer::from_sources(
        Sources {
            tools,
            resources: vec![Arc::new(resources::RupuResources)],
            prompts: vec![Arc::new(prompts::RupuPrompts)],
        },
        &filter,
    )?))
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
    // These catalog tools require an assembled agent execution and invocation.
    // The CP exposes its separate owner-scoped board.work.* operator tools;
    // installing a catalog must not additionally grant agent self-claim/result.
    let catalog = catalog.map(|inner| {
        let shadowed = inner
            .tools()
            .into_iter()
            .map(|tool| tool.name)
            .filter(|name| name.starts_with("board.work_"))
            .collect();
        Arc::new(Shadowed {
            inner: Arc::clone(inner),
            shadowed,
        }) as Arc<dyn ToolSource>
    });
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for source in [Some(operator), Some(wait), catalog].into_iter().flatten() {
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

/// `api_routes().with_state(state)` with the route guard ([`guarded`]) and
/// the unsafe-segment guard applied (no token layer): what in-process calls
/// go through.
pub(crate) fn inner_api(state: &AppState) -> axum::Router {
    guarded(crate::server::api_routes().with_state(state.clone())).layer(axum::middleware::from_fn(
        crate::path_guard::reject_unsafe_segments,
    ))
}

/// The route template an operator tool's request is meant for (as the
/// router registers it, `{root}` expanded): set by the operator source as a
/// request extension, checked by [`guarded`]'s layer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ExpectedRoute(pub(crate) &'static str);

/// `router` with a route layer that refuses, before the handler runs, a
/// request whose [`ExpectedRoute`] is not the route it matched: an argument
/// that spells a sibling route's literal segment (`/api/runs/workflows` for
/// `/api/runs/:id`) never reaches that sibling's handler. A request without
/// the extension passes.
pub(crate) fn guarded(router: axum::Router) -> axum::Router {
    router.route_layer(axum::middleware::from_fn(check_expected_route))
}

async fn check_expected_route(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let expected = req.extensions().get::<ExpectedRoute>().map(|e| e.0);
    let matched = req
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|m| m.as_str());
    if let (Some(expected), Some(matched)) = (expected, matched) {
        if expected != matched {
            return crate::error::ApiError::bad_request(format!(
                "`{}` routes to `{matched}`, not `{expected}`",
                req.uri()
            ))
            .into_response();
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt as _;

    struct CountingCatalog(std::sync::atomic::AtomicUsize);
    #[async_trait]
    impl ToolSource for CountingCatalog {
        fn tools(&self) -> Vec<ToolDef> {
            [
                "board.work_claim",
                "board.work_complete",
                "board.work_post",
                "ordinary.read",
            ]
            .into_iter()
            .map(|name| ToolDef {
                name: name.into(),
                description: "Fixture.".into(),
                input_schema: serde_json::json!({"type":"object"}),
                hints: Default::default(),
            })
            .collect()
        }
        async fn call(&self, _name: &str, _args: Value, _cx: &CallCx) -> Option<ToolResult> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some(ToolResult::Json(serde_json::json!({"called":true})))
        }
    }
    #[tokio::test]
    async fn operator_catalog_does_not_expose_or_invoke_agent_work_authority() {
        let catalog = Arc::new(CountingCatalog(std::sync::atomic::AtomicUsize::new(0)));
        let erased: Arc<dyn ToolSource> = catalog.clone();
        let composed = sources(
            Arc::new(operator_source::OperatorSource::new(axum::Router::new())),
            Arc::new(Listed(vec![])),
            Some(&erased),
        );
        let names: Vec<_> = composed
            .iter()
            .flat_map(|s| s.tools())
            .map(|t| t.name)
            .collect();
        assert!(names.contains(&"board.work.post".into()));
        assert!(names.contains(&"ordinary.read".into()));
        assert!(!names.iter().any(|n| n.starts_with("board.work_")));
        let filtered = composed.last().unwrap();
        for name in ["board.work_claim", "board.work_complete", "board.work_post"] {
            assert!(filtered
                .call(name, Value::Null, &CallCx::default())
                .await
                .is_none());
        }
        assert_eq!(catalog.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(filtered
            .call("ordinary.read", Value::Null, &CallCx::default())
            .await
            .is_some());
        assert_eq!(catalog.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

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

    #[tokio::test]
    async fn inner_api_refuses_a_request_that_matched_another_route() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().to_path_buf(), Default::default());
        let request = |expected| {
            let mut r = http::Request::builder()
                .uri("/api/runs/workflows")
                .body(axum::body::Body::empty())
                .unwrap();
            r.extensions_mut().insert(ExpectedRoute(expected));
            r
        };
        let resp = inner_api(&state)
            .oneshot(request("/api/runs/:id"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            v["error"],
            "`/api/runs/workflows` routes to `/api/runs/workflows`, not `/api/runs/:id`"
        );
        let resp = inner_api(&state)
            .oneshot(request("/api/runs/workflows"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }
}
