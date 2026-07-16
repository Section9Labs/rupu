//! `edit_file` tool — exact-match string replacement.
//!
//! Ambiguous matches (more than one occurrence) are an error; callers
//! should pass enough surrounding context to make `old_string` unique.
//! This is the same contract Claude Code's Edit tool uses.
//!
//! Workspace scope: paths that resolve outside the workspace root are
//! refused via `error: Some(...)` on the ToolOutput. The file must
//! already exist (no implicit create — use `write_file` for that).

use crate::coverage_emit::{attribution_from, emit};
use crate::path_scope::is_inside;
use crate::tool::{render_file_edit_diff, DerivedEvent, Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use chrono::Utc;
use rupu_coverage::FileTouchEvent;
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
        // Compute line range where old_string starts and ends (1-based).
        let byte_offset = text.find(i.old_string.as_str()).unwrap_or(0);
        let start_line = (text[..byte_offset].lines().count() as u32) + 1;
        let old_line_count = i.old_string.lines().count() as u32;
        let end_line = (start_line + old_line_count)
            .saturating_sub(1)
            .max(start_line);
        let lines_changed = i.new_string.lines().count() as u32;

        let new_text = text.replacen(&i.old_string, &i.new_string, 1);
        if let Err(e) = std::fs::write(&abs, &new_text) {
            return Ok(err_output(started, format!("write {}: {e}", i.path)));
        }

        emit(
            ctx,
            FileTouchEvent::Edit {
                path: i.path.clone(),
                line_range: [start_line, end_line],
                lines_changed,
                tool: "edit_file".to_string(),
                attribution: attribution_from(ctx),
                at: Utc::now(),
            },
        )
        .await;

        Ok(ToolOutput {
            stdout: format!("edited {}", i.path),
            error: None,
            duration_ms: started.elapsed().as_millis() as u64,
            derived: Some(DerivedEvent::FileEdit {
                path: i.path.clone(),
                kind: "modify".into(),
                diff: render_file_edit_diff(&i.path, Some(&text), Some(&new_text)),
            }),
            structured: None,
        })
    }
}

fn err_output(started: Instant, msg: String) -> ToolOutput {
    ToolOutput {
        stdout: String::new(),
        error: Some(msg),
        duration_ms: started.elapsed().as_millis() as u64,
        derived: None,
        structured: None,
    }
}
