use rupu_tools::{BashTool, DerivedEvent, Tool, ToolContext};
use serde_json::json;
use std::time::Duration;

fn ctx_with_timeout(secs: u64) -> ToolContext {
    let pwd = std::env::current_dir().unwrap();
    ToolContext {
        workspace_path: pwd,
        bash_env_allowlist: vec![],
        bash_timeout_secs: secs,
    }
}

#[tokio::test]
async fn captures_stdout_and_exit_code() {
    let out = BashTool
        .invoke(json!({ "command": "echo hello" }), &ctx_with_timeout(10))
        .await
        .unwrap();
    assert!(out.stdout.contains("hello"));
    let DerivedEvent::CommandRun { exit_code, .. } = out.derived.unwrap() else {
        panic!()
    };
    assert_eq!(exit_code, 0);
}

#[tokio::test]
async fn nonzero_exit_is_not_a_tool_error() {
    let out = BashTool
        .invoke(json!({ "command": "exit 7" }), &ctx_with_timeout(10))
        .await
        .unwrap();
    // The tool itself succeeded; the agent sees the exit code and decides.
    assert!(out.error.is_none());
    let DerivedEvent::CommandRun { exit_code, .. } = out.derived.unwrap() else {
        panic!()
    };
    assert_eq!(exit_code, 7);
}

#[tokio::test]
async fn timeout_kills_runaway_process() {
    let started = std::time::Instant::now();
    let out = BashTool
        .invoke(json!({ "command": "sleep 60" }), &ctx_with_timeout(2))
        .await
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(10),
        "should have killed at ~2s, took {elapsed:?}"
    );
    assert!(out.error.as_deref().unwrap_or("").contains("timeout"));
}

#[tokio::test]
async fn cwd_is_workspace_path() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let mut ctx = ctx_with_timeout(10);
    ctx.workspace_path = tmp.path().to_path_buf();
    let out = BashTool
        .invoke(json!({ "command": "pwd" }), &ctx)
        .await
        .unwrap();
    let canonical = tmp.path().canonicalize().unwrap().display().to_string();
    assert!(
        out.stdout.contains(&canonical),
        "expected cwd to contain {canonical}, got: {}",
        out.stdout
    );
}

#[tokio::test]
async fn env_allowlist_filters_inherited_env() {
    let mut ctx = ctx_with_timeout(10);
    ctx.bash_env_allowlist = vec!["RUPU_TEST_VAR".into()];
    std::env::set_var("RUPU_TEST_VAR", "hello-rupu");
    std::env::set_var("RUPU_DENIED_VAR", "should-not-leak");

    let out = BashTool
        .invoke(
            json!({ "command": "echo $RUPU_TEST_VAR-$RUPU_DENIED_VAR" }),
            &ctx,
        )
        .await
        .unwrap();
    // Allowed var leaks in; denied var is empty.
    assert!(out.stdout.contains("hello-rupu-"), "got: {}", out.stdout);
    assert!(!out.stdout.contains("should-not-leak"));
}
