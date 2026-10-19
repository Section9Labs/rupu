use rupu_tools::{BashTool, DerivedEvent, Tool, ToolContext};
use serde_json::json;
use std::time::Duration;

fn ctx_with_timeout(secs: u64) -> ToolContext {
    let pwd = std::env::current_dir().unwrap();
    let mut ctx = ToolContext::in_workspace(pwd);
    ctx.workspace.bash.timeout_secs = secs;
    ctx
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
    ctx.workspace.path = tmp.path().to_path_buf();
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
    ctx.workspace.bash.env_allowlist = vec!["RUPU_TEST_VAR".into()];
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

/// The marker naming the launched child a process belongs to reaches every
/// command a run's `bash` starts, whatever the allowlist: the sweep finds a
/// crashed child's leftover tool processes by it.
#[tokio::test]
async fn the_run_marker_reaches_every_command() {
    std::env::set_var(rupu_tools::RUN_ID_ENV, "run_marked");
    let out = BashTool
        .invoke(
            json!({ "command": format!("echo marker=${}", rupu_tools::RUN_ID_ENV) }),
            &ctx_with_timeout(10),
        )
        .await
        .unwrap();
    std::env::remove_var(rupu_tools::RUN_ID_ENV);
    assert!(
        out.stdout.contains("marker=run_marked"),
        "got: {}",
        out.stdout
    );
}

/// A command whose shell runs an inner shell that starts a `sleep`: the
/// sleep is two levels below the tool's shell, and writes its pid where the
/// test can read it.
const NESTED_SLEEP: &str = "/bin/sh -c 'sleep 60 & echo $! > grandchild.pid; wait'; echo x";

fn grandchild_pid(dir: &std::path::Path) -> Option<u32> {
    std::fs::read_to_string(dir.join("grandchild.pid"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Whether `pid` is alive. A zombie is dead: under a PID 1 that never
/// reaps (the musl CI container, where it is `cargo`) a killed orphan stays a
/// zombie, and `kill(pid, 0)` still succeeds on it.
#[cfg(target_os = "linux")]
fn is_running(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // The state is the first field after the parenthesised command name.
    let state = stat
        .rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().next());
    !matches!(state, Some("Z" | "X"))
}

#[cfg(not(target_os = "linux"))]
fn is_running(pid: u32) -> bool {
    let pid = rustix::process::Pid::from_raw(i32::try_from(pid).unwrap()).unwrap();
    rustix::process::test_kill_process(pid).is_ok()
}

/// Wait (at most 5 s) for `pid` to be gone: dead, or a zombie no one has
/// reaped yet.
async fn assert_gone(pid: u32) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while is_running(pid) {
        if std::time::Instant::now() >= deadline {
            let _ = rustix::process::kill_process(
                rustix::process::Pid::from_raw(pid as i32).unwrap(),
                rustix::process::Signal::KILL,
            );
            panic!("the grandchild sleep {pid} outlived its bash call");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn timeout_kills_the_shells_grandchildren() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ToolContext::in_workspace(dir.path());
    ctx.workspace.bash.timeout_secs = 2;
    let out = BashTool
        .invoke(json!({ "command": NESTED_SLEEP }), &ctx)
        .await
        .unwrap();
    assert!(
        out.error.as_deref().unwrap_or("").contains("timeout"),
        "{out:?}"
    );
    let pid = grandchild_pid(dir.path()).expect("the grandchild wrote its pid");
    assert_gone(pid).await;
}

#[tokio::test]
async fn a_dropped_call_kills_the_shells_grandchildren() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::in_workspace(dir.path());
    let mut call = Box::pin(BashTool.invoke(json!({ "command": NESTED_SLEEP }), &ctx));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let pid = loop {
        tokio::select! {
            out = &mut call => panic!("the call finished before it was dropped: {out:?}"),
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
        if let Some(pid) = grandchild_pid(dir.path()) {
            break pid;
        }
        assert!(std::time::Instant::now() < deadline, "no grandchild pid");
    };
    assert!(is_running(pid));
    drop(call);
    assert_gone(pid).await;
}
