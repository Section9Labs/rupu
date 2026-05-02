use assert_fs::prelude::*;
use rupu_tools::{DerivedEvent, Tool, ToolContext, WriteFileTool};
use serde_json::json;

fn ctx(workspace: &std::path::Path) -> ToolContext {
    ToolContext {
        workspace_path: workspace.to_path_buf(),
        ..Default::default()
    }
}

#[tokio::test]
async fn creates_new_file_and_emits_create_derived() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let out = WriteFileTool
        .invoke(
            json!({ "path": "new.txt", "content": "hello\n" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert!(out.error.is_none());
    tmp.child("new.txt").assert("hello\n");
    let derived = out.derived.unwrap();
    let DerivedEvent::FileEdit { kind, .. } = derived else {
        panic!("expected FileEdit derived");
    };
    assert_eq!(kind, "create");
}

#[tokio::test]
async fn overwrites_existing_file_and_emits_modify_derived() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("x.txt").write_str("old\n").unwrap();
    let out = WriteFileTool
        .invoke(
            json!({ "path": "x.txt", "content": "new\n" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    tmp.child("x.txt").assert("new\n");
    let DerivedEvent::FileEdit { kind, .. } = out.derived.unwrap() else {
        panic!()
    };
    assert_eq!(kind, "modify");
}

#[tokio::test]
async fn refuses_path_outside_workspace() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let out = WriteFileTool
        .invoke(
            json!({ "path": "../escape.txt", "content": "x" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert!(out.error.is_some());
}

#[tokio::test]
async fn creates_intermediate_directories() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let out = WriteFileTool
        .invoke(
            json!({ "path": "a/b/c.txt", "content": "x" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert!(out.error.is_none());
    tmp.child("a/b/c.txt").assert("x");
}
