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

    // ── macOS golden fixtures (apps/rupu-macos/Fixtures/) ─────────────────
    //
    // `ToolSpecDto`/`ToolsResponse` are private to this module — the
    // integration test (`tests/macos_fixtures.rs`) can't build them, so
    // their fixture lives here instead. Same `check_fixture` contract as
    // that file (duplicated: a unit test can't share code with an
    // integration test without a public module) — see `api/host_info.rs`'s
    // test module for the established pattern.

    fn check_fixture(name: &str, value: &impl serde::Serialize) {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/rupu-macos/Fixtures");
        let path = dir.join(name);
        let rendered = serde_json::to_string_pretty(value).expect("serialize fixture");
        if std::env::var_os("REGEN_FIXTURES").is_some() {
            std::fs::write(&path, rendered + "\n").expect("write fixture");
            return;
        }
        let on_disk = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing fixture {name}; run `make macos-fixtures`"));
        assert_eq!(
            on_disk.trim_end(),
            rendered,
            "fixture {name} drifted from the Rust types; run `make macos-fixtures`"
        );
    }

    #[test]
    fn tools_fixture_is_current() {
        // Pulled from the REAL catalog (not hand-authored) so the fixture
        // can never silently drift from `tool_catalog()`'s actual shape —
        // `tool_catalog`'s own doc comment guarantees stable ordering.
        let catalog = tool_catalog();
        let read = catalog
            .iter()
            .find(|t| t.kind == ToolKind::Read)
            .expect("at least one read tool in the catalog");
        let write = catalog
            .iter()
            .find(|t| t.kind == ToolKind::Write)
            .expect("at least one write tool in the catalog");
        let response = ToolsResponse {
            tools: vec![
                ToolSpecDto {
                    name: read.name,
                    description: read.description,
                    input_schema: read.input_schema.clone(),
                    kind: kind_str(read.kind),
                },
                ToolSpecDto {
                    name: write.name,
                    description: write.description,
                    input_schema: write.input_schema.clone(),
                    kind: kind_str(write.kind),
                },
            ],
        };
        check_fixture("tools.json", &response);
    }
}
