//! MCP tool catalog — read-only surface for the workflow-step editor.
//!
//! `GET /api/tools` mirrors `rupu_mcp::tools::tool_catalog()` into a wire
//! shape the web editor can render as connector cards. `ToolSpec::kind` is
//! `#[serde(skip)]` on the internal type (its serialization is pinned by
//! `rupu-mcp`'s `schema_snapshot` test), so we map into a local response DTO
//! here instead of touching that derive.

use crate::state::AppState;
use axum::{routing::get, Json, Router};
use rupu_mcp::tools::{tool_catalog, ToolKind};
use serde::Serialize;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/tools", get(get_tools))
}

#[derive(Serialize)]
struct ToolSpecDto {
    name: &'static str,
    description: &'static str,
    input_schema: serde_json::Value,
    kind: &'static str,
}

#[derive(Serialize)]
struct ToolsResponse {
    tools: Vec<ToolSpecDto>,
}

fn kind_str(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Write => "write",
    }
}

async fn get_tools() -> Json<ToolsResponse> {
    let tools = tool_catalog()
        .into_iter()
        .map(|t| ToolSpecDto {
            name: t.name,
            description: t.description,
            input_schema: t.input_schema,
            kind: kind_str(t.kind),
        })
        .collect();
    Json(ToolsResponse { tools })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::Request;
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    fn test_state(tmp: &tempfile::TempDir) -> AppState {
        AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
    }

    #[tokio::test]
    async fn get_tools_lists_the_mcp_catalog_with_kind() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app = routes().with_state(test_state(&tmp));

        let req = Request::builder()
            .method("GET")
            .uri("/api/tools")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), http::StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let tools = json["tools"].as_array().expect("tools array");

        let create_pr = tools
            .iter()
            .find(|t| t["name"] == "scm.prs.create")
            .expect("scm.prs.create present");
        assert_eq!(create_pr["kind"], "write");

        let has_read = tools.iter().any(|t| t["kind"] == "read");
        assert!(has_read, "at least one read tool expected");
    }
}
