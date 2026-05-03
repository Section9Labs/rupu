//! `write_file` tool — create or overwrite a file. Emits a
//! [`DerivedEvent::FileEdit`] so the transcript indexes file changes
//! without parsing tool inputs.
//!
//! Workspace scope: paths that resolve outside the workspace root are
//! refused via `error: Some(...)` on the ToolOutput. Intermediate
//! directories are created as needed (mkdir -p semantics).

use crate::path_scope::is_inside;
use crate::tool::{DerivedEvent, Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::time::Instant;

#[derive(Deserialize)]
struct Input {
    path: String,
    content: String,
}

/// Writes a file relative to the workspace root, emitting a
/// [`DerivedEvent::FileEdit`] with `kind = "create"` for new files
/// or `kind = "modify"` for overwrites.
#[derive(Debug, Default, Clone)]
pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Create a file or overwrite an existing one in the workspace. Use this for new files; for edits to existing files prefer `edit_file` to preserve unrelated content. Intermediate directories are created as needed. Paths must be inside the workspace root."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path relative to the workspace root."
                },
                "content": {
                    "type": "string",
                    "description": "Full content of the file. Existing content is replaced."
                }
            },
            "required": ["path", "content"]
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
        let kind = if abs.exists() { "modify" } else { "create" };
        if let Some(parent) = abs.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Ok(err_output(
                    started,
                    format!("mkdir {}: {e}", parent.display()),
                ));
            }
        }
        if let Err(e) = std::fs::write(&abs, &i.content) {
            return Ok(err_output(started, format!("write {}: {e}", i.path)));
        }
        Ok(ToolOutput {
            stdout: format!("wrote {} bytes to {}", i.content.len(), i.path),
            error: None,
            duration_ms: started.elapsed().as_millis() as u64,
            derived: Some(DerivedEvent::FileEdit {
                path: i.path,
                kind: kind.to_string(),
                diff: String::new(), // full-content writes; no minimal diff in v0
            }),
        })
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
