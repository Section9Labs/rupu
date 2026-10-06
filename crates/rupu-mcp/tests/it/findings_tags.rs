//! `findings.query` / `findings.tag`: the MCP-side tag tools for `action:`
//! steps and `rupu mcp serve`.

use rupu_mcp::{FindingsContext, McpPermission, ToolDispatcher};
use rupu_scm::Registry;
use rupu_tools::PermissionMode;
use std::sync::Arc;

fn ctx(workspace: &std::path::Path) -> FindingsContext {
    FindingsContext {
        workspace_path: workspace.to_path_buf(),
        scope_name: "triage-flow".to_string(),
        run_id: "run_mcp_tags".to_string(),
        model: "m".to_string(),
        surface: rupu_coverage::Surface::Workflow,
        options: rupu_coverage::FindingWriteOptions::default()
            .with_profile(rupu_coverage::FindingProfile::Summary),
        codename: Some("jade-reef".to_string()),
        provider: None,
    }
}

async fn record(d: &ToolDispatcher, summary: &str) -> String {
    let out = d
        .call(
            "findings.record",
            serde_json::json!({"scope": "repo", "summary": summary, "severity": "high", "rationale": "r"}),
        )
        .await
        .unwrap();
    out.trim_start_matches("finding_id: ").to_string()
}

#[tokio::test]
async fn tag_then_query_by_tag() {
    let tmp = tempfile::TempDir::new().unwrap();
    let d = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx(tmp.path()));
    let a = record(&d, "first").await;
    let _b = record(&d, "second").await;

    let out = d
        .call(
            "findings.tag",
            serde_json::json!({"finding_ids": [a], "add": ["Needs-POC"]}),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["outcomes"][0]["after"], serde_json::json!(["needs-poc"]));

    let out = d
        .call("findings.query", serde_json::json!({"q": "tag:needs-poc"}))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["total"], 1);
    assert_eq!(v["rows"][0]["id"], a);

    let events =
        rupu_coverage::read_tag_events(&rupu_coverage::TagLog::for_workspace(tmp.path())).unwrap();
    match &events[0].by {
        rupu_coverage::TagActor::Agent(at) => {
            assert_eq!(at.run_id, "run_mcp_tags");
            assert_eq!(at.agent, None);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn readonly_mode_blocks_findings_tag_but_not_query() {
    let tmp = tempfile::TempDir::new().unwrap();
    let d = ToolDispatcher::new(
        Arc::new(Registry::default()),
        McpPermission::new(PermissionMode::Readonly, vec!["*".into()]),
    )
    .with_findings(ctx(tmp.path()));
    let err = d
        .call(
            "findings.tag",
            serde_json::json!({"finding_ids": ["fnd_x"], "add": ["x"]}),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("readonly"), "{err}");
    assert!(d
        .call("findings.query", serde_json::json!({}))
        .await
        .is_ok());
}

#[tokio::test]
async fn without_run_context_the_tools_refuse() {
    let d = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all());
    let err = d
        .call("findings.query", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unavailable"), "{err}");
}
