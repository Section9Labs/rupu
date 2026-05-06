//! `LineStreamPrinter` — the default output UI for `rupu`.
//!
//! Writes a streaming vertical timeline to stdout, line-by-line.
//! Works in any terminal, any pipe, and any CI runner — no alt-screen
//! takeover. Colors come from the Okesu palette and auto-degrade when
//! stdout is not a TTY or `NO_COLOR=1` is set.
//!
//! Design constraint: this is a single-writer, append-only stream.
//! No cursor save/restore, no in-place glyph animation — those break
//! when interleaved with concurrent text output. Status changes are
//! conveyed by what we print at the time of the transition (header
//! line for "started", footer line for "done").

use super::palette::{
    self, AWAITING, BRAND, BRAND_300, COMPLETE, DIM, FAILED, RUNNING, SEPARATOR, TOOL_ARROW,
};
use super::spinner::{Spinner, SpinnerHandle};
#[cfg(test)]
use super::palette::Status;
use chrono::{DateTime, Utc};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal;
use rupu_orchestrator::FindingRecord;
use std::io::{self, IsTerminal, Write};
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
        let _ = palette::write_bold_colored(&mut buf, workflow_name, BRAND);
        buf.push_str("  ");
        let _ = palette::write_colored(&mut buf, run_id, DIM);
        buf.push_str("  ");
        let ts = started_at.format("%H:%M:%S").to_string();
        let _ = palette::write_colored(&mut buf, &ts, DIM);
        println!("{buf}");
        // Continuous rail: even the gap line carries the pipe so the
        // user's eye doesn't lose the column.
        self.print_rail_only();
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
        self.print_rail_only();
    }

    /// `├─ ◐ <step_id>  (agent · provider · model)`
    ///
    /// Static glyph — no animation. Returns a no-op `SpinnerHandle` so the
    /// caller's lifecycle code keeps working without changes.
    pub fn step_start(
        &mut self,
        step_id: &str,
        agent: Option<&str>,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> SpinnerHandle {
        self.step_start = Some(std::time::Instant::now());

        let mut buf = String::new();
        self.push_prefix(&mut buf, BRANCH);
        buf.push(' ');
        // Static "working" glyph in blue.
        let _ = palette::write_bold_colored(&mut buf, "◐", RUNNING);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, step_id, RUNNING);

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

        Spinner::start()
    }

    /// `├─ ⧗ panel: <step_id>  (N panelists)`
    ///
    /// Special opener for panel steps — sets visual expectation that
    /// children will follow indented. Returns a no-op handle.
    pub fn panel_start(&mut self, step_id: &str, panelists: usize) -> SpinnerHandle {
        self.step_start = Some(std::time::Instant::now());

        let mut buf = String::new();
        self.push_prefix(&mut buf, BRANCH);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, "◐", RUNNING);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, step_id, RUNNING);
        let meta = format!("  (panel · {panelists} panelists)");
        let _ = palette::write_colored(&mut buf, &meta, DIM);
        println!("{buf}");
        let _ = io::stdout().flush();

        Spinner::start()
    }

    /// Render one panelist child line under a panel step: status glyph,
    /// agent name, optional one-line summary, and findings count.
    pub fn panelist_line(
        &mut self,
        agent: &str,
        success: bool,
        findings_count: usize,
    ) {
        let mut buf = String::new();
        // Indent under parent step using the rail.
        self.push_indent_pipes(&mut buf);
        let _ = palette::write_colored(&mut buf, PIPE, BRAND_300);
        buf.push_str("  ");
        // Branch character for child.
        buf.push_str("├─ ");
        let (glyph, color) = if success {
            ("✓", COMPLETE)
        } else {
            ("✗", FAILED)
        };
        let _ = palette::write_bold_colored(&mut buf, glyph, color);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, agent, color);
        // Findings tally.
        let tally = if findings_count == 1 {
            "  · 1 finding".to_string()
        } else {
            format!("  · {findings_count} findings")
        };
        let _ = palette::write_colored(&mut buf, &tally, DIM);
        println!("{buf}");
    }

    /// Print a phase separator. Caller is responsible for any preceding
    /// rail line (step_done already emits one).
    pub fn phase_separator(&mut self) {
        let mut buf = String::new();
        self.push_content_prefix(&mut buf);
        let line = "──────────────────────";
        let _ = palette::write_colored(&mut buf, line, SEPARATOR);
        println!("{buf}");
        self.print_rail_only();
    }

    /// One assistant-text chunk. Preserves blank lines as rail-only lines so
    /// the visual column never breaks.
    pub fn assistant_chunk(&mut self, chunk: &str) {
        for line in chunk.split('\n') {
            if line.is_empty() {
                self.print_rail_only();
            } else {
                let mut buf = String::new();
                self.push_content_prefix(&mut buf);
                buf.push_str(line);
                println!("{buf}");
            }
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

    /// `│  ✓ <step_id>  · N findings · Xs`  — panel-step footer with
    /// findings tally instead of token count.
    pub fn panel_done(
        &mut self,
        step_id: &str,
        success: bool,
        findings_count: usize,
        duration: Duration,
    ) {
        let elapsed = self
            .step_start
            .take()
            .map(|s| s.elapsed())
            .unwrap_or(duration);
        let dur_str = format_duration(elapsed);
        let mut buf = String::new();
        self.push_content_prefix(&mut buf);
        let (glyph, color) = if success {
            ("✓", COMPLETE)
        } else {
            ("✗", FAILED)
        };
        let _ = palette::write_bold_colored(&mut buf, glyph, color);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, step_id, color);
        buf.push_str("  ");
        let tally = if findings_count == 1 {
            "· 1 finding".to_string()
        } else {
            format!("· {findings_count} findings")
        };
        let _ = palette::write_colored(&mut buf, &tally, DIM);
        buf.push_str("  ");
        let dur_meta = format!("· {dur_str}");
        let _ = palette::write_colored(&mut buf, &dur_meta, DIM);
        println!("{buf}");
        self.print_rail_only();
    }

    /// `│  ✓ <step_id>  · <duration> · <tokens> tokens`
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
        self.print_rail_only();
    }

    /// `│  ✗ <step_id>  failed: <reason>`
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
        self.print_rail_only();
    }

    /// `├─ ⏸ <step_id>` + prompt + options.
    ///
    /// Reads a single keypress via crossterm raw mode. Falls back to
    /// line-buffered read when stdin is not a TTY (CI / piped).
    /// Echoes the chosen character and emits a newline.
    /// Returns `'a'`, `'r'`, `'v'`, `'q'`, or another typed char. Ctrl-C
    /// and Esc map to `'q'`.
    pub fn approval_prompt(&mut self, step_id: &str, prompt: &str) -> io::Result<char> {
        let mut buf = String::new();
        self.push_prefix(&mut buf, BRANCH);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, "⏸", AWAITING);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, step_id, AWAITING);
        println!("{buf}");

        for line in prompt.split('\n') {
            if line.is_empty() {
                self.print_rail_only();
            } else {
                let mut b = String::new();
                self.push_content_prefix(&mut b);
                b.push_str(line);
                println!("{b}");
            }
        }

        // Options + key affordance, both amber.
        let options_str = "[a] approve   [r] reject   [v] view findings   [q] detach";
        let mut b = String::new();
        self.push_content_prefix(&mut b);
        let _ = palette::write_colored(&mut b, options_str, AWAITING);
        println!("{b}");

        // Inline marker that names the keys — no `> ` (which reads as
        // "type something here").
        let mut b = String::new();
        self.push_content_prefix(&mut b);
        let _ = palette::write_bold_colored(&mut b, "[a/r/v/q]: ", AWAITING);
        print!("{b}");
        let _ = io::stdout().flush();

        let ch = if io::stdin().is_terminal() {
            read_single_key()?
        } else {
            read_line_first_char()?
        };

        // Echo the chosen character and a newline.
        let mut echo = String::new();
        let _ = palette::write_bold_colored(&mut echo, &ch.to_string(), AWAITING);
        println!("{echo}");
        Ok(ch)
    }

    /// Optional reason prompt for reject. Returns the typed line.
    /// Line-buffered: the user really does need to type a sentence here.
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

    /// Pretty-print a slice of `FindingRecord`s grouped by source step.
    /// `groups` is a vec of `(step_id, findings)` pairs in display order.
    /// When `groups` has just one (synthetic) "all" group the step header
    /// is suppressed.
    pub fn print_findings(&mut self, groups: &[(String, Vec<FindingRecord>)]) {
        let total: usize = groups.iter().map(|(_, fs)| fs.len()).sum();
        if total == 0 {
            let mut b = String::new();
            self.push_content_prefix(&mut b);
            let _ = palette::write_colored(&mut b, "(no findings)", DIM);
            println!("{b}");
            return;
        }

        // Header line: "─── findings (N total) ───"
        let mut hdr = String::new();
        self.push_content_prefix(&mut hdr);
        let header_text = if total == 1 {
            "─── 1 finding ───".to_string()
        } else {
            format!("─── {total} findings ───")
        };
        let _ = palette::write_colored(&mut hdr, &header_text, SEPARATOR);
        println!("{hdr}");
        self.print_rail_only();

        for (step_id, findings) in groups {
            if groups.len() > 1 && !step_id.is_empty() {
                // Step subhead.
                let mut sub = String::new();
                self.push_content_prefix(&mut sub);
                let label = format!("from {} ({})", step_id, findings.len());
                let _ = palette::write_colored(&mut sub, &label, DIM);
                println!("{sub}");
            }

            for f in findings {
                let (sev_color, sev_bold) = severity_color(&f.severity);
                let badge = format!("[{}]", f.severity);

                // Badge + title line.
                let mut b = String::new();
                self.push_content_prefix(&mut b);
                if sev_bold {
                    let _ = palette::write_bold_colored(&mut b, &badge, sev_color);
                } else {
                    let _ = palette::write_colored(&mut b, &badge, sev_color);
                }
                b.push_str("  ");
                let _ = palette::write_bold_colored(&mut b, &f.title, palette::Status::Active.color());
                println!("{b}");

                // Source.
                if !f.source.is_empty() {
                    let mut src = String::new();
                    self.push_content_prefix(&mut src);
                    src.push_str("    ");
                    let stext = format!("source: {}", f.source);
                    let _ = palette::write_colored(&mut src, &stext, DIM);
                    println!("{src}");
                }

                // Body (wrapped).
                if !f.body.is_empty() {
                    for line in wrap_text(&f.body, 76) {
                        let mut bl = String::new();
                        self.push_content_prefix(&mut bl);
                        bl.push_str("    ");
                        let _ = palette::write_colored(&mut bl, &line, DIM);
                        println!("{bl}");
                    }
                }
                self.print_rail_only();
            }
        }
    }

    /// `✓ <workflow_name> complete  <run_id>  · <duration> · <tokens> tokens`
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
        buf.push_str(" complete  ");
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
        buf.push_str(" failed  ");
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

    fn push_prefix(&self, buf: &mut String, branch: &str) {
        self.push_indent_pipes(buf);
        buf.push_str(branch);
    }

    fn push_content_prefix(&self, buf: &mut String) {
        self.push_indent_pipes(buf);
        let _ = palette::write_colored(buf, PIPE, BRAND_300);
        buf.push_str("  ");
    }

    fn push_indent_pipes(&self, buf: &mut String) {
        for _ in 0..self.indent {
            let _ = palette::write_colored(buf, PIPE, BRAND_300);
            buf.push_str(SPACE);
        }
    }

    /// Print a single `│` line at current indent — used to keep the rail
    /// continuous through blank gaps.
    fn print_rail_only(&self) {
        let mut buf = String::new();
        self.push_indent_pipes(&mut buf);
        let _ = palette::write_colored(&mut buf, PIPE, BRAND_300);
        println!("{buf}");
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
        _          => (palette::SEV_INFO, false),
    }
}

/// Naïve word-wrap: split on whitespace, re-join into lines ≤ `width` chars.
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
            _ => continue,
        }
    };
    terminal::disable_raw_mode()?;
    Ok(result)
}

/// Fallback: read a line, return its first char (or `'q'` on EOF / blank).
fn read_line_first_char() -> io::Result<char> {
    let mut line = String::new();
    let n = io::stdin().read_line(&mut line)?;
    if n == 0 {
        return Ok('q');
    }
    Ok(line.trim().chars().next().unwrap_or('q'))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_orchestrator::FindingRecord;
    use std::time::Duration;

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
            p.pop_indent();
            assert_eq!(p.indent, 0);
        });
    }

    #[test]
    fn test_content_prefix_at_indent0() {
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

    fn make_finding(severity: &str, title: &str, body: &str, source: &str) -> FindingRecord {
        FindingRecord {
            severity: severity.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            source: source.to_string(),
        }
    }

    #[test]
    fn test_severity_color_mapping() {
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

        let (color2, _) = severity_color("note");
        assert_eq!(color2, palette::SEV_INFO);
    }

    #[test]
    fn test_print_findings_empty_no_panic() {
        no_color();
        let mut p = LineStreamPrinter::new();
        p.print_findings(&[]);
    }

    #[test]
    fn test_print_findings_single_group_no_panic() {
        no_color();
        let mut p = LineStreamPrinter::new();
        let groups = vec![(
            "review".to_string(),
            vec![
                make_finding("critical", "SQL Injection", "Body", "scanner"),
                make_finding("info", "Notice", "", ""),
            ],
        )];
        p.print_findings(&groups);
    }

    #[test]
    fn test_print_findings_multi_group_no_panic() {
        no_color();
        let mut p = LineStreamPrinter::new();
        let groups = vec![
            (
                "security".to_string(),
                vec![make_finding("critical", "X", "B", "s")],
            ),
            (
                "perf".to_string(),
                vec![make_finding("high", "Y", "B", "p")],
            ),
        ];
        p.print_findings(&groups);
    }
}
