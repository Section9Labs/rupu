//! `read_file` tool — full-file read with line-numbered output.
//!
//! Returns the file contents prefixed by 1-based line numbers separated
//! by tab — same format as Claude Code's Read tool, so models that
//! were trained on that shape produce well-aligned line references.
//!
//! Workspace scope: paths that resolve outside the workspace root
//! (e.g., via `../`) are refused with `error: Some("path X escapes
//! workspace")` on the ToolOutput. The tool itself returns `Ok(...)` —
//! the agent sees the error and decides what to do.

use crate::path_scope::is_inside;
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::time::Instant;

#[derive(Deserialize)]
struct Input {
    path: String,
}

/// Reads a file relative to the workspace root with line-numbered
/// output. See module docs for the path-escape behavior.
#[derive(Debug, Default, Clone)]
pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Read a file from the workspace and return its contents prefixed by 1-based line numbers separated by tabs. Always use this before editing a file so you have current line numbers. Paths are relative to the workspace root; absolute paths and paths that escape the workspace (e.g. `../`) are rejected."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path relative to the workspace root, e.g. `src/lib.rs` or `tests/foo.rs`."
                }
            },
            "required": ["path"]
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let i: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let abs = ctx.workspace_path.join(&i.path);
        if !is_inside(&ctx.workspace_path, &abs) {
            return Ok(err_output(
                started,
                format!("path {} escapes workspace", i.path),
            ));
        }
        match std::fs::read_to_string(&abs) {
            Ok(text) => {
                let mut out = String::with_capacity(text.len() + 64);
                for (idx, line) in text.lines().enumerate() {
                    use std::fmt::Write;
                    writeln!(out, "{}\t{}", idx + 1, line).expect("write to String never fails");
                }
                Ok(ToolOutput {
                    stdout: out,
                    error: None,
                    duration_ms: started.elapsed().as_millis() as u64,
                    derived: None,
                })
            }
            Err(e) => Ok(err_output(started, format!("read {}: {e}", i.path))),
        }
    }
}

fn err_output(started: Instant, msg: String) -> ToolOutput {
    ToolOutput {
        stdout: String::new(),
        error: Some(msg),
        duration_ms: started.elapsed().as_millis() as u64,
        derived: None,
    }
}
