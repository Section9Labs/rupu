//! `ast_grep` tool — structural (syntax-tree) code search. Delegates
//! to the `ast-grep` binary if available, falling back to a clear
//! error otherwise.
//!
//! Why ast-grep: tree-sitter-backed pattern matching across 20+
//! languages via one binary. Reimplementing tree-sitter in-process
//! would be a large dependency surface for a capability the binary
//! already provides — this mirrors the `grep` tool's `rg` wrapper.
//!
//! Binary name is `ast-grep` only. We do NOT fall back to the `sg`
//! alias: it collides with a system tool on macOS.
//!
//! Exit-code semantics (match ripgrep): 0 = matches, 1 = no matches
//! (NOT an error), 2+ = real failure. We treat 0 and 1 as success;
//! anything else surfaces stderr in `error`.

use crate::coverage_emit::{attribution_from, emit};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use chrono::Utc;
use rupu_coverage::FileTouchEvent;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;

#[derive(Deserialize)]
struct Input {
    /// Structural pattern in ast-grep syntax. Metavariables: `$VAR`
    /// matches one named node, `$$$` matches zero or more nodes.
    pattern: String,
    /// Grammar to parse the pattern and target files with (e.g. `rust`,
    /// `python`, `typescript`). Required — a pattern is ambiguous
    /// without a grammar.
    lang: String,
    /// Optional sub-path within the workspace; defaults to workspace
    /// root.
    #[serde(default)]
    path: Option<String>,
}

/// Workspace-scoped structural search that delegates to `ast-grep`.
#[derive(Debug, Default, Clone)]
pub struct AstGrepTool;

#[async_trait]
impl Tool for AstGrepTool {
    fn name(&self) -> &'static str {
        "ast_grep"
    }

    fn description(&self) -> &'static str {
        "Search the workspace by code STRUCTURE (syntax tree), not text, using ast-grep. \
Provide a `pattern` in ast-grep syntax and a `lang` (rust, python, typescript, go, …). \
Metavariables: `$VAR` matches one named node, `$$$` matches zero or more nodes. \
Example: pattern `impl $T for $S` with lang `rust` finds trait impls; \
pattern `async fn $NAME($$$) -> Result<$$$>` finds async fns returning Result. \
Output is `path:line:col: match` lines (1-based, workspace-relative). \
Prefer this over `grep` when you want syntactic matches (call sites, impls, \
signatures) instead of regex over raw text. Returns empty stdout (not an error) \
when there are no matches."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Structural pattern in ast-grep syntax. Metavariables: `$VAR` = one node, `$$$` = zero-or-more nodes. Example: `impl $T for $S`."
                },
                "lang": {
                    "type": "string",
                    "description": "Language grammar to parse with, e.g. `rust`, `python`, `typescript`, `go`, `javascript`, `java`, `c`, `cpp`. Required."
                },
                "path": {
                    "type": "string",
                    "description": "Optional sub-path within the workspace to restrict the search. Defaults to the whole workspace."
                }
            },
            "required": ["pattern", "lang"]
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let i: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;

        let ast_grep = match which::which("ast-grep") {
            Ok(p) => p,
            Err(_) => {
                return Ok(ToolOutput {
                    stdout: String::new(),
                    error: Some(
                        "ast-grep not found; install with 'brew install ast-grep' or 'cargo install ast-grep'".into(),
                    ),
                    duration_ms: started.elapsed().as_millis() as u64,
                    derived: None,
                });
            }
        };

        let search_path = i
            .path
            .as_deref()
            .map(|p| ctx.workspace_path.join(p))
            .unwrap_or_else(|| ctx.workspace_path.clone());

        let out = Command::new(ast_grep)
            .arg("run")
            .arg("--pattern")
            .arg(&i.pattern)
            .arg("--lang")
            .arg(&i.lang)
            .arg("--json=stream")
            .arg(&search_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        let raw_stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

        // ast-grep exit code: 0 = matches, 1 = no matches (success), 2+ = error.
        // BUT a bad `path` exits 1 and a malformed `pattern` exits 0 — both write a
        // diagnostic to stderr while a legitimate run leaves stderr empty. So treat
        // any non-empty stderr as a real error regardless of exit code, else the
        // failure is silently reported as "no matches".
        let trimmed_stderr = stderr.trim();
        let error = if !trimmed_stderr.is_empty() {
            Some(stderr.clone())
        } else if matches!(out.status.code(), Some(0) | Some(1)) {
            None
        } else {
            Some("ast-grep failed".into())
        };

        // On success, parse the JSON-Lines stream into compact
        // `path:line:col: <first line of match>` output and per-file
        // coverage events. `--json=stream` emits one JSON object per
        // match; line/column are 0-based, so we add 1.
        let mut stdout = String::new();
        if error.is_none() {
            let mut by_file: BTreeMap<String, Vec<u32>> = BTreeMap::new();
            for raw_line in raw_stdout.lines() {
                let obj: Value = match serde_json::from_str(raw_line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let raw_path = obj.get("file").and_then(Value::as_str).unwrap_or("");
                if raw_path.is_empty() {
                    continue;
                }
                let start = obj.get("range").and_then(|r| r.get("start"));
                let line0 = start.and_then(|s| s.get("line")).and_then(Value::as_u64);
                let col0 = start.and_then(|s| s.get("column")).and_then(Value::as_u64);
                let (Some(line0), Some(col0)) = (line0, col0) else {
                    continue;
                };
                let line = (line0 as u32) + 1;
                let col = (col0 as u32) + 1;

                // First line of the (possibly multi-line) matched text.
                let snippet = obj
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("");

                // Make the path workspace-relative if possible.
                let rel_path = std::path::Path::new(raw_path)
                    .strip_prefix(&ctx.workspace_path)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| raw_path.to_string());

                stdout.push_str(&format!("{rel_path}:{line}:{col}: {snippet}\n"));
                by_file.entry(rel_path).or_default().push(line);
            }

            for (path, matched_lines) in by_file {
                let match_count = matched_lines.len() as u32;
                emit(
                    ctx,
                    FileTouchEvent::Grep {
                        path,
                        pattern: i.pattern.clone(),
                        match_count,
                        matched_lines,
                        tool: "ast_grep".to_string(),
                        attribution: attribution_from(ctx),
                        at: Utc::now(),
                    },
                )
                .await;
            }
        }

        Ok(ToolOutput {
            stdout,
            error,
            duration_ms: started.elapsed().as_millis() as u64,
            derived: None,
        })
    }
}
