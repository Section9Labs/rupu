use assert_fs::prelude::*;
use rupu_tools::{DerivedEvent, Tool, ToolContext, WriteFileTool};
use serde_json::json;

fn ctx(workspace: &std::path::Path) -> ToolContext {
    ToolContext::in_workspace(workspace)
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
    let DerivedEvent::FileEdit { kind, diff, .. } = derived else {
        panic!("expected FileEdit derived");
    };
    assert_eq!(kind, "create");
    assert!(diff.contains("diff --git a/new.txt b/new.txt"));
    assert!(diff.contains("+++ b/new.txt"));
    assert!(diff.contains("+hello"));
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
    let DerivedEvent::FileEdit { kind, diff, .. } = out.derived.unwrap() else {
        panic!()
    };
    assert_eq!(kind, "modify");
    assert!(diff.contains("--- a/x.txt"));
    assert!(diff.contains("+++ b/x.txt"));
    assert!(diff.contains("-old"));
    assert!(diff.contains("+new"));
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

/// `nope/../../outside/f`: `nope` does not exist, so a check that stops at
/// the nearest existing ancestor sees the workspace, while `create_dir_all`
/// would create `nope` and the OS would then resolve `..` out of it.
#[tokio::test]
async fn a_dotdot_through_a_missing_dir_cannot_escape() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let ws = tmp.child("ws");
    ws.create_dir_all().unwrap();
    tmp.child("outside/f").write_str("original").unwrap();
    let out = WriteFileTool
        .invoke(
            json!({ "path": "nope/../../outside/f", "content": "pwned" }),
            &ctx(ws.path()),
        )
        .await
        .unwrap();
    assert!(
        out.error
            .as_deref()
            .unwrap_or_default()
            .contains("escapes workspace"),
        "{:?}",
        out.error
    );
    assert!(
        !ws.path().join("nope").exists(),
        "the refusal created ws/nope"
    );
    tmp.child("outside/f").assert("original");
}

#[tokio::test]
async fn a_dotdot_through_an_existing_dir_stays_allowed() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("sub").create_dir_all().unwrap();
    let out = WriteFileTool
        .invoke(
            json!({ "path": "sub/../file.txt", "content": "x" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert_eq!(out.error, None);
    tmp.child("file.txt").assert("x");
}

/// A dangling symlink does not "exist", so a walk to the nearest existing
/// ancestor skipped over it and `write` followed it out of the workspace.
#[cfg(unix)]
#[tokio::test]
async fn a_dangling_symlink_cannot_be_written_through() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let ws = tmp.child("ws");
    ws.create_dir_all().unwrap();
    let target = tmp.path().join("outside.txt");
    std::os::unix::fs::symlink(&target, ws.path().join("link")).unwrap();
    let out = WriteFileTool
        .invoke(
            json!({ "path": "link", "content": "pwned" }),
            &ctx(ws.path()),
        )
        .await
        .unwrap();
    assert!(
        out.error
            .as_deref()
            .unwrap_or_default()
            .contains("escapes workspace"),
        "{:?}",
        out.error
    );
    assert!(!target.exists(), "the write followed the link out");
}
