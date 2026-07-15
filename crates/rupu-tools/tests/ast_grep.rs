use assert_fs::prelude::*;
use rupu_tools::{AstGrepTool, Tool, ToolContext};
use serde_json::json;

fn ctx(workspace: &std::path::Path) -> ToolContext {
    ToolContext {
        workspace_path: workspace.to_path_buf(),
        ..Default::default()
    }
}

fn skip_if_no_ast_grep() -> bool {
    which::which("ast-grep").is_err()
}

#[tokio::test]
async fn finds_structural_matches() {
    if skip_if_no_ast_grep() {
        eprintln!("skipping: ast-grep not installed");
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("x.rs")
        .write_str("fn main() {\n    println!(\"hi\");\n}\nfn helper() {}\n")
        .unwrap();
    let out = AstGrepTool
        .invoke(
            json!({ "pattern": "fn $NAME() { $$$ }", "lang": "rust" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
    // Compact grep-style, workspace-relative path, 1-based line/col.
    assert!(
        out.stdout.contains("x.rs:1:1:"),
        "stdout was: {}",
        out.stdout
    );
    assert!(
        out.stdout.contains("x.rs:4:1:"),
        "stdout was: {}",
        out.stdout
    );
    // Absolute paths must be stripped to workspace-relative.
    assert!(!out.stdout.contains(tmp.path().to_str().unwrap()));
}

#[tokio::test]
async fn no_matches_returns_empty_stdout() {
    if skip_if_no_ast_grep() {
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("x.rs").write_str("fn main() {}\n").unwrap();
    let out = AstGrepTool
        .invoke(
            json!({ "pattern": "struct $X {}", "lang": "rust" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert!(out.stdout.is_empty(), "stdout was: {}", out.stdout);
    assert!(out.error.is_none());
}

#[tokio::test]
async fn missing_pattern_is_invalid_input() {
    if skip_if_no_ast_grep() {
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    let res = AstGrepTool
        .invoke(json!({ "lang": "rust" }), &ctx(tmp.path()))
        .await;
    assert!(res.is_err());
}

#[tokio::test]
async fn missing_lang_is_invalid_input() {
    if skip_if_no_ast_grep() {
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    let res = AstGrepTool
        .invoke(json!({ "pattern": "fn $N() { $$$ }" }), &ctx(tmp.path()))
        .await;
    assert!(res.is_err());
}
