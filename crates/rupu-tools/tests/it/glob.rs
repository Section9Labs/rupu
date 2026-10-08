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

#[tokio::test]
async fn glob_stays_in_workspace() {
    // W1 T17: a pattern that climbs out of the workspace — relative, absolute
    // or through a symlinked directory — yields nothing outside it, as the
    // other fs tools refuse such paths.
    let outer = assert_fs::TempDir::new().unwrap();
    outer.child("secret.txt").write_str("s").unwrap();
    outer.child("ws/inside.txt").write_str("").unwrap();
    std::os::unix::fs::symlink(outer.path(), outer.path().join("ws/up")).unwrap();
    let ws = outer.path().join("ws");
    let absolute = format!("{}/*", outer.path().display());
    for pattern in ["../*", "../**/*", "**/../../*", "up/*", absolute.as_str()] {
        let out = GlobTool
            .invoke(json!({ "pattern": pattern }), &ctx(&ws))
            .await
            .unwrap();
        assert!(
            !out.stdout.contains("secret.txt"),
            "{pattern} escaped the workspace: {}",
            out.stdout
        );
    }
}
