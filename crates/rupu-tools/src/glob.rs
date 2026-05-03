//! `glob` tool — recursive pattern matching via `globwalk`.
//!
//! Returns a sorted, newline-separated list of matching file paths
//! relative to the workspace root. Pattern syntax is glob-style with
//! `**` for recursive descent.

use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::time::Instant;

#[derive(Deserialize)]
struct Input {
    pattern: String,
}

/// Workspace-scoped glob. Returns matching file paths relative to the
/// workspace root.
#[derive(Debug, Default, Clone)]
pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "glob"
    }

    fn description(&self) -> &'static str {
        "List files in the workspace matching a glob pattern. Output is one path per line, sorted, relative to the workspace root. Supports `**` for recursive descent. Returns empty stdout when nothing matches."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern, e.g. `src/**/*.rs` or `*.toml`."
                }
            },
            "required": ["pattern"]
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let i: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;

        let walker = globwalk::GlobWalkerBuilder::from_patterns(&ctx.workspace_path, &[&i.pattern])
            .max_depth(64)
            .follow_links(false)
            .build()
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        let mut matches = vec![];
        for entry in walker.flatten() {
            if entry.file_type().is_file() {
                let rel = entry
                    .path()
                    .strip_prefix(&ctx.workspace_path)
                    .unwrap_or(entry.path());
                matches.push(rel.display().to_string());
            }
        }
        matches.sort();

        Ok(ToolOutput {
            stdout: matches.join("\n"),
            error: None,
            duration_ms: started.elapsed().as_millis() as u64,
            derived: None,
        })
    }
}
