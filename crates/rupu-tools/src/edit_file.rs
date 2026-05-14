//! `edit_file` tool — exact-match string replacement.
//!
//! Ambiguous matches (more than one occurrence) are an error; callers
//! should pass enough surrounding context to make `old_string` unique.
//! This is the same contract Claude Code's Edit tool uses.
//!
//! Workspace scope: paths that resolve outside the workspace root are
//! refused via `error: Some(...)` on the ToolOutput. The file must
//! already exist (no implicit create — use `write_file` for that).

use crate::path_scope::is_inside;
use crate::tool::{render_file_edit_diff, DerivedEvent, Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::time::Instant;

#[derive(Deserialize)]
struct Input {
    path: String,
    old_string: String,
    new_string: String,
}

/// Replaces an exact string in a file relative to the workspace root.
/// Emits a [`DerivedEvent::FileEdit`] with `kind = "modify"` and a
/// minimal +/- diff of the changed text.
#[derive(Debug, Default, Clone)]
pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "Replace an exact string in a file. The `old_string` must match exactly once in the file; if it matches zero times or more than once, the edit fails. Pass enough surrounding context (a few lines before/after) to make `old_string` uniquely identifying. The file must already exist; for new files use `write_file`."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path relative to the workspace root."
                },
                "old_string": {
                    "type": "string",
                    "description": "Exact substring to replace. Must match exactly once."
                },
                "new_string": {
                    "type": "string",
                    "description": "Replacement substring."
                }
            },
            "required": ["path", "old_string", "new_string"]
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
        let text = match std::fs::read_to_string(&abs) {
            Ok(t) => t,
            Err(e) => return Ok(err_output(started, format!("read {}: {e}", i.path))),
        };
        let count = text.matches(&i.old_string).count();
        if count == 0 {
            return Ok(err_output(
                started,
                format!("old_string not found in {}", i.path),
            ));
        }
        if count > 1 {
            return Ok(err_output(
                started,
                format!(
                    "old_string matches {count} places in {}; provide more context",
                    i.path
                ),
            ));
        }
        let new_text = text.replacen(&i.old_string, &i.new_string, 1);
        if let Err(e) = std::fs::write(&abs, &new_text) {
            return Ok(err_output(started, format!("write {}: {e}", i.path)));
        }
        Ok(ToolOutput {
            stdout: format!("edited {}", i.path),
            error: None,
            duration_ms: started.elapsed().as_millis() as u64,
            derived: Some(DerivedEvent::FileEdit {
                path: i.path.clone(),
                kind: "modify".into(),
                diff: render_file_edit_diff(&i.path, Some(&text), Some(&new_text)),
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
