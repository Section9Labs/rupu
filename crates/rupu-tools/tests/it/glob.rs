use assert_fs::prelude::*;
use rupu_tools::{GlobTool, Tool, ToolContext};
use serde_json::json;

fn ctx(workspace: &std::path::Path) -> ToolContext {
    ToolContext {
        workspace_path: workspace.to_path_buf(),
        ..Default::default()
    }
}

#[tokio::test]
async fn matches_files_by_pattern() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("a.rs").write_str("").unwrap();
    tmp.child("b.rs").write_str("").unwrap();
    tmp.child("c.txt").write_str("").unwrap();
    let out = GlobTool
        .invoke(json!({ "pattern": "*.rs" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.error.is_none());
    assert!(out.stdout.contains("a.rs"));
    assert!(out.stdout.contains("b.rs"));
    assert!(!out.stdout.contains("c.txt"));
}

#[tokio::test]
async fn matches_recursively_with_double_star() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("src/lib.rs").write_str("").unwrap();
    tmp.child("src/mod/x.rs").write_str("").unwrap();
    let out = GlobTool
        .invoke(json!({ "pattern": "**/*.rs" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.stdout.contains("src/lib.rs"));
    assert!(out.stdout.contains("src/mod/x.rs"));
}

#[tokio::test]
async fn no_matches_returns_empty() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let out = GlobTool
        .invoke(json!({ "pattern": "*.zzz" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.stdout.is_empty());
    assert!(out.error.is_none());
}
