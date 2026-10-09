//! The tool catalog — read-only surface for the CP (spec W4 §3.4).
//!
//! `GET /api/tools` lists every tool in `rupu_tools::ToolCatalog`: builtins,
//! coverage/findings, connectors and the agentiflow tools, each with its
//! effect, the services it needs and whether an `action:` workflow step may
//! call it (`action_eligible`, which the workflow editor's action picker
//! filters on).

use crate::state::AppState;
use axum::{routing::get, Json, Router};
use rupu_tools::{Effect, Service, ToolCatalog};
use serde::Serialize;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/tools", get(get_tools))
}

#[derive(Serialize)]
struct ToolDto {
    name: &'static str,
    /// Legacy names the tool still answers to.
    aliases: Vec<&'static str>,
    /// `core` for the core fs/shell tools, else the name's first segment;
    /// `null` for the legacy unqualified dispatch names.
    namespace: Option<&'static str>,
    effect: Effect,
    needs: &'static [Service],
    description: &'static str,
    input_schema: serde_json::Value,
    action_eligible: bool,
}

#[derive(Serialize)]
struct ToolsResponse {
    tools: Vec<ToolDto>,
}

async fn get_tools() -> Json<ToolsResponse> {
    let tools = ToolCatalog::all()
        .iter()
        .map(|d| ToolDto {
            name: d.name,
            aliases: d.aliases.iter().map(|a| a.name).collect(),
            namespace: d.namespace(),
            effect: d.effect,
            needs: d.needs,
            description: d.description,
            input_schema: (d.input_schema)(),
            action_eligible: d.is_action_eligible(),
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

    /// `api_tools_lists_catalog` (W4 §6.5): the whole catalog, with effects.
    #[tokio::test]
    async fn get_tools_lists_the_whole_catalog_with_effects() {
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
        assert_eq!(tools.len(), ToolCatalog::all().len());
        let tool = |name: &str| {
            tools
                .iter()
                .find(|t| t["name"] == name)
                .unwrap_or_else(|| panic!("{name} missing"))
        };

        let bash = tool("bash");
        assert_eq!(bash["effect"], "write");
        assert_eq!(bash["namespace"], "core");
        assert_eq!(bash["action_eligible"], false);

        let report = tool("findings.report");
        assert_eq!(report["effect"], "record");
        assert_eq!(report["needs"], serde_json::json!(["findings"]));
        assert!(report["aliases"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("findings.record")));
        assert_eq!(report["action_eligible"], true);

        let board = tool("board.post");
        assert_eq!(board["effect"], "record");
        assert_eq!(board["action_eligible"], false);

        let create_pr = tool("scm.prs.create");
        assert_eq!(create_pr["effect"], "external");
        assert_eq!(create_pr["namespace"], "scm");
        assert_eq!(create_pr["action_eligible"], true);
        assert!(create_pr["input_schema"]["properties"]["head"].is_object());
    }
}
