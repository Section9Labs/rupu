use assert_fs::prelude::*;
use rupu_tools::{ReadFileTool, Tool, ToolContext};
use serde_json::json;

fn ctx(workspace: &std::path::Path) -> ToolContext {
    ToolContext {
        workspace_path: workspace.to_path_buf(),
        ..Default::default()
    }
}

#[tokio::test]
async fn reads_file_with_line_numbers() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let f = tmp.child("hello.txt");
    f.write_str("first\nsecond\nthird\n").unwrap();

    let tool = ReadFileTool;
    let out = tool
        .invoke(json!({ "path": "hello.txt" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.stdout.contains("1\tfirst"));
    assert!(out.stdout.contains("2\tsecond"));
    assert!(out.stdout.contains("3\tthird"));
    assert!(out.error.is_none());
}

#[tokio::test]
async fn missing_file_returns_error() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let tool = ReadFileTool;
    let out = tool
        .invoke(json!({ "path": "nope.txt" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.error.is_some(), "expected error for missing file");
}

#[tokio::test]
async fn rejects_path_outside_workspace() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let tool = ReadFileTool;
    let out = tool
        .invoke(json!({ "path": "../etc/passwd" }), &ctx(tmp.path()))
        .await;
    // Either err or output-with-error is acceptable; not allowed to read.
    let invalid = match out {
        Err(_) => true,
        Ok(o) => o.error.is_some(),
    };
    assert!(invalid, "must refuse paths escaping workspace");
}

#[tokio::test]
async fn missing_path_input_is_invalid() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let tool = ReadFileTool;
    let res = tool.invoke(json!({}), &ctx(tmp.path())).await;
    assert!(res.is_err());
}
