//! Integration tests verifying that file-touching built-in tools emit
//! `FileTouchEvent`s to the `CoverageWriter` when one is present on
//! `ToolContext`.
//!
//! Each test:
//!   1. Creates a temporary workspace with a known file layout.
//!   2. Spawns a `CoverageWriterHandle` backed by a temp directory.
//!   3. Calls the tool under test with a `ToolContext` that has the
//!      writer wired in.
//!   4. Drops the `ToolContext` so the cloned `Arc<CoverageWriter>` is
//!      released before shutdown (the writer task exits when ALL sender
//!      channels are dropped — keeping an Arc alive past `shutdown()` would
//!      cause `task.await` to hang forever).
//!   5. Shuts down the writer (flushes all pending writes).
//!   6. Reads `files.jsonl` and asserts the expected events landed.

use rupu_coverage::{CoveragePaths, CoverageWriterHandle, FileTouchEvent};
use rupu_tools::{BashTool, EditFileTool, GlobTool, GrepTool, ReadFileTool, Tool, ToolContext};
use serde_json::json;
use std::sync::Arc;

/// Build a `ToolContext` pointed at `workspace` with the given coverage writer.
fn ctx_with_coverage(
    workspace: &std::path::Path,
    writer: Arc<rupu_coverage::CoverageWriter>,
) -> ToolContext {
    ToolContext {
        workspace_path: workspace.to_path_buf(),
        coverage_writer: Some(writer),
        surface_tag: Some("agent".to_string()),
        run_id: Some("run_test_01".to_string()),
        model: Some("test-model".to_string()),
        ..Default::default()
    }
}

/// Read all `FileTouchEvent`s from the `files.jsonl` ledger.
fn read_events(paths: &CoveragePaths) -> Vec<FileTouchEvent> {
    let body = std::fs::read_to_string(&paths.files).unwrap_or_default();
    body.lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("valid FileTouchEvent JSON"))
        .collect()
}

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_file_emits_read_event() {
    let workspace = tempfile::TempDir::new().unwrap();
    std::fs::write(workspace.path().join("hello.txt"), "line1\nline2\nline3\n").unwrap();

    let ledger = tempfile::TempDir::new().unwrap();
    let paths = CoveragePaths::new(ledger.path(), "test-target");
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();

    // Scope the ctx so the Arc<CoverageWriter> clone is dropped before shutdown.
    {
        let ctx = ctx_with_coverage(workspace.path(), handle.writer.clone());
        let tool = ReadFileTool;
        let out = tool
            .invoke(json!({ "path": "hello.txt" }), &ctx)
            .await
            .unwrap();
        assert!(out.error.is_none(), "read should succeed: {out:?}");
    } // ctx (and its Arc<CoverageWriter>) dropped here

    handle.shutdown().await;

    let events = read_events(&paths);
    assert_eq!(events.len(), 1, "expected one read event; got: {events:?}");
    match &events[0] {
        FileTouchEvent::Read {
            path,
            line_range,
            tool,
            ..
        } => {
            assert_eq!(path, "hello.txt");
            assert_eq!(tool, "read_file");
            assert_eq!(line_range[0], 1);
            assert_eq!(line_range[1], 3); // 3 lines
        }
        other => panic!("expected Read event, got: {other:?}"),
    }
}

#[tokio::test]
async fn read_file_no_event_on_missing_file() {
    let workspace = tempfile::TempDir::new().unwrap();

    let ledger = tempfile::TempDir::new().unwrap();
    let paths = CoveragePaths::new(ledger.path(), "test-target");
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();

    {
        let ctx = ctx_with_coverage(workspace.path(), handle.writer.clone());
        let tool = ReadFileTool;
        let out = tool
            .invoke(json!({ "path": "no_such_file.txt" }), &ctx)
            .await
            .unwrap();
        assert!(out.error.is_some(), "missing file should produce error");
    }

    handle.shutdown().await;

    let events = read_events(&paths);
    assert!(
        events.is_empty(),
        "no events should be emitted on error path; got: {events:?}"
    );
}

#[tokio::test]
async fn read_file_no_event_without_coverage_writer() {
    let workspace = tempfile::TempDir::new().unwrap();
    std::fs::write(workspace.path().join("hi.txt"), "hello\n").unwrap();

    // ToolContext without a coverage_writer — confirm the tool still works.
    let ctx = ToolContext {
        workspace_path: workspace.path().to_path_buf(),
        ..Default::default()
    };
    let tool = ReadFileTool;
    let out = tool
        .invoke(json!({ "path": "hi.txt" }), &ctx)
        .await
        .unwrap();
    assert!(out.error.is_none());
    assert!(out.stdout.contains("hello"));
}

// ---------------------------------------------------------------------------
// edit_file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn edit_file_emits_edit_event() {
    let workspace = tempfile::TempDir::new().unwrap();
    std::fs::write(
        workspace.path().join("src.rs"),
        "fn main() {\n    println!(\"hello\");\n}\n",
    )
    .unwrap();

    let ledger = tempfile::TempDir::new().unwrap();
    let paths = CoveragePaths::new(ledger.path(), "test-target");
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();

    {
        let ctx = ctx_with_coverage(workspace.path(), handle.writer.clone());
        let tool = EditFileTool;
        let out = tool
            .invoke(
                json!({
                    "path": "src.rs",
                    "old_string": "println!(\"hello\");",
                    "new_string": "println!(\"world\");"
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.error.is_none(), "edit should succeed: {out:?}");
    }

    handle.shutdown().await;

    let events = read_events(&paths);
    assert_eq!(events.len(), 1, "expected one edit event; got: {events:?}");
    match &events[0] {
        FileTouchEvent::Edit { path, tool, .. } => {
            assert_eq!(path, "src.rs");
            assert_eq!(tool, "edit_file");
        }
        other => panic!("expected Edit event, got: {other:?}"),
    }
}

#[tokio::test]
async fn edit_file_no_event_on_not_found() {
    let workspace = tempfile::TempDir::new().unwrap();
    std::fs::write(workspace.path().join("f.rs"), "fn foo() {}\n").unwrap();

    let ledger = tempfile::TempDir::new().unwrap();
    let paths = CoveragePaths::new(ledger.path(), "test-target");
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();

    {
        let ctx = ctx_with_coverage(workspace.path(), handle.writer.clone());
        let tool = EditFileTool;
        let out = tool
            .invoke(
                json!({
                    "path": "f.rs",
                    "old_string": "DOES_NOT_EXIST",
                    "new_string": "replacement"
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.error.is_some(), "non-matching edit should produce error");
    }

    handle.shutdown().await;

    let events = read_events(&paths);
    assert!(
        events.is_empty(),
        "no events on error path; got: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// glob
// ---------------------------------------------------------------------------

#[tokio::test]
async fn glob_emits_glob_events() {
    let workspace = tempfile::TempDir::new().unwrap();
    std::fs::write(workspace.path().join("a.rs"), "").unwrap();
    std::fs::write(workspace.path().join("b.rs"), "").unwrap();
    std::fs::write(workspace.path().join("c.txt"), "").unwrap();

    let ledger = tempfile::TempDir::new().unwrap();
    let paths = CoveragePaths::new(ledger.path(), "test-target");
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();

    {
        let ctx = ctx_with_coverage(workspace.path(), handle.writer.clone());
        let tool = GlobTool;
        let out = tool
            .invoke(json!({ "pattern": "*.rs" }), &ctx)
            .await
            .unwrap();
        assert!(out.error.is_none(), "glob should succeed: {out:?}");
    }

    handle.shutdown().await;

    let events = read_events(&paths);
    // Two .rs files should each emit a Glob event.
    assert_eq!(events.len(), 2, "expected 2 glob events; got: {events:?}");
    for ev in &events {
        match ev {
            FileTouchEvent::Glob {
                path,
                pattern,
                tool,
                ..
            } => {
                assert!(
                    path.ends_with(".rs"),
                    "glob event path should be a .rs file: {path}"
                );
                assert_eq!(pattern, "*.rs");
                assert_eq!(tool, "glob");
            }
            other => panic!("expected Glob event, got: {other:?}"),
        }
    }
}

#[tokio::test]
async fn glob_no_events_when_nothing_matches() {
    let workspace = tempfile::TempDir::new().unwrap();
    std::fs::write(workspace.path().join("readme.txt"), "").unwrap();

    let ledger = tempfile::TempDir::new().unwrap();
    let paths = CoveragePaths::new(ledger.path(), "test-target");
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();

    {
        let ctx = ctx_with_coverage(workspace.path(), handle.writer.clone());
        let tool = GlobTool;
        let out = tool
            .invoke(json!({ "pattern": "*.rs" }), &ctx)
            .await
            .unwrap();
        assert!(out.error.is_none());
    }

    handle.shutdown().await;

    let events = read_events(&paths);
    assert!(
        events.is_empty(),
        "no events when no files match; got: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// grep
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grep_emits_grep_events_for_matched_files() {
    // Guard: skip gracefully if `rg` is not on PATH.
    if std::process::Command::new("rg")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("SKIP grep_emits_grep_events_for_matched_files: `rg` not found in PATH");
        return;
    }

    let workspace = tempfile::TempDir::new().unwrap();
    // file_a contains the pattern; file_b does not.
    std::fs::write(
        workspace.path().join("file_a.txt"),
        "hello world\nrust is great\n",
    )
    .unwrap();
    std::fs::write(workspace.path().join("file_b.txt"), "no match here\n").unwrap();

    let ledger = tempfile::TempDir::new().unwrap();
    let paths = CoveragePaths::new(ledger.path(), "test-target");
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();

    {
        let ctx = ctx_with_coverage(workspace.path(), handle.writer.clone());
        let tool = GrepTool;
        let out = tool
            .invoke(json!({ "pattern": "hello" }), &ctx)
            .await
            .unwrap();
        assert!(out.error.is_none(), "grep should succeed: {out:?}");
        assert!(
            out.stdout.contains("file_a.txt"),
            "stdout should mention the matching file"
        );
    }

    handle.shutdown().await;

    let events = read_events(&paths);
    assert_eq!(
        events.len(),
        1,
        "expected exactly one Grep event (file_a only); got: {events:?}"
    );
    match &events[0] {
        FileTouchEvent::Grep {
            path,
            pattern,
            match_count,
            tool,
            ..
        } => {
            assert_eq!(path, "file_a.txt");
            assert_eq!(pattern, "hello");
            assert_eq!(*match_count, 1, "one matching line");
            assert_eq!(tool, "grep");
        }
        other => panic!("expected Grep event, got: {other:?}"),
    }
}

#[tokio::test]
async fn grep_no_events_when_nothing_matches() {
    // Guard: skip gracefully if `rg` is not on PATH.
    if std::process::Command::new("rg")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("SKIP grep_no_events_when_nothing_matches: `rg` not found in PATH");
        return;
    }

    let workspace = tempfile::TempDir::new().unwrap();
    std::fs::write(workspace.path().join("only.txt"), "no relevant content\n").unwrap();

    let ledger = tempfile::TempDir::new().unwrap();
    let paths = CoveragePaths::new(ledger.path(), "test-target");
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();

    {
        let ctx = ctx_with_coverage(workspace.path(), handle.writer.clone());
        let tool = GrepTool;
        let out = tool
            .invoke(json!({ "pattern": "PATTERN_THAT_WONT_MATCH_ANYTHING" }), &ctx)
            .await
            .unwrap();
        // Exit code 1 (no matches) is NOT an error for rg.
        assert!(out.error.is_none(), "no-match grep should succeed: {out:?}");
    }

    handle.shutdown().await;

    let events = read_events(&paths);
    assert!(
        events.is_empty(),
        "no events expected when pattern doesn't match; got: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// bash
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bash_emits_cmd_events_for_workspace_path_args() {
    let workspace = tempfile::TempDir::new().unwrap();
    std::fs::write(workspace.path().join("target.txt"), "some content\n").unwrap();

    let ledger = tempfile::TempDir::new().unwrap();
    let paths = CoveragePaths::new(ledger.path(), "test-target");
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();

    {
        let ctx = ctx_with_coverage(workspace.path(), handle.writer.clone());
        let tool = BashTool;
        let out = tool
            .invoke(json!({ "command": "wc -l target.txt" }), &ctx)
            .await
            .unwrap();
        assert!(out.error.is_none(), "bash should succeed: {out:?}");
    }

    handle.shutdown().await;

    let events = read_events(&paths);
    assert_eq!(
        events.len(),
        1,
        "expected one Cmd event for target.txt; got: {events:?}"
    );
    match &events[0] {
        FileTouchEvent::Cmd {
            path,
            command,
            tool,
            ..
        } => {
            assert_eq!(path, "target.txt");
            assert_eq!(command, "wc -l target.txt");
            assert_eq!(tool, "bash");
        }
        other => panic!("expected Cmd event, got: {other:?}"),
    }
}

#[tokio::test]
async fn bash_skips_flag_tokens() {
    let workspace = tempfile::TempDir::new().unwrap();

    let ledger = tempfile::TempDir::new().unwrap();
    let paths = CoveragePaths::new(ledger.path(), "test-target");
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();

    {
        let ctx = ctx_with_coverage(workspace.path(), handle.writer.clone());
        let tool = BashTool;
        // `ls -la` — `-la` starts with `-` so should be skipped; `ls` is not a
        // file in the workspace, so no Cmd event should be emitted at all.
        let out = tool
            .invoke(json!({ "command": "ls -la" }), &ctx)
            .await
            .unwrap();
        assert!(out.error.is_none(), "bash should succeed: {out:?}");
    }

    handle.shutdown().await;

    let events = read_events(&paths);
    assert!(
        events.is_empty(),
        "flag tokens and non-file tokens should not emit Cmd events; got: {events:?}"
    );
}

#[tokio::test]
async fn bash_no_events_for_non_file_tokens() {
    let workspace = tempfile::TempDir::new().unwrap();

    let ledger = tempfile::TempDir::new().unwrap();
    let paths = CoveragePaths::new(ledger.path(), "test-target");
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();

    {
        let ctx = ctx_with_coverage(workspace.path(), handle.writer.clone());
        let tool = BashTool;
        // `echo hello` — neither token is an existing workspace file.
        let out = tool
            .invoke(json!({ "command": "echo hello" }), &ctx)
            .await
            .unwrap();
        assert!(out.error.is_none(), "bash should succeed: {out:?}");
        assert!(
            out.stdout.contains("hello"),
            "echo output should contain 'hello'"
        );
    }

    handle.shutdown().await;

    let events = read_events(&paths);
    assert!(
        events.is_empty(),
        "non-file tokens should produce no Cmd events; got: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// Attribution fields
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_file_event_carries_attribution() {
    let workspace = tempfile::TempDir::new().unwrap();
    std::fs::write(workspace.path().join("x.txt"), "content\n").unwrap();

    let ledger = tempfile::TempDir::new().unwrap();
    let paths = CoveragePaths::new(ledger.path(), "test-target");
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();

    {
        let ctx = ctx_with_coverage(workspace.path(), handle.writer.clone());
        let tool = ReadFileTool;
        tool.invoke(json!({ "path": "x.txt" }), &ctx)
            .await
            .unwrap();
    }

    handle.shutdown().await;

    let body = std::fs::read_to_string(&paths.files).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(body.lines().next().unwrap()).unwrap();
    assert_eq!(v["run_id"], "run_test_01");
    assert_eq!(v["model"], "test-model");
    assert_eq!(v["surface"], "agent");
}
