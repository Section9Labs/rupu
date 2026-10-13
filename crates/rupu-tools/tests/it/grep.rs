use assert_fs::prelude::*;
use rupu_tools::{GrepTool, Tool, ToolContext};
use serde_json::json;

fn ctx(workspace: &std::path::Path) -> ToolContext {
    ToolContext::in_workspace(workspace)
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

/// The read tools open the path their scope check resolved; what they
/// report is still spelled under the workspace path as given, here a link.
#[cfg(unix)]
#[tokio::test]
async fn a_linked_workspace_reports_paths_as_given() {
    if skip_if_no_rg() {
        eprintln!("skipping: ripgrep not installed");
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir_all(real.join("src")).unwrap();
    std::fs::write(real.join("src/a.txt"), "needle\n").unwrap();
    let ws = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &ws).unwrap();

    let grep = GrepTool
        .invoke(json!({ "pattern": "needle", "path": "src" }), &ctx(&ws))
        .await
        .unwrap();
    assert_eq!(
        grep.stdout,
        format!("{}:1:needle\n", ws.join("src/a.txt").display())
    );
    let glob = rupu_tools::GlobTool
        .invoke(json!({ "pattern": "**/*.txt" }), &ctx(&ws))
        .await
        .unwrap();
    assert_eq!(glob.stdout, "src/a.txt");
    let read = rupu_tools::ReadFileTool
        .invoke(json!({ "path": "src/a.txt" }), &ctx(&ws))
        .await
        .unwrap();
    assert_eq!(read.stdout, "1\tneedle\n");
}
