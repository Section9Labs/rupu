//! `bash` tool — execute a shell command in the workspace cwd with a
//! controlled environment.
//!
//! Security model:
//! - **cwd is locked** to the `workspace_path` from `ToolContext` —
//!   commands cannot inherit an arbitrary cwd from agent input.
//! - **Environment is cleared** then repopulated with `PATH`, `HOME`,
//!   `USER`, `TERM`, `LANG` (always allowed) plus `bash_env_allowlist`
//!   names (per-workspace). Other inherited env vars are dropped.
//! - **Timeout** sends SIGTERM (via tokio's `kill_on_drop` when the
//!   child handle drops at the end of its scope) — effectively SIGKILL
//!   on most Unix kernels for processes that ignore SIGTERM. The
//!   `tool_result` carries `error: Some("timeout after Ns")`.
//!
//! Exit codes: a non-zero exit is NOT a tool error. The tool succeeds
//! (Ok(ToolOutput { error: None, ... })) and the agent sees the exit
//! code via the `CommandRun` derived event.

use crate::coverage_emit::{attribution_from, emit};
use crate::tool::{DerivedEvent, Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use chrono::Utc;
use rupu_coverage::FileTouchEvent;
use serde::Deserialize;
use serde_json::Value;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::time::timeout;

#[derive(Deserialize)]
struct Input {
    command: String,
}

/// Owns one tracked capture call and guarantees `finished()` fires exactly
/// once — on every return path and when the `invoke` future is dropped
/// mid-await (a run cancelled while a bash call is in flight).
struct CaptureGuard(Option<Box<dyn rupu_netflow::CaptureCall>>);

impl CaptureGuard {
    fn shell_prefix(&self) -> Option<String> {
        self.0.as_ref().and_then(|c| c.shell_prefix())
    }

    fn spawned(&mut self, pid: u32) {
        if let Some(c) = self.0.as_mut() {
            c.spawned(pid);
        }
    }
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        if let Some(c) = self.0.take() {
            c.finished();
        }
    }
}

/// Always-forwarded environment variables (in addition to the
/// workspace-configured allowlist).
const ALWAYS_ALLOWED_ENV: &[&str] = &["PATH", "HOME", "USER", "TERM", "LANG"];

/// Bash subprocess tool with timeout, env allowlist, and CommandRun
/// derived event.
#[derive(Debug, Default, Clone)]
pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> &'static str {
        "Execute a shell command in the workspace directory. The command runs with a controlled environment (PATH, HOME, USER, TERM, LANG plus a per-workspace allowlist). Default timeout 120 seconds, configurable per-call. Use this for compilation, tests, git operations, and anything else that needs a shell. The cwd is locked to the workspace path; cd outside the workspace will produce an error from the shell, not an escape."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute, e.g. `cargo test`, `git diff HEAD`, `ls -la src/`."
                }
            },
            "required": ["command"]
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let i: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;

        // Subprocess network capture: only when a backend, a sink, the run
        // id and the tool-call id are all present. Otherwise the exact
        // pre-capture path runs.
        let mut capture_call: Option<CaptureGuard> = match (
            ctx.net_capture.as_ref(),
            ctx.netflow_sink.as_ref(),
            ctx.run_id.as_ref(),
            ctx.tool_call_id.as_ref(),
        ) {
            (Some(capture), Some(sink), Some(run_id), Some(tool_call_id)) => Some(CaptureGuard(
                Some(capture.begin(rupu_netflow::CallAttribution {
                    run_id: run_id.clone(),
                    step_id: None,
                    agent: ctx.agent.clone(),
                    codename: ctx.codename.clone(),
                    tool_call_id: tool_call_id.clone(),
                    sink: sink.clone(),
                })),
            )),
            _ => None,
        };
        let script = match capture_call.as_ref().and_then(|g| g.shell_prefix()) {
            Some(prefix) => format!("{prefix}\n{}", i.command),
            None => i.command.clone(),
        };

        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(&script);
        cmd.current_dir(&ctx.workspace_path);
        cmd.env_clear();
        for key in ALWAYS_ALLOWED_ENV
            .iter()
            .copied()
            .chain(ctx.bash_env_allowlist.iter().map(|s| s.as_str()))
        {
            if let Ok(val) = std::env::var(key) {
                cmd.env(key, val);
            }
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => return Err(ToolError::Execution(e.to_string())),
        };
        if let (Some(guard), Some(pid)) = (capture_call.as_mut(), child.id()) {
            guard.spawned(pid);
        }
        let timeout_dur = Duration::from_secs(ctx.bash_timeout_secs);

        // `capture_call` (if any) finishes when it drops: on every return
        // path, and also if this future is dropped mid-await (a cancelled
        // run).
        match timeout(timeout_dur, child.wait_with_output()).await {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
                let exit_code = out.status.code().unwrap_or(-1);
                let combined = if stderr.is_empty() {
                    stdout.clone()
                } else if stdout.is_empty() {
                    stderr.clone()
                } else {
                    format!("{stdout}\n[stderr]\n{stderr}")
                };

                // Emit Cmd events for tokens that look like existing file paths.
                // Skip flag tokens (starting with '-') and the shell invocation itself.
                for token in i.command.split_whitespace() {
                    if token.starts_with('-') {
                        continue;
                    }
                    let candidate = ctx.workspace_path.join(token);
                    if candidate.is_file() {
                        emit(
                            ctx,
                            FileTouchEvent::Cmd {
                                path: token.to_string(),
                                command: i.command.clone(),
                                tool: "bash".to_string(),
                                attribution: attribution_from(ctx),
                                at: Utc::now(),
                            },
                        )
                        .await;
                    }
                }

                Ok(ToolOutput {
                    stdout: combined,
                    error: None,
                    duration_ms: started.elapsed().as_millis() as u64,
                    derived: Some(DerivedEvent::CommandRun {
                        argv: vec!["/bin/sh".into(), "-c".into(), i.command],
                        cwd: ctx.workspace_path.display().to_string(),
                        exit_code,
                        stdout_bytes: out.stdout.len() as u64,
                        stderr_bytes: out.stderr.len() as u64,
                    }),
                    structured: None,
                })
            }
            Ok(Err(e)) => Ok(ToolOutput {
                stdout: String::new(),
                error: Some(format!("wait: {e}")),
                duration_ms: started.elapsed().as_millis() as u64,
                derived: None,
                structured: None,
            }),
            Err(_elapsed) => {
                // Timeout. The kill_on_drop above will SIGKILL when
                // the child handle is dropped at the end of this scope.
                Ok(ToolOutput {
                    stdout: String::new(),
                    error: Some(format!("timeout after {}s", ctx.bash_timeout_secs)),
                    duration_ms: started.elapsed().as_millis() as u64,
                    derived: None,
                    structured: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_netflow::{CallAttribution, CaptureCall, MemorySink, SubprocessCapture};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Log {
        begun: Vec<(String, String)>,
        pids: Vec<u32>,
        finished: u32,
        events: Vec<&'static str>,
    }

    struct Recorder {
        prefix: Option<String>,
        log: Arc<Mutex<Log>>,
    }

    struct RecordedCall {
        prefix: Option<String>,
        log: Arc<Mutex<Log>>,
    }

    impl SubprocessCapture for Recorder {
        fn begin(&self, call: CallAttribution) -> Box<dyn CaptureCall> {
            let mut l = self.log.lock().unwrap();
            l.events.push("begin");
            l.begun.push((call.run_id, call.tool_call_id));
            drop(l);
            Box::new(RecordedCall {
                prefix: self.prefix.clone(),
                log: self.log.clone(),
            })
        }
        fn run_finished(&self, _run_id: &str) {}
    }

    impl CaptureCall for RecordedCall {
        fn shell_prefix(&self) -> Option<String> {
            self.prefix.clone()
        }
        fn spawned(&mut self, pid: u32) {
            let mut l = self.log.lock().unwrap();
            l.events.push("spawned");
            l.pids.push(pid);
        }
        fn finished(self: Box<Self>) {
            let mut l = self.log.lock().unwrap();
            l.events.push("finished");
            l.finished += 1;
        }
    }

    fn ctx(dir: &std::path::Path, capture: Option<Recorder>) -> ToolContext {
        let mut c = ToolContext {
            workspace_path: dir.to_path_buf(),
            ..Default::default()
        };
        if let Some(r) = capture {
            c.net_capture = Some(Arc::new(r));
            c.netflow_sink = Some(Arc::new(MemorySink::default()));
            c.run_id = Some("run-x".into());
            c.tool_call_id = Some("toolu_1".into());
        }
        c
    }

    #[tokio::test]
    async fn drives_capture_around_the_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(Mutex::new(Log::default()));
        let c = ctx(
            dir.path(),
            Some(Recorder {
                prefix: None,
                log: log.clone(),
            }),
        );
        let out = BashTool
            .invoke(serde_json::json!({"command": "echo $$"}), &c)
            .await
            .unwrap();
        let l = log.lock().unwrap();
        assert_eq!(l.begun, vec![("run-x".to_string(), "toolu_1".to_string())]);
        assert_eq!(l.pids.len(), 1);
        assert!(l.pids[0] > 0);
        // The recorded pid is the shell's own pid, not a stray.
        assert_eq!(out.stdout.trim(), l.pids[0].to_string(), "{out:?}");
        assert_eq!(l.finished, 1);
        assert_eq!(l.events, vec!["begin", "spawned", "finished"]);
    }

    #[tokio::test]
    async fn shell_prefix_is_prepended() {
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(Mutex::new(Log::default()));
        let c = ctx(
            dir.path(),
            Some(Recorder {
                prefix: Some("export RUPU_MARK=1".into()),
                log: log.clone(),
            }),
        );
        let out = BashTool
            .invoke(serde_json::json!({"command": "echo \"m=$RUPU_MARK\""}), &c)
            .await
            .unwrap();
        assert!(out.stdout.contains("m=1"), "{out:?}");
        // The derived event still records the original command.
        match out.derived {
            Some(DerivedEvent::CommandRun { argv, .. }) => {
                assert_eq!(argv[2], "echo \"m=$RUPU_MARK\"");
            }
            other => panic!("unexpected derived: {other:?}"),
        }
        assert_eq!(log.lock().unwrap().finished, 1);
    }

    #[tokio::test]
    async fn finished_runs_on_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(Mutex::new(Log::default()));
        let mut c = ctx(
            dir.path(),
            Some(Recorder {
                prefix: None,
                log: log.clone(),
            }),
        );
        c.bash_timeout_secs = 1;
        let out = BashTool
            .invoke(serde_json::json!({"command": "sleep 5"}), &c)
            .await
            .unwrap();
        assert!(out.error.unwrap().contains("timeout"));
        assert_eq!(log.lock().unwrap().finished, 1);
    }

    #[tokio::test]
    async fn spawn_failure_still_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(Mutex::new(Log::default()));
        let mut c = ctx(
            &dir.path().join("does-not-exist"),
            Some(Recorder {
                prefix: None,
                log: log.clone(),
            }),
        );
        c.workspace_path = dir.path().join("does-not-exist");
        let res = BashTool
            .invoke(serde_json::json!({"command": "echo hi"}), &c)
            .await;
        assert!(res.is_err(), "{res:?}");
        let l = log.lock().unwrap();
        assert_eq!(l.finished, 1);
        assert_eq!(l.events, vec!["begin", "finished"]);
    }

    #[tokio::test]
    async fn dropped_future_still_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(Mutex::new(Log::default()));
        let c = ctx(
            dir.path(),
            Some(Recorder {
                prefix: None,
                log: log.clone(),
            }),
        );
        let fut = BashTool.invoke(serde_json::json!({"command": "sleep 5"}), &c);
        let _ = tokio::time::timeout(Duration::from_millis(300), fut).await;
        let l = log.lock().unwrap();
        assert_eq!(l.events, vec!["begin", "spawned", "finished"]);
    }

    #[tokio::test]
    async fn no_capture_means_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path(), None);
        let out = BashTool
            .invoke(serde_json::json!({"command": "echo hi"}), &c)
            .await
            .unwrap();
        assert!(out.stdout.contains("hi"));
        assert!(out.error.is_none());
    }
}
