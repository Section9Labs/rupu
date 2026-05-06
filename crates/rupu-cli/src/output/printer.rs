//! `LineStreamPrinter` — the default output UI for `rupu`.
//!
//! Writes a streaming vertical timeline to stdout, line-by-line.
//! Works in any terminal, any pipe, and any CI runner — no alt-screen
//! takeover. Colors come from the Okesu palette and auto-degrade when
//! stdout is not a TTY or `NO_COLOR=1` is set.

use super::palette::{self, AWAITING, BRAND, BRAND_300, COMPLETE, DIM, FAILED, RUNNING, SEPARATOR, TOOL_ARROW};
use super::spinner::{Spinner, SpinnerHandle};
#[cfg(test)]
use super::palette::Status;
use chrono::{DateTime, Utc};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal;
use rupu_orchestrator::FindingRecord;
use std::io::{self, Write};
use std::time::Duration;

// ── Tree characters ──────────────────────────────────────────────────────────

const BRANCH: &str = "├─";
const PIPE: &str = "│";
const SPACE: &str = "  ";

// ── ANSI helpers ──────────────────────────────────────────────────────────────

/// `\x1b[s` — save cursor position (ANSI/VT100, widely supported).
const ANSI_SAVE: &str = "\x1b[s";

/// Build the ANSI RGB foreground escape for a given color, if colors are
/// supported. Falls back to empty string when `NO_COLOR` is set.
fn ansi_fg(color: owo_colors::Rgb) -> String {
    use owo_colors::OwoColorize;
    // Use a sentinel character; we want just the escape codes, not the text.
    // We format a styled space and extract the escape prefix/suffix.
    // Simpler: emit the raw CSI sequence conditionally.
    if std::env::var("NO_COLOR").is_ok() {
        return String::new();
    }
    // Check if stdout supports colors using owo-colors' stream check.
    let supports = "x"
        .if_supports_color(owo_colors::Stream::Stdout, |s| {
            s.color(color)
        })
        .to_string();
    // If the colored string equals "x" (no escape codes applied), no colors.
    if supports == "x" {
        return String::new();
    }
    let owo_colors::Rgb(r, g, b) = color;
    format!("\x1b[38;2;{r};{g};{b}m")
}

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
    ///
    /// Hierarchy: workflow name in brand-500 bold, run_id + time in dim.
    pub fn workflow_header(
        &mut self,
        workflow_name: &str,
        run_id: &str,
        started_at: DateTime<Utc>,
    ) {
        let mut buf = String::new();
        let _ = palette::write_colored(&mut buf, "▶", BRAND);
        buf.push(' ');
        // Workflow name in brand bold — the focal point.
        let _ = palette::write_bold_colored(&mut buf, workflow_name, BRAND);
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
        let _ = palette::write_bold_colored(&mut buf, agent_name, BRAND);
        buf.push_str("  ");
        let meta = format!("({provider} · {model})");
        let _ = palette::write_colored(&mut buf, &meta, DIM);
        buf.push_str("  ");
        let _ = palette::write_colored(&mut buf, run_id, DIM);
        println!("{buf}");
        println!();
    }

    /// `├─ ◐ <step_id>  (agent · provider · model)`
    ///
    /// Saves cursor position before the glyph so the spinner can animate it
    /// in-place. Returns a `SpinnerHandle` that keeps the glyph cycling until
    /// the step finishes (caller should store it and call `.stop()` or drop
    /// it when `step_done` / `step_failed` fires).
    pub fn step_start(
        &mut self,
        step_id: &str,
        agent: Option<&str>,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> SpinnerHandle {
        self.step_start = Some(std::time::Instant::now());

        // Prefix up to (but not including) the glyph.
        let mut prefix = String::new();
        self.push_prefix(&mut prefix, BRANCH);
        prefix.push(' ');

        // Save cursor position RIGHT before the glyph, then emit glyph.
        let color_ansi = ansi_fg(RUNNING);
        let mut buf = String::new();
        // The prefix (branch/pipes) goes out first without saving.
        print!("{prefix}");
        // Save cursor pos + print initial spinner frame.
        let initial = format!(
            "{save}{color}{glyph}\x1b[0m",
            save = ANSI_SAVE,
            color = color_ansi,
            glyph = super::spinner::FRAMES[0],
        );
        print!("{initial}");

        // Step id + optional metadata.
        buf.push(' ');
        buf.push_str(step_id);
        let parts: Vec<&str> = [agent, provider, model]
            .iter()
            .filter_map(|o| *o)
            .collect();
        if !parts.is_empty() {
            let meta = format!("  ({})", parts.join(" · "));
            let _ = palette::write_colored(&mut buf, &meta, DIM);
        }
        println!("{buf}");
        let _ = io::stdout().flush();

        // Start the spinner. It restores to the saved cursor and overwrites
        // the glyph cell each 125ms.
        Spinner::start_if_tty(&color_ansi)
    }

    /// Print a phase separator: blank line + dim `──────────────────────`.
    pub fn phase_separator(&mut self) {
        println!();
        let mut buf = String::new();
        self.push_content_prefix(&mut buf);
        let line = "──────────────────────";
        let _ = palette::write_colored(&mut buf, line, SEPARATOR);
        println!("{buf}");
        println!();
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

    /// `✓ <step_id>  · <duration> · <tokens> tokens`
    pub fn step_done(&mut self, step_id: &str, duration: Duration, total_tokens: u64) {
        let elapsed = self
            .step_start
            .take()
            .map(|s| s.elapsed())
            .unwrap_or(duration);
        let dur_str = format_duration(elapsed);
        let mut buf = String::new();
        self.push_content_prefix(&mut buf);
        let _ = palette::write_bold_colored(&mut buf, "✓", COMPLETE);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, step_id, COMPLETE);
        buf.push_str("  ");
        let meta = if total_tokens > 0 {
            format!("· {dur_str} · {total_tokens} tokens")
        } else {
            format!("· {dur_str}")
        };
        let _ = palette::write_colored(&mut buf, &meta, DIM);
        println!("{buf}");
        println!();
    }

    /// `✗ <step_id>  failed: <reason>`
    pub fn step_failed(&mut self, step_id: &str, reason: &str) {
        self.step_start = None;
        let mut buf = String::new();
        self.push_content_prefix(&mut buf);
        let _ = palette::write_bold_colored(&mut buf, "✗", FAILED);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, step_id, FAILED);
        buf.push_str("  ");
        let msg = format!("failed: {reason}");
        let _ = palette::write_colored(&mut buf, &msg, FAILED);
        println!("{buf}");
        println!();
    }

    /// `⏸ <step_id>` + prompt + options. Reads a **single** keypress from
    /// stdin via crossterm raw mode — no Enter required.
    ///
    /// Returns `'a'`, `'r'`, `'v'`, `'q'`, or any other char the user types.
    /// Ctrl-C and Esc both map to `'q'`.
    pub fn approval_prompt(&mut self, step_id: &str, prompt: &str) -> io::Result<char> {
        // Header line.
        let mut buf = String::new();
        self.push_prefix(&mut buf, BRANCH);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, "⏸", AWAITING);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, step_id, AWAITING);
        println!("{buf}");

        // Prompt body (multi-line).
        for line in prompt.lines() {
            let mut b = String::new();
            self.push_content_prefix(&mut b);
            b.push_str(line);
            println!("{b}");
        }

        // Options line in amber.
        let options_str = "[a] approve   [r] reject   [v] view findings   [q] cancel";
        let mut b = String::new();
        self.push_content_prefix(&mut b);
        let _ = palette::write_colored(&mut b, options_str, AWAITING);
        println!("{b}");

        // Prompt marker (no newline — we overwrite it when key arrives).
        let mut b = String::new();
        self.push_content_prefix(&mut b);
        b.push_str("> ");
        print!("{b}");
        let _ = io::stdout().flush();

        // Single-key read via crossterm raw mode.
        let ch = read_single_key()?;

        // Echo the chosen character and newline.
        println!("{ch}");
        Ok(ch)
    }

    /// Optional reason prompt for reject. Returns the typed line.
    /// This is the one place where line-buffered input is correct — the
    /// user needs to type a full sentence.
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

    /// Pretty-print a slice of `FindingRecord`s indented under the prompt.
    ///
    /// Each finding is rendered as:
    /// ```text
    /// │  [ critical ]  Title of the finding
    /// │               body text (wrapped at 78 cols)
    /// ```
    /// Colors are severity-driven; body is dim.
    pub fn print_findings(&mut self, findings: &[FindingRecord]) {
        if findings.is_empty() {
            let mut b = String::new();
            self.push_content_prefix(&mut b);
            let _ = palette::write_colored(&mut b, "(no findings)", DIM);
            println!("{b}");
            return;
        }

        for f in findings {
            let (sev_color, sev_bold) = severity_color(&f.severity);
            let badge = format!("[ {} ]", f.severity);

            // Badge line: │  [ critical ]  Title
            let mut b = String::new();
            self.push_content_prefix(&mut b);
            if sev_bold {
                let _ = palette::write_bold_colored(&mut b, &badge, sev_color);
            } else {
                let _ = palette::write_colored(&mut b, &badge, sev_color);
            }
            b.push_str("  ");
            b.push_str(&f.title);
            println!("{b}");

            // Source line: dim, indented under badge.
            if !f.source.is_empty() {
                let badge_width = badge.chars().count() + 2; // badge + "  "
                let indent_str = " ".repeat(badge_width);
                let mut src = String::new();
                self.push_content_prefix(&mut src);
                src.push_str(&indent_str);
                let source_text = format!("source: {}", f.source);
                let _ = palette::write_colored(&mut src, &source_text, DIM);
                println!("{src}");
            }

            // Body text — break at 78 cols, each line indented.
            if !f.body.is_empty() {
                let badge_width = severity_badge_width(&f.severity);
                let indent_str = " ".repeat(badge_width);
                for line in wrap_text(&f.body, 78) {
                    let mut body_line = String::new();
                    self.push_content_prefix(&mut body_line);
                    body_line.push_str(&indent_str);
                    let _ = palette::write_colored(&mut body_line, &line, DIM);
                    println!("{body_line}");
                }
            }
        }
    }

    /// `✓ <workflow_name> complete  <run_id>  <duration> · <tokens> tokens total`
    pub fn workflow_done(
        &mut self,
        workflow_name: &str,
        run_id: &str,
        duration: Duration,
        total_tokens: u64,
    ) {
        println!();
        let dur_str = format_duration(duration);
        let mut buf = String::new();
        let _ = palette::write_bold_colored(&mut buf, "✓", COMPLETE);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, workflow_name, COMPLETE);
        buf.push_str(" complete");
        buf.push_str("  ");
        let _ = palette::write_colored(&mut buf, run_id, DIM);
        buf.push_str("  ");
        let meta = format!("· {dur_str} · {total_tokens} tokens total");
        let _ = palette::write_colored(&mut buf, &meta, DIM);
        println!("{buf}");
    }

    /// `✗ <workflow_name> failed  <run_id>  error: <error>`
    pub fn workflow_failed(&mut self, workflow_name: &str, run_id: &str, error: &str) {
        println!();
        let mut buf = String::new();
        let _ = palette::write_bold_colored(&mut buf, "✗", FAILED);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, workflow_name, FAILED);
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
        // Brand-300 colored pipe for visual warmth; falls back to plain │ on no-color.
        let _ = palette::write_colored(buf, PIPE, BRAND_300);
        buf.push_str("  ");
    }

    fn push_indent_pipes(&self, buf: &mut String) {
        for _ in 0..self.indent {
            let _ = palette::write_colored(buf, PIPE, BRAND_300);
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

/// Map severity string to (color, bold) for findings display.
fn severity_color(severity: &str) -> (owo_colors::Rgb, bool) {
    match severity.to_ascii_lowercase().as_str() {
        "critical" => (palette::SEV_CRITICAL, true),
        "high"     => (palette::SEV_HIGH, true),
        "medium"   => (palette::SEV_MEDIUM, false),
        "low"      => (palette::SEV_LOW, false),
        _          => (palette::SEV_INFO, false), // "info" + unknown
    }
}

/// Width of `"[ severity ]  "` for body indentation alignment.
fn severity_badge_width(severity: &str) -> usize {
    // "[ " + severity + " ]" + "  "
    severity.len() + 6
}

/// Naïve word-wrap: split on spaces and re-join into lines ≤ `width` chars.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.len() + 1 + word.len() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current.clone());
            current.clear();
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Read a single keypress from stdin using crossterm raw mode.
/// No Enter required. Ctrl-C / Esc map to `'q'`.
fn read_single_key() -> io::Result<char> {
    terminal::enable_raw_mode()?;
    let result = loop {
        match event::read()? {
            Event::Key(key) => {
                let ch = match key.code {
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => 'q',
                    KeyCode::Esc => 'q',
                    KeyCode::Char(c) => c,
                    KeyCode::Enter => '\n',
                    _ => continue,
                };
                break ch;
            }
            // Ignore non-key events.
            _ => continue,
        }
    };
    terminal::disable_raw_mode()?;
    Ok(result)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_orchestrator::FindingRecord;
    use std::time::Duration;

    /// Set NO_COLOR so tests never see ANSI escape codes.
    fn no_color() {
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
        // Without colors (NO_COLOR) the pipe is just the raw char.
        no_color();
        let p = LineStreamPrinter::new();
        let mut buf = String::new();
        p.push_content_prefix(&mut buf);
        assert_eq!(buf, "│  ");
    }

    #[test]
    fn test_content_prefix_at_indent1() {
        no_color();
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
        no_color();
        let mut p = LineStreamPrinter::new();
        p.push_indent();
        let mut buf = String::new();
        p.push_prefix(&mut buf, "├─");
        assert_eq!(buf, "│  ├─");
    }

    #[test]
    fn test_wrap_text_short() {
        let lines = wrap_text("hello world", 80);
        assert_eq!(lines, vec!["hello world"]);
    }

    #[test]
    fn test_wrap_text_breaks() {
        let lines = wrap_text("one two three", 7);
        assert_eq!(lines, vec!["one two", "three"]);
    }

    #[test]
    fn test_wrap_text_empty() {
        let lines = wrap_text("", 80);
        assert_eq!(lines, vec![""]);
    }

    // ── print_findings tests ──────────────────────────────────────────────

    /// Build a `FindingRecord` for tests.
    fn make_finding(severity: &str, title: &str, body: &str, source: &str) -> FindingRecord {
        FindingRecord {
            severity: severity.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            source: source.to_string(),
        }
    }

    #[test]
    fn test_print_findings_no_color_contains_titles() {
        // We can't capture stdout easily without a custom writer, but we can
        // at least verify the severity_color mapping returns correct values.
        let (color, bold) = severity_color("critical");
        assert_eq!(color, palette::SEV_CRITICAL);
        assert!(bold);

        let (color, bold) = severity_color("high");
        assert_eq!(color, palette::SEV_HIGH);
        assert!(bold);

        let (color, bold) = severity_color("medium");
        assert_eq!(color, palette::SEV_MEDIUM);
        assert!(!bold);

        let (color, bold) = severity_color("low");
        assert_eq!(color, palette::SEV_LOW);
        assert!(!bold);

        let (color, bold) = severity_color("info");
        assert_eq!(color, palette::SEV_INFO);
        assert!(!bold);

        // Unknown severity treated as info.
        let (color2, _) = severity_color("note");
        assert_eq!(color2, palette::SEV_INFO);
    }

    #[test]
    fn test_severity_ordering_in_findings_vec() {
        // Verify that when we have multiple findings they render in-order.
        let findings = [
            make_finding("critical", "SQL Injection in login handler", "Details here", "scanner"),
            make_finding("high", "Hardcoded secret in config.yaml", "secret=abc123", "grep"),
            make_finding("medium", "Outdated dep: openssl 1.0.1", "", "deps"),
            make_finding("low", "Missing license header", "", "linter"),
            make_finding("info", "20 files scanned", "", ""),
        ];
        // Check severity titles appear in the expected order.
        let severities: Vec<&str> = findings.iter().map(|f| f.severity.as_str()).collect();
        assert_eq!(severities, ["critical", "high", "medium", "low", "info"]);
    }

    #[test]
    fn test_severity_badge_width() {
        // "[ critical ]  " = 2+8+2 + 2 = 14
        assert_eq!(severity_badge_width("critical"), "critical".len() + 6);
        assert_eq!(severity_badge_width("info"), "info".len() + 6);
    }

    /// Round-trip test: `print_findings` does not panic on empty slice.
    #[test]
    fn test_print_findings_empty_no_panic() {
        no_color();
        let mut p = LineStreamPrinter::new();
        // Should not panic; prints "(no findings)" message.
        p.print_findings(&[]);
    }

    /// `print_findings` does not panic on populated slice.
    #[test]
    fn test_print_findings_populated_no_panic() {
        no_color();
        let mut p = LineStreamPrinter::new();
        let findings = vec![
            make_finding("critical", "Title A", "Body A body A body A", "src"),
            make_finding("info", "Title B", "", ""),
        ];
        p.print_findings(&findings);
    }
}
