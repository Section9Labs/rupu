//! `LineStreamPrinter` — the default output UI for `rupu`.
//!
//! Writes a streaming vertical timeline to stdout, line-by-line.
//! Works in any terminal, any pipe, and any CI runner — no alt-screen
//! takeover. Colors come from the Okesu palette and auto-degrade when
//! stdout is not a TTY or `NO_COLOR=1` is set.

use super::palette::{self, AWAITING, BRAND, COMPLETE, DIM, FAILED, RUNNING, TOOL_ARROW};
#[cfg(test)]
use super::palette::Status;
use chrono::{DateTime, Utc};
use std::io::{self, Write};
use std::time::Duration;

// ── Tree characters ──────────────────────────────────────────────────────────

const BRANCH: &str = "├─";
const PIPE: &str = "│";
const SPACE: &str = "  ";

// ── LineStreamPrinter ────────────────────────────────────────────────────────

/// Line-stream timeline printer. Writes to `stdout`.
///
/// Indent depth is managed by `push_indent` / `pop_indent` for
/// nested panel runs.
pub struct LineStreamPrinter {
    indent: usize,
    step_start: Option<std::time::Instant>,
}

impl Default for LineStreamPrinter {
    fn default() -> Self {
        Self::new()
    }
}

impl LineStreamPrinter {
    pub fn new() -> Self {
        Self {
            indent: 0,
            step_start: None,
        }
    }

    // ── Public API ────────────────────────────────────────────────────────

    /// `▶ <workflow_name>  <run_id>  HH:MM:SS`
    pub fn workflow_header(
        &mut self,
        workflow_name: &str,
        run_id: &str,
        started_at: DateTime<Utc>,
    ) {
        let mut buf = String::new();
        let _ = palette::write_colored(&mut buf, "▶", BRAND);
        buf.push(' ');
        buf.push_str(workflow_name);
        buf.push_str("  ");
        let _ = palette::write_colored(&mut buf, run_id, DIM);
        buf.push_str("  ");
        let ts = started_at.format("%H:%M:%S").to_string();
        let _ = palette::write_colored(&mut buf, &ts, DIM);
        println!("{buf}");
        println!();
    }

    /// `▶ <agent_name>  (<provider> · <model>)  <run_id>`
    pub fn agent_header(
        &mut self,
        agent_name: &str,
        provider: &str,
        model: &str,
        run_id: &str,
    ) {
        let mut buf = String::new();
        let _ = palette::write_colored(&mut buf, "▶", BRAND);
        buf.push(' ');
        buf.push_str(agent_name);
        buf.push_str("  ");
        let meta = format!("({provider} · {model})");
        let _ = palette::write_colored(&mut buf, &meta, DIM);
        buf.push_str("  ");
        let _ = palette::write_colored(&mut buf, run_id, DIM);
        println!("{buf}");
        println!();
    }

    /// `├─ ● <step_id>  (agent · provider · model)`
    pub fn step_start(
        &mut self,
        step_id: &str,
        agent: Option<&str>,
        provider: Option<&str>,
        model: Option<&str>,
    ) {
        self.step_start = Some(std::time::Instant::now());
        let mut buf = String::new();
        self.push_prefix(&mut buf, BRANCH);
        buf.push(' ');
        let _ = palette::write_colored(&mut buf, "●", RUNNING);
        buf.push(' ');
        buf.push_str(step_id);

        // Optional metadata in dim parentheses.
        let parts: Vec<&str> = [agent, provider, model]
            .iter()
            .filter_map(|o| *o)
            .collect();
        if !parts.is_empty() {
            let meta = format!("  ({})", parts.join(" · "));
            let _ = palette::write_colored(&mut buf, &meta, DIM);
        }
        println!("{buf}");
    }

    /// One assistant-text chunk. Auto-prefixes with `│  ` for indentation.
    pub fn assistant_chunk(&mut self, chunk: &str) {
        for line in chunk.lines() {
            let mut buf = String::new();
            self.push_content_prefix(&mut buf);
            buf.push_str(line);
            println!("{buf}");
        }
    }

    /// Tool call: `│  → <tool>  <summary>`
    pub fn tool_call(&mut self, tool: &str, summary: &str) {
        let mut buf = String::new();
        self.push_content_prefix(&mut buf);
        let arrow = "→ ";
        let _ = palette::write_colored(&mut buf, arrow, TOOL_ARROW);
        buf.push_str(tool);
        if !summary.is_empty() {
            buf.push_str("  ");
            let _ = palette::write_colored(&mut buf, summary, DIM);
        }
        println!("{buf}");
    }

    /// `✓ <step_id>  <duration> · <tokens>t`
    pub fn step_done(&mut self, step_id: &str, duration: Duration, total_tokens: u64) {
        let elapsed = self
            .step_start
            .take()
            .map(|s| s.elapsed())
            .unwrap_or(duration);
        let dur_str = format_duration(elapsed);
        let mut buf = String::new();
        self.push_content_prefix(&mut buf);
        let _ = palette::write_colored(&mut buf, "✓", COMPLETE);
        buf.push(' ');
        buf.push_str(step_id);
        buf.push_str("  ");
        let meta = format!("{dur_str} · {total_tokens}t");
        let _ = palette::write_colored(&mut buf, &meta, DIM);
        println!("{buf}");
        println!();
    }

    /// `✗ <step_id>  failed: <reason>`
    pub fn step_failed(&mut self, step_id: &str, reason: &str) {
        self.step_start = None;
        let mut buf = String::new();
        self.push_content_prefix(&mut buf);
        let _ = palette::write_colored(&mut buf, "✗", FAILED);
        buf.push(' ');
        buf.push_str(step_id);
        buf.push_str("  ");
        let msg = format!("failed: {reason}");
        let _ = palette::write_colored(&mut buf, &msg, FAILED);
        println!("{buf}");
        println!();
    }

    /// `⏸ <step_id>` + prompt + options. Reads a single char from stdin.
    ///
    /// Returns `'a'`, `'r'`, `'v'`, `'q'`, or any other char the user types.
    pub fn approval_prompt(&mut self, step_id: &str, prompt: &str) -> io::Result<char> {
        // Header line.
        let mut buf = String::new();
        self.push_prefix(&mut buf, BRANCH);
        buf.push(' ');
        let _ = palette::write_colored(&mut buf, "⏸", AWAITING);
        buf.push(' ');
        buf.push_str(step_id);
        println!("{buf}");

        // Prompt body (multi-line).
        for line in prompt.lines() {
            let mut b = String::new();
            self.push_content_prefix(&mut b);
            b.push_str(line);
            println!("{b}");
        }

        // Options line.
        let options_str = "[a] approve   [r] reject   [v] view findings   [q] cancel run";
        let mut b = String::new();
        self.push_content_prefix(&mut b);
        let _ = palette::write_colored(&mut b, options_str, AWAITING);
        println!("{b}");

        // Prompt marker.
        let mut b = String::new();
        self.push_content_prefix(&mut b);
        b.push_str("> ");
        print!("{b}");
        let _ = io::stdout().flush();

        // Read a single character.
        let ch = read_single_char()?;
        println!("{ch}");
        Ok(ch)
    }

    /// Optional reason prompt for reject. Returns the typed line.
    pub fn reject_reason_prompt(&mut self) -> io::Result<String> {
        let mut b = String::new();
        self.push_content_prefix(&mut b);
        b.push_str("Reason (optional, Enter to skip): ");
        print!("{b}");
        let _ = io::stdout().flush();
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        Ok(line.trim().to_string())
    }

    /// `✓ <workflow_name> complete  <run_id>  <duration> · <tokens>t total`
    pub fn workflow_done(
        &mut self,
        workflow_name: &str,
        run_id: &str,
        duration: Duration,
        total_tokens: u64,
    ) {
        let dur_str = format_duration(duration);
        let mut buf = String::new();
        let _ = palette::write_colored(&mut buf, "✓", COMPLETE);
        buf.push(' ');
        buf.push_str(workflow_name);
        buf.push_str(" complete");
        buf.push_str("  ");
        let _ = palette::write_colored(&mut buf, run_id, DIM);
        buf.push_str("  ");
        let meta = format!("{dur_str} · {total_tokens}t total");
        let _ = palette::write_colored(&mut buf, &meta, DIM);
        println!("{buf}");
    }

    /// `✗ <workflow_name> failed  <run_id>  error: <error>`
    pub fn workflow_failed(&mut self, workflow_name: &str, run_id: &str, error: &str) {
        let mut buf = String::new();
        let _ = palette::write_colored(&mut buf, "✗", FAILED);
        buf.push(' ');
        buf.push_str(workflow_name);
        buf.push_str(" failed");
        buf.push_str("  ");
        let _ = palette::write_colored(&mut buf, run_id, DIM);
        buf.push_str("  ");
        let msg = format!("error: {error}");
        let _ = palette::write_colored(&mut buf, &msg, FAILED);
        println!("{buf}");
    }

    /// Bump indent depth — for nested panel step runs.
    pub fn push_indent(&mut self) {
        self.indent += 1;
    }

    /// Pop indent depth.
    pub fn pop_indent(&mut self) {
        self.indent = self.indent.saturating_sub(1);
    }

    // ── Internal helpers ──────────────────────────────────────────────────

    /// Write `├─` (branch) or `└─` (last) prefixed by `indent` × `│  ` pipes.
    fn push_prefix(&self, buf: &mut String, branch: &str) {
        self.push_indent_pipes(buf);
        buf.push_str(branch);
    }

    /// Write the content-indent prefix: `indent` × `│  ` plus one more `│  `.
    fn push_content_prefix(&self, buf: &mut String) {
        self.push_indent_pipes(buf);
        buf.push_str(PIPE);
        buf.push_str("  ");
    }

    fn push_indent_pipes(&self, buf: &mut String) {
        for _ in 0..self.indent {
            buf.push_str(PIPE);
            buf.push_str(SPACE);
        }
    }
}

// ── Formatting helpers ────────────────────────────────────────────────────────

/// Format a `Duration` as `Xs` or `Xm Ys` or `HhXmYs`.
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        let tenths = d.subsec_millis() / 100;
        format!("{secs}.{tenths}s")
    } else if secs < 3600 {
        let m = secs / 60;
        let s = secs % 60;
        format!("{m}m {s}s")
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{h}h {m}m {s}s")
    }
}

/// Read a single character from stdin. Reads a line and returns the
/// first character. This is the simplest cross-platform implementation;
/// it requires the user to press Enter after their choice. Raw-mode
/// single-keypress is a UI enhancement that can be added later via the
/// crossterm dep when needed.
fn read_single_char() -> io::Result<char> {
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().chars().next().unwrap_or('a'))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Set NO_COLOR so tests never see ANSI escape codes.
    fn no_color() {
        // SAFETY: single-threaded test environment.
        std::env::set_var("NO_COLOR", "1");
    }

    fn run_with_printer<F: FnOnce(&mut LineStreamPrinter)>(f: F) {
        no_color();
        let mut p = LineStreamPrinter::new();
        f(&mut p);
    }

    #[test]
    fn test_format_duration_subsecond() {
        let d = Duration::from_millis(4300);
        assert_eq!(format_duration(d), "4.3s");
    }

    #[test]
    fn test_format_duration_minutes() {
        let d = Duration::from_secs(75);
        assert_eq!(format_duration(d), "1m 15s");
    }

    #[test]
    fn test_format_duration_hours() {
        let d = Duration::from_secs(3723);
        assert_eq!(format_duration(d), "1h 2m 3s");
    }

    #[test]
    fn test_status_glyphs() {
        assert_eq!(Status::Complete.glyph(), '✓');
        assert_eq!(Status::Failed.glyph(), '✗');
        assert_eq!(Status::Awaiting.glyph(), '⏸');
        assert_eq!(Status::Active.glyph(), '●');
        assert_eq!(Status::Skipped.glyph(), '⊘');
    }

    #[test]
    fn test_printer_indent() {
        run_with_printer(|p| {
            assert_eq!(p.indent, 0);
            p.push_indent();
            assert_eq!(p.indent, 1);
            p.push_indent();
            assert_eq!(p.indent, 2);
            p.pop_indent();
            assert_eq!(p.indent, 1);
            p.pop_indent();
            assert_eq!(p.indent, 0);
            // pop below 0 does nothing.
            p.pop_indent();
            assert_eq!(p.indent, 0);
        });
    }

    #[test]
    fn test_content_prefix_at_indent0() {
        let p = LineStreamPrinter::new();
        let mut buf = String::new();
        p.push_content_prefix(&mut buf);
        assert_eq!(buf, "│  ");
    }

    #[test]
    fn test_content_prefix_at_indent1() {
        let mut p = LineStreamPrinter::new();
        p.push_indent();
        let mut buf = String::new();
        p.push_content_prefix(&mut buf);
        assert_eq!(buf, "│  │  ");
    }

    #[test]
    fn test_branch_prefix_at_indent0() {
        let p = LineStreamPrinter::new();
        let mut buf = String::new();
        p.push_prefix(&mut buf, "├─");
        assert_eq!(buf, "├─");
    }

    #[test]
    fn test_branch_prefix_at_indent1() {
        let mut p = LineStreamPrinter::new();
        p.push_indent();
        let mut buf = String::new();
        p.push_prefix(&mut buf, "├─");
        assert_eq!(buf, "│  ├─");
    }
}
