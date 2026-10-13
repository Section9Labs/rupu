use assert_fs::prelude::*;
use rupu_tools::{DerivedEvent, EditFileTool, Tool, ToolContext};
use serde_json::json;

fn ctx(workspace: &std::path::Path) -> ToolContext {
    ToolContext::in_workspace(workspace)
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

#[tokio::test]
async fn a_dotdot_through_a_missing_dir_cannot_escape() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let ws = tmp.child("ws");
    ws.create_dir_all().unwrap();
    tmp.child("outside/f").write_str("original").unwrap();
    let out = EditFileTool
        .invoke(
            json!({ "path": "nope/../../outside/f", "old_string": "original", "new_string": "pwned" }),
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
    tmp.child("file.txt").write_str("old").unwrap();
    let out = EditFileTool
        .invoke(
            json!({ "path": "sub/../file.txt", "old_string": "old", "new_string": "new" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert_eq!(out.error, None);
    tmp.child("file.txt").assert("new");
}
