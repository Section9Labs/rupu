//! `glob` tool — recursive pattern matching via `globwalk`.
//!
//! Returns a sorted, newline-separated list of matching file paths
//! relative to the workspace root, or absolute for a pattern under one of
//! the run's read roots (a skill's directory). Pattern syntax is
//! glob-style with `**` for recursive descent. Matches outside the walk's
//! base are dropped.

use crate::coverage_emit::{attribution_from, emit};
use crate::descriptor::{Effect, ToolDescriptor};
use crate::path_scope::{in_read_root, is_inside};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput, WorkspaceScope};
use async_trait::async_trait;
use chrono::Utc;
use rupu_coverage::FileTouchEvent;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
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
    wildcard: true,
    description: "List files in the workspace matching a glob pattern. Output is one path per line, sorted, relative to the workspace root. Supports `**` for recursive descent. Patterns are relative to the workspace; an absolute pattern is allowed only inside a skill's base directory (`skills.load` returns it), and its matches are absolute. Returns empty stdout when nothing matches.",
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

        let read_root = read_root_pattern(&ctx.workspace, &i.pattern);
        let from_root = read_root.is_some();
        let (base, pattern) =
            read_root.unwrap_or_else(|| (ctx.workspace.path.clone(), i.pattern.clone()));
        let walker = globwalk::GlobWalkerBuilder::from_patterns(&base, &[&pattern])
            .max_depth(64)
            .follow_links(false)
            .build()
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        // Workspace scope, as the other fs tools: a pattern that climbs out
        // (`../*`, an absolute path) yields nothing outside the walk's base.
        // Matches under a read root are absolute; workspace matches relative.
        let mut matches = vec![];
        for entry in walker.flatten() {
            if entry.file_type().is_file() && is_inside(&base, entry.path()) {
                let shown = if from_root {
                    entry.path()
                } else {
                    entry
                        .path()
                        .strip_prefix(&ctx.workspace.path)
                        .unwrap_or(entry.path())
                };
                matches.push(shown.display().to_string());
            }
        }
        matches.sort();

        // Emit one FileTouchEvent per matched workspace path (success — walker
        // never returns a Rust-level error for non-matches, it simply yields
        // nothing). A skill's files are not target files.
        let touched = matches
            .iter()
            .filter(|p| !from_root && !in_read_root(&ctx.workspace, &ctx.workspace.path.join(p)));
        for path in touched {
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

/// An absolute pattern under one of the run's read roots walks from that
/// root; anything else walks the workspace.
fn read_root_pattern(ws: &WorkspaceScope, pattern: &str) -> Option<(PathBuf, String)> {
    ws.read_roots.iter().find_map(|root| {
        let prefix = format!("{}/", root.display());
        pattern
            .strip_prefix(&prefix)
            .map(|rest| (root.clone(), rest.to_owned()))
    })
}
