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

use crate::tool::{DerivedEvent, Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
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

        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(&i.command);
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

        let child = cmd
            .spawn()
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        let timeout_dur = Duration::from_secs(ctx.bash_timeout_secs);

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
                })
            }
            Ok(Err(e)) => Ok(ToolOutput {
                stdout: String::new(),
                error: Some(format!("wait: {e}")),
                duration_ms: started.elapsed().as_millis() as u64,
                derived: None,
            }),
            Err(_elapsed) => {
                // Timeout. The kill_on_drop above will SIGKILL when
                // the child handle is dropped at the end of this scope.
                Ok(ToolOutput {
                    stdout: String::new(),
                    error: Some(format!("timeout after {}s", ctx.bash_timeout_secs)),
                    duration_ms: started.elapsed().as_millis() as u64,
                    derived: None,
                })
            }
        }
    }
}
