use assert_fs::prelude::*;
use rupu_tools::{GrepTool, Tool, ToolContext};
use serde_json::json;

fn ctx(workspace: &std::path::Path) -> ToolContext {
    ToolContext {
        workspace_path: workspace.to_path_buf(),
        ..Default::default()
    }
}

fn skip_if_no_rg() -> bool {
    which::which("rg").is_err()
}

#[tokio::test]
async fn finds_matches_across_files() {
    if skip_if_no_rg() {
        eprintln!("skipping: ripgrep not installed");
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("a.txt").write_str("foo bar\n").unwrap();
    tmp.child("b.txt").write_str("baz qux\n").unwrap();
    let out = GrepTool
        .invoke(json!({ "pattern": "bar" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.error.is_none());
    assert!(out.stdout.contains("a.txt"));
    assert!(out.stdout.contains("foo bar"));
    assert!(!out.stdout.contains("b.txt"));
}

#[tokio::test]
async fn no_matches_returns_empty_stdout() {
    if skip_if_no_rg() {
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("a.txt").write_str("foo\n").unwrap();
    let out = GrepTool
        .invoke(json!({ "pattern": "xyz" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.stdout.is_empty());
    assert!(out.error.is_none());
}

#[tokio::test]
async fn invalid_input_errors() {
    if skip_if_no_rg() {
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    let res = GrepTool.invoke(json!({}), &ctx(tmp.path())).await;
    assert!(res.is_err());
}
