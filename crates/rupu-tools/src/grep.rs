//! `grep` tool — delegates to the `rg` binary if available, falling
//! back to a clear error otherwise.
//!
//! Why ripgrep: gitignore-aware, fast, and every developer's machine
//! has it. Reimplementing in v0 would be over-engineered for the
//! surface area we need.
//!
//! Exit-code semantics: ripgrep returns 0 on matches, 1 on no
//! matches (NOT an error), and 2+ on real failure. We treat 0 and 1
//! as success; anything else surfaces stderr in `error`.

use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;

#[derive(Deserialize)]
struct Input {
    pattern: String,
    /// Optional sub-path within the workspace; defaults to workspace
    /// root.
    #[serde(default)]
    path: Option<String>,
}

/// Workspace-scoped grep that delegates to `rg`.
#[derive(Debug, Default, Clone)]
pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn description(&self) -> &'static str {
        "Search the workspace for a pattern using ripgrep. Output is `path:line:match` lines, gitignore-aware. Use this to locate symbols, callers, or any text across the workspace. Returns empty stdout (not an error) when there are no matches."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for. Ripgrep's default regex syntax (similar to PCRE)."
                },
                "path": {
                    "type": "string",
                    "description": "Optional sub-path within the workspace to restrict the search. Defaults to the whole workspace."
                }
            },
            "required": ["pattern"]
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let i: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let rg = which::which("rg")
            .map_err(|_| ToolError::Execution("`rg` (ripgrep) not found in PATH".into()))?;

        let search_path = i
            .path
            .as_deref()
            .map(|p| ctx.workspace_path.join(p))
            .unwrap_or_else(|| ctx.workspace_path.clone());

        let out = Command::new(rg)
            .arg("--with-filename")
            .arg("--line-number")
            .arg("--no-heading")
            .arg(&i.pattern)
            .arg(&search_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

        // ripgrep exit code: 0 = matches, 1 = no matches (success), 2+ = error
        let error = match out.status.code() {
            Some(0) | Some(1) => None,
            _ => Some(if stderr.is_empty() {
                "rg failed".into()
            } else {
                stderr
            }),
        };

        Ok(ToolOutput {
            stdout,
            error,
            duration_ms: started.elapsed().as_millis() as u64,
            derived: None,
        })
    }
}
