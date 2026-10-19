//! The catalog as a tool source: workspace-scoped contexts per call.

use rupu_mcp::catalog::{CatalogSource, ContextFactory, HIDDEN_BY_OPERATOR_TOOLS};
use rupu_mcp::{CallCx, ToolResult, ToolSource};
use rupu_tools::ToolContext;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// Maps any workspace string to `root`; `"missing"` is an error.
pub struct TestFactory {
    pub root: PathBuf,
}

impl ContextFactory for TestFactory {
    fn context(&self, workspace: Option<&str>) -> Result<ToolContext, String> {
        if workspace == Some("missing") {
            return Err("no such project: missing".into());
        }
        let mut ctx = ToolContext::in_workspace(&self.root);
        ctx.services.scm = Some(Arc::new(rupu_scm::Registry::default()));
        ctx.services.findings = Some(rupu_coverage::FindingWriteOptions::default());
        let id = ctx.identity_mut();
        id.run_id = "mcp_test".into();
        id.scope_name = Some("mcp".into());
        Ok(ctx)
    }
}

pub fn source(root: &std::path::Path) -> CatalogSource {
    CatalogSource::new(Arc::new(TestFactory {
        root: root.to_path_buf(),
    }))
    .expect("catalog source")
}

fn required(def: &rupu_mcp::ToolDef) -> Vec<String> {
    def.input_schema["required"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn operator_named_tools_are_not_listed() {
    let tmp = tempfile::TempDir::new().unwrap();
    let names: Vec<String> = source(tmp.path())
        .tools()
        .into_iter()
        .map(|t| t.name)
        .collect();
    for hidden in HIDDEN_BY_OPERATOR_TOOLS {
        assert!(!names.iter().any(|n| n == hidden), "{hidden} listed");
    }
    assert!(names.iter().any(|n| n == "read_file"));
}

#[test]
fn fs_tools_require_a_workspace_argument() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tools = source(tmp.path()).tools();
    let read = tools.iter().find(|t| t.name == "read_file").unwrap();
    assert!(required(read).contains(&"workspace".to_string()));
    assert_eq!(
        read.input_schema["properties"]["workspace"]["type"],
        "string"
    );
    let repos = tools.iter().find(|t| t.name == "scm.repos.list").unwrap();
    assert!(!required(repos).contains(&"workspace".to_string()));
    assert!(repos.input_schema["properties"].get("workspace").is_none());
}

#[tokio::test]
async fn read_file_reads_inside_the_named_workspace() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("hello.txt"), "hello from p1").unwrap();
    let r = source(tmp.path())
        .call(
            "read_file",
            json!({"workspace": "p1", "path": "hello.txt"}),
            &CallCx::default(),
        )
        .await
        .expect("served");
    match r {
        ToolResult::Text(t) => assert!(t.contains("hello from p1"), "{t}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_unknown_workspace_is_a_tool_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let r = source(tmp.path())
        .call(
            "read_file",
            json!({"workspace": "missing", "path": "x"}),
            &CallCx::default(),
        )
        .await
        .expect("served");
    match r {
        ToolResult::Error(t) => assert!(t.contains("no such project: missing"), "{t}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_missing_workspace_argument_is_a_tool_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let r = source(tmp.path())
        .call("read_file", json!({"path": "x"}), &CallCx::default())
        .await
        .expect("served");
    assert!(matches!(r, ToolResult::Error(t) if t.contains("workspace")));
}

#[tokio::test]
async fn an_unserved_name_is_none() {
    let tmp = tempfile::TempDir::new().unwrap();
    let s = source(tmp.path());
    assert!(s
        .call("nope", json!({}), &CallCx::default())
        .await
        .is_none());
    assert!(s
        .call(
            "findings.query",
            json!({"workspace": "p"}),
            &CallCx::default()
        )
        .await
        .is_none());
}

#[tokio::test]
async fn findings_record_alias_records_through_the_one_findings_tool() {
    let tmp = tempfile::TempDir::new().unwrap();
    let report: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap();
    let s = source(tmp.path());
    let r = s
        .call(
            "findings.record",
            json!({"workspace": "p1", "scope": "repo", "report": report}),
            &CallCx::default(),
        )
        .await
        .expect("alias served");
    match r {
        ToolResult::Text(t) => assert!(t.starts_with("finding_id: fnd_"), "{t}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn hints_follow_the_effect() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tools = source(tmp.path()).tools();
    let read = tools.iter().find(|t| t.name == "read_file").unwrap();
    assert!(read.hints.read_only);
    for name in ["bash", "write_file"] {
        let t = tools.iter().find(|t| t.name == name).unwrap();
        assert!(t.hints.destructive && !t.hints.read_only, "{name}");
    }
    let comment = tools.iter().find(|t| t.name == "scm.prs.comment").unwrap();
    assert!(comment.hints.open_world && !comment.hints.destructive);
}

struct NoScm(TestFactory);

impl ContextFactory for NoScm {
    fn context(&self, ws: Option<&str>) -> Result<ToolContext, String> {
        let mut ctx = self.0.context(ws)?;
        ctx.services.scm = None;
        Ok(ctx)
    }
}

#[test]
fn tools_missing_a_service_are_reported_not_dropped() {
    let tmp = tempfile::TempDir::new().unwrap();
    let s = CatalogSource::new(Arc::new(NoScm(TestFactory {
        root: tmp.path().to_path_buf(),
    })))
    .unwrap();
    assert!(s.unavailable().iter().any(|u| u.tool == "scm.prs.comment"));
    assert!(!s.tools().iter().any(|t| t.name == "scm.prs.comment"));
    // A fully served catalog reports no connector tool missing.
    assert!(!source(tmp.path())
        .unavailable()
        .iter()
        .any(|u| u.tool == "scm.prs.comment"));
}
