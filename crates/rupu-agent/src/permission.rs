//! Permission mode resolution + interactive prompt UX.
//!
//! Resolution precedence (spec §"Permission model"):
//!   CLI flag > agent frontmatter > project config > global config > default (Ask)

use rupu_tools::PermissionMode;

/// Pick the effective mode. The interactive prompt UX (in this same
/// module, [`PermissionPrompt`]) consumes the result.
pub fn resolve_mode(
    cli_flag: Option<PermissionMode>,
    agent_frontmatter: Option<PermissionMode>,
    project_config: Option<PermissionMode>,
    global_config: Option<PermissionMode>,
) -> PermissionMode {
    cli_flag
        .or(agent_frontmatter)
        .or(project_config)
        .or(global_config)
        .unwrap_or(PermissionMode::Ask)
}

/// Parse the textual mode from agent frontmatter / config files.
/// Returns `None` for an unknown string (caller decides whether that's
/// a hard error or a "skip this layer").
pub fn parse_mode(s: &str) -> Option<PermissionMode> {
    match s {
        "ask" => Some(PermissionMode::Ask),
        "bypass" => Some(PermissionMode::Bypass),
        "readonly" => Some(PermissionMode::Readonly),
        _ => None,
    }
}

/// Operator decision for an `Ask`-mode tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Allow this single tool call.
    Allow,
    /// Allow all calls of this tool kind for the rest of this run.
    AllowAlwaysForToolThisRun,
    /// Deny this single tool call (agent sees `permission_denied`).
    Deny,
    /// Stop the run entirely.
    StopRun,
}

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};

/// Truncation cap per `input` field shown in the prompt body. Spec
/// §"Permission model" says ~200 chars per field with a `more` option;
/// v0 ships a fixed truncation marker — interactive expand is deferred.
const TRUNCATE_AT: usize = 200;

/// Interactive prompt for `Ask`-mode tool calls. The reader is
/// type-erased via `Box<dyn BufRead>` so both in-memory and TTY
/// constructors return the same concrete type — no `R` type parameter
/// means callers need no turbofish. Two lifetime parameters keep the
/// reader and writer borrows independent so the test can move `output`
/// after `prompt` is dropped.
pub struct PermissionPrompt<'r, 'w, W: Write> {
    reader: Box<dyn BufRead + 'r>,
    writer: &'w mut W,
}

impl<'r, 'w, W: Write> PermissionPrompt<'r, 'w, W> {
    /// Create a new prompt from any `BufRead` + a mutable writer reference.
    pub fn new<R: BufRead + 'r>(input: R, output: &'w mut W) -> Self {
        Self {
            reader: Box::new(input),
            writer: output,
        }
    }

    /// Convenience constructor for in-memory / scripted-input tests.
    /// `input` is a byte slice; `output` is any mutable `Write` target.
    ///
    /// Deviation from plan: the struct uses `Box<dyn BufRead>` and two
    /// independent lifetime parameters instead of a generic `R` type
    /// parameter. This avoids E0283 (inference failure when `new_in_memory`
    /// is on a generic `impl<R>` block) and E0505 (borrow/move conflict
    /// when reader and writer shared the same `'a`), while keeping the
    /// public API identical to what the tests call.
    pub fn new_in_memory(input: &'r [u8], output: &'w mut W) -> Self {
        Self::new(BufReader::new(input), output)
    }

    /// Print the prompt body and read a single decision character.
    /// Re-prompts on invalid input.
    ///
    /// One-line format: `  → <tool>  <inline-summary>  (ws: <compact-path>)  [y/n/a/s]: `
    /// — collapses the previous five-line block (`Tool:`, `Workspace:`,
    /// `Input:`, JSON body, `Decision [y/n/a/s]:`) into a single
    /// horizontal scan. The summary picker knows the common tools
    /// (`bash`, `read`, `write_file`, `edit_file`); unknown tools fall
    /// back to a compact JSON one-liner. Long values are truncated with
    /// the `…(more)` marker so the line stays under one terminal row.
    pub fn ask(
        &mut self,
        tool: &str,
        input_json: &Value,
        workspace_path: &str,
    ) -> std::io::Result<PermissionDecision> {
        let summary = render_inline_summary(tool, input_json);
        let ws = compact_workspace(workspace_path);
        loop {
            write!(
                self.writer,
                "  → {tool}  {summary}  (ws: {ws})  [y/n/a/s]: "
            )?;
            self.writer.flush()?;
            let mut line = String::new();
            if self.reader.read_line(&mut line)? == 0 {
                // EOF — treat as Stop.
                return Ok(PermissionDecision::StopRun);
            }
            match line.trim() {
                "y" | "Y" => return Ok(PermissionDecision::Allow),
                "n" | "N" => return Ok(PermissionDecision::Deny),
                "a" | "A" => return Ok(PermissionDecision::AllowAlwaysForToolThisRun),
                "s" | "S" => return Ok(PermissionDecision::StopRun),
                other => {
                    writeln!(
                        self.writer,
                        "  Unknown: {other:?}. Please choose y, n, a, or s."
                    )?;
                }
            }
        }
    }
}

/// Pull a compact one-line summary out of `input_json` for the most
/// common tools. Returns `tool`-tailored output rather than raw JSON
/// so the operator's eye lands on the load-bearing field at a glance.
fn render_inline_summary(tool: &str, v: &Value) -> String {
    match tool {
        "bash" => v
            .get("command")
            .and_then(|c| c.as_str())
            .map(|s| truncate_inline(s, TRUNCATE_AT))
            .unwrap_or_else(|| "(no command)".to_string()),
        "read" => v
            .get("path")
            .and_then(|p| p.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "(no path)".to_string()),
        "write_file" => {
            let path = v.get("path").and_then(|p| p.as_str()).unwrap_or("?");
            let bytes = v
                .get("content")
                .and_then(|c| c.as_str())
                .map(|s| s.len())
                .unwrap_or(0);
            format!("{path}  ({bytes} bytes)")
        }
        "edit_file" => v
            .get("path")
            .and_then(|p| p.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "(no path)".to_string()),
        _ => {
            // Fallback: compact JSON one-liner, truncated. Don't pretty-
            // print — that would re-introduce multiple lines.
            let s = serde_json::to_string(v).unwrap_or_else(|_| String::new());
            truncate_inline(&s, TRUNCATE_AT)
        }
    }
}

/// Truncate `s` to `max` chars, appending `…(more)` when cut. The
/// `(more)` marker is asserted by the existing prompt tests; keep the
/// substring stable.
fn truncate_inline(s: &str, max: usize) -> String {
    // `s.chars().count()` would be more correct for multi-byte input
    // but tool inputs are mostly ASCII; the byte-length heuristic is
    // good enough and matches what the previous renderer did.
    if s.len() <= max {
        return s.to_string();
    }
    // Find a UTF-8-safe cut point at or before `max`.
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…(more)", &s[..cut])
}

/// Render `path` for the inline prompt: shows the full path when short,
/// otherwise an ellipsis-prefixed tail (`…/T/.tmpXEVvgt`). Keeps the
/// prompt under a typical 100-col terminal width.
fn compact_workspace(path: &str) -> String {
    const MAX: usize = 30;
    if path.len() <= MAX {
        return path.to_string();
    }
    // Take the trailing MAX-1 chars after a leading ellipsis.
    let suffix_len = MAX - 1;
    let start = path.len() - suffix_len;
    // UTF-8-safe boundary.
    let mut start = start;
    while start < path.len() && !path.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &path[start..])
}

impl<'w> PermissionPrompt<'static, 'w, std::io::Stderr> {
    /// Constructor for the CLI's `Ask` mode: wraps real `stdin` for
    /// input and a borrowed `Stderr` for output.
    ///
    /// Returns a prompt whose reader is `stdin()` boxed behind
    /// `dyn BufRead + 'static`, and whose writer is the provided
    /// `&mut Stderr`. The caller stashes a stderr handle in a local
    /// (`let mut stderr = std::io::stderr();`) and passes `&mut stderr`.
    /// `Stderr` (not `StderrLock`) implements `Write` directly and
    /// re-locks per-write — fine for interactive prompts where each
    /// write is small and operator-paced.
    ///
    /// Output goes to stderr (not stdout) so any piped command output
    /// stays clean — interactive prompts are operator UI, not data.
    pub fn for_stdio(stderr: &'w mut std::io::Stderr) -> Self {
        let reader: Box<dyn BufRead + 'static> = Box::new(BufReader::new(std::io::stdin()));
        Self {
            reader,
            writer: stderr,
        }
    }
}

