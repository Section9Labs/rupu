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
    pub fn ask(
        &mut self,
        tool: &str,
        input_json: &Value,
        workspace_path: &str,
    ) -> std::io::Result<PermissionDecision> {
        // Render the prompt body
        writeln!(self.writer)?;
        writeln!(self.writer, "  Tool:      {tool}")?;
        writeln!(self.writer, "  Workspace: {workspace_path}")?;
        let pretty = render_input(input_json);
        writeln!(self.writer, "  Input:")?;
        for line in pretty.lines() {
            writeln!(self.writer, "    {line}")?;
        }
        loop {
            write!(self.writer, "  Decision [y/n/a/s]: ")?;
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

fn render_input(v: &Value) -> String {
    let s = serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string());
    let mut lines: Vec<String> = Vec::new();
    for raw in s.lines() {
        if raw.len() > TRUNCATE_AT {
            let cut = &raw[..TRUNCATE_AT];
            lines.push(format!("{cut}\u{2026}(more)"));
        } else {
            lines.push(raw.to_string());
        }
    }
    lines.join("\n")
}
