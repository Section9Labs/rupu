//! `glob` tool — recursive pattern matching via `globwalk`.
//!
//! Returns a sorted, newline-separated list of matching file paths
//! relative to the workspace root. Pattern syntax is glob-style with
//! `**` for recursive descent. Matches outside the workspace root are
//! dropped.

use crate::coverage_emit::{attribution_from, emit};
use crate::descriptor::{Effect, ToolDescriptor};
use crate::path_scope::is_inside;
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use chrono::Utc;
use rupu_coverage::FileTouchEvent;
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

/// This tool's descriptor.
pub static DESCRIPTOR: ToolDescriptor = ToolDescriptor {
    name: "glob",
    aliases: &[],
    effect: Effect::Read,
    needs: &[],
    uses: &[],
    description: "List files in the workspace matching a glob pattern. Output is one path per line, sorted, relative to the workspace root. Supports `**` for recursive descent. Returns empty stdout when nothing matches.",
    input_schema: descriptor_schema,
};

fn descriptor_schema() -> Value {
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

#[async_trait]
impl Tool for GlobTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &DESCRIPTOR
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let i: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;

        let walker = globwalk::GlobWalkerBuilder::from_patterns(&ctx.workspace.path, &[&i.pattern])
            .max_depth(64)
            .follow_links(false)
            .build()
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        // Workspace scope, as the other fs tools: a pattern that climbs out
        // (`../*`, an absolute path) yields nothing outside the workspace.
        let mut matches = vec![];
        for entry in walker.flatten() {
            if entry.file_type().is_file() && is_inside(&ctx.workspace.path, entry.path()) {
                let rel = entry
                    .path()
                    .strip_prefix(&ctx.workspace.path)
                    .unwrap_or(entry.path());
                matches.push(rel.display().to_string());
            }
        }
        matches.sort();

        // Emit one FileTouchEvent per matched path (success — walker never
        // returns a Rust-level error for non-matches, it simply yields nothing).
        for path in &matches {
            emit(
                ctx,
                FileTouchEvent::Glob {
                    path: path.clone(),
                    pattern: i.pattern.clone(),
                    tool: "glob".to_string(),
                    attribution: attribution_from(ctx),
                    at: Utc::now(),
                },
            )
            .await;
        }

        Ok(ToolOutput {
            stdout: matches.join("\n"),
            error: None,
            duration_ms: started.elapsed().as_millis() as u64,
            derived: None,
            structured: None,
        })
    }
}
