use assert_fs::prelude::*;
use rupu_tools::{DerivedEvent, EditFileTool, Tool, ToolContext};
use serde_json::json;

fn ctx(workspace: &std::path::Path) -> ToolContext {
    ToolContext {
        workspace_path: workspace.to_path_buf(),
        ..Default::default()
    }
}

#[tokio::test]
async fn replaces_exact_match() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("src.txt").write_str("foo\nbar\nbaz\n").unwrap();
    let out = EditFileTool
        .invoke(
            json!({ "path": "src.txt", "old_string": "bar\n", "new_string": "BAR\n" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert!(out.error.is_none(), "edit failed: {:?}", out.error);
    tmp.child("src.txt").assert("foo\nBAR\nbaz\n");
    let DerivedEvent::FileEdit { kind, diff, .. } = out.derived.unwrap() else {
        panic!()
    };
    assert_eq!(kind, "modify");
    assert!(diff.contains("--- a/src.txt"));
    assert!(diff.contains("+++ b/src.txt"));
    assert!(diff.contains("-bar"));
    assert!(diff.contains("+BAR"));
}

#[tokio::test]
async fn fails_when_old_string_not_found() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("src.txt").write_str("foo\n").unwrap();
    let out = EditFileTool
        .invoke(
            json!({ "path": "src.txt", "old_string": "missing", "new_string": "x" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert!(out.error.is_some());
    tmp.child("src.txt").assert("foo\n"); // unchanged
}

#[tokio::test]
async fn fails_when_old_string_is_ambiguous() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("src.txt").write_str("dup\ndup\n").unwrap();
    let out = EditFileTool
        .invoke(
            json!({ "path": "src.txt", "old_string": "dup", "new_string": "x" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert!(out.error.is_some(), "ambiguous match must error");
}

#[tokio::test]
async fn refuses_path_outside_workspace() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let out = EditFileTool
        .invoke(
            json!({ "path": "../etc/x", "old_string": "a", "new_string": "b" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert!(out.error.is_some());
}
