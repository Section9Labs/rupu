//! `LineStreamPrinter` — the default output UI for `rupu`.
//!
//! Writes a streaming vertical timeline to stdout, line-by-line.
//! Works in any terminal, any pipe, and any CI runner — no alt-screen
//! takeover. Colors come from the Okesu palette and auto-degrade when
//! stdout is not a TTY or `NO_COLOR=1` is set.
//!
//! Animation: a single `indicatif::ProgressBar` (the "ticker") may
//! occupy the bottom row of the visible stream, owned by an
//! `indicatif::MultiProgress` group. All printed lines go through
//! `multi.println` so the ticker is cleared and re-rendered around
//! each emission — that's the contract that lets us avoid the
//! cursor-save races that killed the previous spinner attempt. When
//! stdout is not a TTY (`MultiProgress` detects this), the ticker
//! draws nothing and `multi.println` falls through to the regular
//! print stream — pipes and CI runners get clean output.

use super::palette::{
    self, Status, AWAITING, BRAND, BRAND_300, COMPLETE, DIM, FAILED, RUNNING, SEPARATOR, TOOL_ARROW,
};
use super::spinner::{Spinner, SpinnerHandle};
use chrono::{DateTime, Utc};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rupu_orchestrator::FindingRecord;
use std::io::{self, IsTerminal, Write};
use std::time::Duration;

// ── Tree characters ──────────────────────────────────────────────────────────

const BRANCH: &str = "├─";
/// Light vertical thread for rupu chrome (step headers, footers, the
/// continuous rail). Soft purple — easy on the eye, doesn't compete
/// with the agent body it surrounds.
const PIPE: &str = "│";
/// Heavier vertical bar for agent body lines (assistant chunks).
/// Visual delineation between "rupu emitted this" and "the agent
/// emitted this" without needing any explicit framing — the eye reads
/// it as one continuous gutter on the left of the streamed output.
const BAR_HEAVY: &str = "┃";
/// Frame opener — pairs with [`FRAME_BOT`] to form a Slack/Discord-style
/// callout around an agent's run. The top edge carries the agent name;
/// the bottom edge carries the close summary.
const FRAME_TOP: &str = "╭─";
/// Frame closer — see [`FRAME_TOP`].
const FRAME_BOT: &str = "╰─";
/// One indent level = `│` + this padding. Sized so the inner rail of
/// indent N+1 lands in the same column as the body bar `┃` of the
/// frame at indent N — i.e. the parent panel's body bar IS the child's
/// inner rail. Without this alignment the frame opens at col 3·N+2,
/// children's rails sit at col 3·(N+1), and the eye reads a 1-column
/// jog where the body column "disappears" through the children.
const SPACE: &str = " ";

/// Number of trailing `─` characters drawn after the agent name in the
/// frame opener. Just enough to read as a section rule without crowding
/// the meta tail.
const FRAME_RULE_DASHES: usize = 6;

const DIM_CLOSE: &str = "\x1b[0m";

/// Fallback terminal width when stdout is not a tty (pipe, CI). Same
/// number `comfy-table` uses for headless renders. Long agent lines
/// in non-tty mode wrap to this width with continuation prefix —
/// pipelines that grep / wc / etc. on rupu output get sensible row
/// lengths instead of one 4000-char line.
const FALLBACK_TERM_WIDTH: u16 = 100;

// ── LineStreamPrinter ────────────────────────────────────────────────────────

/// Line-stream timeline printer. Writes to `stdout`.
///
/// Indent depth is managed by `push_indent` / `pop_indent` for
/// nested panel runs. The optional `ticker` field hosts an animated
/// indicatif spinner on the bottom row when something is in flight;
/// callers control it via [`Self::start_ticker`], [`Self::tick_with`],
/// and [`Self::stop_ticker`]. All printed output is routed through
/// `multi.println` so the ticker redraws cleanly around each line.
pub struct LineStreamPrinter {
    indent: usize,
    step_start: Option<std::time::Instant>,
    multi: MultiProgress,
    ticker: Option<ProgressBar>,
    /// True when stdout is an interactive TTY. When false, all `out` /
    /// `out_partial` calls bypass `MultiProgress` entirely so non-TTY
    /// consumers (pipes, cargo-test capture, CI logs) get a plain
    /// stdout stream with no spinner control codes.
    is_tty: bool,
    /// Terminal column count, captured at construction. Used to wrap
    /// long agent body lines so the visual gutter stays continuous
    /// (terminal-side wrap drops the leading prefix on continuation
    /// rows). Falls back to [`FALLBACK_TERM_WIDTH`] when the size
    /// probe fails (non-tty, sandboxed terminal).
    term_width: u16,
    /// UI preferences (color / theme / pager). Cached at construction
    /// so we don't re-read the config on every assistant chunk —
    /// streaming hot paths fire dozens of these per step.
    prefs: crate::cmd::ui::UiPrefs,
}

impl Default for LineStreamPrinter {
    fn default() -> Self {
        Self::new()
    }
}

impl LineStreamPrinter {
    pub fn new() -> Self {
        // Only animate when stdout is an interactive TTY. In every
        // other case (pipe, redirected file, cargo-test stdout capture,
        // CI runner) we use a hidden draw target — the ticker becomes
        // a no-op and `out` falls through to plain `println!` via the
        // multi-progress `println` path, which writes to the real
        // stdout regardless of the draw target. This keeps the line
        // stream clean for downstream consumers and avoids the
        // tick-thread / capture-pipe deadlocks that bit the v0.4.x
        // attempt at this same feature.
        let is_tty = io::stdout().is_terminal();
        let target = if is_tty {
            indicatif::ProgressDrawTarget::stdout()
        } else {
            indicatif::ProgressDrawTarget::hidden()
        };
        let multi = MultiProgress::with_draw_target(target);
        // Probe terminal width once. crossterm's `terminal::size()`
        // queries the controlling tty; on non-tty we fall through to
        // the documented fallback. The width is treated as static for
        // the run — handling SIGWINCH mid-stream would force a
        // re-layout that the line-stream model doesn't support.
        let term_width = terminal::size()
            .map(|(w, _)| w)
            .unwrap_or(FALLBACK_TERM_WIDTH);
        let prefs = crate::output::diag::prefs_for_diag(false);
        Self {
            indent: 0,
            step_start: None,
            multi,
            ticker: None,
            is_tty,
            term_width,
            prefs,
        }
    }

    // ── Print routing ────────────────────────────────────────────────────
    // All `print*` calls in this module go through these helpers instead
    // of `print!` / `println!` directly. That gives `MultiProgress` a
    // chance to clear the ticker, write the line, and re-render the
    // ticker — preventing the visual interleaving that broke the
    // previous spinner attempt.

    /// Print a line above the ticker (or to stdout when no ticker /
    /// not a TTY). Strips a single trailing newline if present so we
    /// don't end up with a double blank.
    fn out(&self, line: &str) {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        if self.is_tty {
            // Coordinate with the ticker so the spinner row gets
            // cleared and re-rendered around this line.
            if self.multi.println(trimmed).is_err() {
                println!("{trimmed}");
            }
        } else {
            // Non-TTY (pipe, cargo-test capture, CI). The hidden
            // draw target would swallow `multi.println` lines, so go
            // direct to stdout instead. No ticker is up in this
            // branch anyway — `start_ticker` is gated on `is_tty`.
            println!("{trimmed}");
        }
    }

    /// Print a partial line (no newline). Used for the approval-prompt
    /// inline tail. Bypasses `MultiProgress` since we need cursor to
    /// stay on the same line for the keypress.
    fn out_partial(&self, line: &str) {
        if self.is_tty {
            // Suspend the ticker while we hold an inline cursor so it
            // can't erase the prompt before the user sees it.
            self.multi.suspend(|| {
                print!("{line}");
                let _ = io::stdout().flush();
            });
        } else {
            print!("{line}");
            let _ = io::stdout().flush();
        }
    }

    // ── Ticker control ───────────────────────────────────────────────────

    /// Start the bottom-row ticker with the given message. No-op when
    /// a ticker is already running; use [`Self::tick_with`] to update
    /// the message instead.
    ///
    /// The ticker is rendered with the active body prefix (`│ ┃ `) so
    /// it visually continues the in-flight step's frame rather than
    /// floating loose at the bottom. Three live components:
    /// 1. spinner glyph — auto-rotates via `enable_steady_tick`,
    /// 2. caller-supplied message — updated via [`Self::tick_with`],
    /// 3. elapsed time — auto-rendered via indicatif's `{elapsed}`.
    ///
    /// Together they read as "this step is alive and N seconds in".
    pub fn start_ticker(&mut self, message: impl Into<String>) {
        if !self.is_tty {
            // No terminal to animate against. Bail before any
            // indicatif setup so non-TTY consumers (pipes, CI, cargo
            // test capture) get a clean stdout stream.
            return;
        }
        if let Some(pb) = &self.ticker {
            // Already running — update the message in place rather
            // than double-starting (which would put two bars on the
            // bottom row). This makes `start_ticker` idempotent and
            // lets workflow_printer keep a workflow-level ticker
            // alive across step boundaries: per-step `start_ticker`
            // calls update the message, then per-step `stop_ticker`
            // tears it down at end of step, and the workflow-level
            // poll loop re-arms it before the next sleep.
            pb.set_message(message.into());
            return;
        }
        let pb = self.multi.add(ProgressBar::new_spinner());
        // Body-prefix-aligned template. Indicatif treats anything
        // outside `{...}` as a literal — including the 24-bit ANSI
        // escapes inside the prefix string — so it renders the
        // gutter exactly the way `assistant_chunk` does, with the
        // spinner glyph + message picking up where streaming text
        // would have. The trailing `  · {elapsed}` adds a live
        // running counter dimmed with `:.dim` so the operator sees
        // both that the spinner is moving AND that real time is
        // passing (the glyph alone reads as "running" but doesn't
        // disambiguate stuck-vs-slow).
        let mut body_prefix = String::new();
        self.push_body_prefix(&mut body_prefix);
        let template = build_ticker_template(&body_prefix);
        pb.set_style(
            ProgressStyle::with_template(&template)
                .unwrap_or_else(|_| ProgressStyle::default_spinner())
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
        );
        pb.set_message(message.into());
        // ~80 ms per tick reads as a smooth pulse without flooding
        // the terminal. The steady-tick thread also re-renders
        // `{elapsed}` so the time counter updates roughly 12× per
        // second — fast enough to feel live, slow enough to stay
        // out of the way.
        pb.enable_steady_tick(Duration::from_millis(80));
        self.ticker = Some(pb);
    }

    /// Update the ticker's message in place. No-op when no ticker is
    /// running — caller can blindly call this on every event without
    /// having to re-check state.
    pub fn tick_with(&self, message: impl Into<String>) {
        if let Some(pb) = &self.ticker {
            pb.set_message(message.into());
        }
    }

    /// Stop and clear the bottom-row ticker. Idempotent — safe to call
    /// from event paths that may or may not have started a ticker.
    pub fn stop_ticker(&mut self) {
        if let Some(pb) = self.ticker.take() {
            pb.finish_and_clear();
        }
    }

    /// Hand out a clone of the underlying `MultiProgress`. Cheap —
    /// `MultiProgress` is `Arc`-backed internally. Used by the
    /// `AskDecider` so it can call `multi.suspend(...)` around the
    /// stderr permission prompt: indicatif rewrites the spinner row
    /// via `\r` on stdout, but `\r` is a terminal-level cursor move
    /// — it clobbers ANY content on that row, including a prompt
    /// the agent runtime just wrote to stderr. Suspending freezes
    /// the bars until the prompt resolves so the operator can
    /// actually see (and respond to) `[y/n/a/s]:`.
    pub fn multi_handle(&self) -> MultiProgress {
        self.multi.clone()
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
        self.out(&buf);
        // Continuous rail: even the gap line carries the pipe so the
        // user's eye doesn't lose the column.
        self.print_rail_only();
    }

    /// `▶ <agent_name>  (<provider> · <model>)  <run_id>`
    pub fn agent_header(&mut self, agent_name: &str, provider: &str, model: &str, run_id: &str) {
        let mut buf = String::new();
        let _ = palette::write_colored(&mut buf, "▶", BRAND);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, agent_name, BRAND);
        buf.push_str("  ");
        let meta = format!("({provider} · {model})");
        let _ = palette::write_colored(&mut buf, &meta, DIM);
        buf.push_str("  ");
        let _ = palette::write_colored(&mut buf, run_id, DIM);
        self.out(&buf);
        self.print_rail_only();
    }

    /// `▶ session attach  <agent_name>  <session_id>`
    pub fn session_header(&mut self, session_id: &str, agent_name: &str) {
        let mut buf = String::new();
        let _ = palette::write_colored(&mut buf, "▶", BRAND);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, "session attach", BRAND);
        buf.push_str("  ");
        let _ = palette::write_bold_colored(&mut buf, agent_name, BRAND);
        buf.push_str("  ");
        let _ = palette::write_colored(&mut buf, session_id, DIM);
        self.out(&buf);
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

        // Frame opener: `├─╭─ ◐ <headline> ────  step: <id>  (provider · model)`
        // The agent name is the headline when present (matt's preferred
        // reading: "I want to see *which* agent is running"), with the
        // workflow step_id demoted to the dim meta tail. When no agent
        // is wired (utility steps), step_id IS the headline and there
        // is no `step:` tail.
        let mut buf = String::new();
        self.push_frame_open(&mut buf);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, "◐", RUNNING);
        buf.push(' ');
        let headline = agent.unwrap_or(step_id);
        let _ = palette::write_bold_colored(&mut buf, headline, RUNNING);
        buf.push(' ');
        let rule = "─".repeat(FRAME_RULE_DASHES);
        let _ = palette::write_colored(&mut buf, &rule, BRAND_300);

        // Meta tail. When the agent is shown as headline, surface the
        // step_id so the operator can correlate to the YAML; otherwise
        // skip it (it's already the headline).
        let mut meta_parts: Vec<String> = Vec::new();
        if agent.is_some() {
            meta_parts.push(format!("step: {step_id}"));
        }
        let provider_model: Vec<&str> = [provider, model].iter().filter_map(|o| *o).collect();
        if !provider_model.is_empty() {
            meta_parts.push(format!("({})", provider_model.join(" · ")));
        }
        if !meta_parts.is_empty() {
            buf.push_str("  ");
            let _ = palette::write_colored(&mut buf, &meta_parts.join("  "), DIM);
        }
        self.out(&buf);
        // Light up the bottom-row ticker so the operator knows the
        // step is in flight even before any chunks arrive. The
        // workflow_printer can call `tick_with` to refine the message
        // as the LLM transitions through tool calls + assistant
        // chunks. Stopped automatically by `step_done` / `step_failed`.
        self.start_ticker(format!("running {step_id}…"));

        Spinner::start()
    }

    /// `├─╭─ ◐ <step_id> ────  (<kind_label> · N items)`
    ///
    /// Opener for `for_each` / `parallel` fan-out steps — establishes
    /// the parent frame so child iterations can render at indent+1.
    /// `kind_label` is the YAML keyword (`for_each`, `parallel`) so
    /// the operator can correlate the rendered shape to the workflow.
    pub fn fanout_start(&mut self, step_id: &str, kind_label: &str, count: usize) -> SpinnerHandle {
        self.step_start = Some(std::time::Instant::now());

        let mut buf = String::new();
        self.push_frame_open(&mut buf);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, "◐", RUNNING);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, step_id, RUNNING);
        buf.push(' ');
        let rule = "─".repeat(FRAME_RULE_DASHES);
        let _ = palette::write_colored(&mut buf, &rule, BRAND_300);
        let item_word = match kind_label {
            "parallel" => {
                if count == 1 {
                    "sub-step"
                } else {
                    "sub-steps"
                }
            }
            _ => {
                if count == 1 {
                    "item"
                } else {
                    "items"
                }
            }
        };
        let meta = format!("  ({kind_label} · {count} {item_word})");
        let _ = palette::write_colored(&mut buf, &meta, DIM);
        self.out(&buf);
        self.start_ticker(format!("running {step_id} ({kind_label} · {count})…"));

        Spinner::start()
    }

    /// `├─╰─ ✓ done · S/T · <duration>` — closer for fan-out steps.
    /// Shows success/total ratio so a partial-failure run reads clearly.
    pub fn fanout_done(
        &mut self,
        step_id: &str,
        success: bool,
        success_count: usize,
        total: usize,
        duration: Duration,
    ) {
        self.stop_ticker();
        let elapsed = self
            .step_start
            .take()
            .map(|s| s.elapsed())
            .unwrap_or(duration);
        let dur_str = format_duration(elapsed);
        let mut buf = String::new();
        self.push_frame_close(&mut buf);
        buf.push(' ');
        let (glyph, color) = if success {
            ("✓", COMPLETE)
        } else {
            ("✗", FAILED)
        };
        let _ = palette::write_bold_colored(&mut buf, glyph, color);
        buf.push(' ');
        let closure = if success { "done" } else { "failed" };
        let tally = format!("{closure} · {success_count}/{total} · {dur_str}");
        let _ = palette::write_colored(&mut buf, &tally, color);
        let _ = palette::write_colored(&mut buf, &format!("  ({step_id})"), DIM);
        self.out(&buf);
        self.print_rail_only();
    }

    /// `├─ ⧗ panel: <step_id>  (N panelists)`
    ///
    /// Special opener for panel steps — sets visual expectation that
    /// children will follow indented. Returns a no-op handle.
    pub fn panel_start(&mut self, step_id: &str, panelists: usize) -> SpinnerHandle {
        self.step_start = Some(std::time::Instant::now());

        let mut buf = String::new();
        self.push_frame_open(&mut buf);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, "◐", RUNNING);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, step_id, RUNNING);
        buf.push(' ');
        let rule = "─".repeat(FRAME_RULE_DASHES);
        let _ = palette::write_colored(&mut buf, &rule, BRAND_300);
        let meta = format!("  (panel · {panelists} panelists)");
        let _ = palette::write_colored(&mut buf, &meta, DIM);
        self.out(&buf);
        // Panel runs N panelists in parallel — phrase the message
        // accordingly so the operator doesn't think we've stalled
        // before any panelist has reported.
        self.start_ticker(format!(
            "running {step_id} panel ({panelists} panelist{plural})…",
            plural = if panelists == 1 { "" } else { "s" }
        ));

        Spinner::start()
    }

    /// Render one panelist child line under a panel step: status glyph,
    /// agent name, optional one-line summary, and findings count.
    pub fn panelist_line(&mut self, agent: &str, success: bool, findings_count: usize) {
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
        self.out(&buf);
    }

    /// Compact tree row for focused fan-out / dispatch rendering.
    /// Uses the current indent level and a simple branch glyph rather
    /// than opening a full nested frame.
    pub fn tree_item(&mut self, label: &str, status: Status, detail: Option<&str>) {
        let mut buf = String::new();
        self.push_indent_pipes(&mut buf);
        let _ = palette::write_colored(&mut buf, PIPE, BRAND_300);
        buf.push_str("  ");
        let _ = palette::write_colored(&mut buf, BRANCH, BRAND_300);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, &status.glyph().to_string(), status.color());
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, label, status.color());
        if let Some(detail) = detail.filter(|detail| !detail.trim().is_empty()) {
            buf.push_str("  ");
            let _ = palette::write_colored(&mut buf, detail, DIM);
        }
        self.out(&buf);
    }

    /// Continuation detail for [`Self::tree_item`].
    pub fn tree_note(&mut self, message: &str) {
        let mut buf = String::new();
        self.push_indent_pipes(&mut buf);
        let _ = palette::write_colored(&mut buf, PIPE, BRAND_300);
        buf.push_str("  ");
        let _ = palette::write_colored(&mut buf, PIPE, BRAND_300);
        buf.push_str("   ");
        let _ = palette::write_colored(&mut buf, message, DIM);
        self.out(&buf);
    }

    /// Footer for a panelist child frame (replaces the legacy
    /// one-line `panelist_line` summary when the panelist has its own
    /// child frame open). Same shape as `step_done` but the meta
    /// shows findings count instead of token count — that's the
    /// semantically interesting tally for a reviewer.
    pub fn panelist_done(&mut self, sub_id: &str, findings_count: usize, duration: Duration) {
        let dur_str = format_duration(duration);
        let mut buf = String::new();
        self.push_frame_close(&mut buf);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, "✓", COMPLETE);
        buf.push(' ');
        let tally = if findings_count == 1 {
            format!("done · 1 finding · {dur_str}")
        } else {
            format!("done · {findings_count} findings · {dur_str}")
        };
        let _ = palette::write_colored(&mut buf, &tally, COMPLETE);
        let _ = palette::write_colored(&mut buf, &format!("  ({sub_id})"), DIM);
        self.out(&buf);
        self.print_rail_only();
    }

    /// Print a phase separator. Caller is responsible for any preceding
    /// rail line (step_done already emits one).
    pub fn phase_separator(&mut self) {
        let mut buf = String::new();
        self.push_content_prefix(&mut buf);
        let line = "──────────────────────";
        let _ = palette::write_colored(&mut buf, line, SEPARATOR);
        self.out(&buf);
        self.print_rail_only();
    }

    /// One assistant-text chunk. Highlights the body via syntect's
    /// markdown grammar (so fenced code blocks, headings, lists, etc.
    /// pick up color) and wraps each line to the terminal width with
    /// a continuation prefix — without that wrap, the terminal's own
    /// soft-wrap drops the leading gutter glyph on the wrapped row
    /// and visually breaks the timeline.
    ///
    /// Body lines render with the heavier `┃` bar (vs. the `│` rupu
    /// uses for chrome) so a reader scanning the stream sees agent
    /// output and rupu structure as two distinct columns.
    ///
    /// Preserves blank lines as rail-only lines so the visual column
    /// never breaks across paragraphs.
    pub fn assistant_chunk(&mut self, chunk: &str) {
        // Refresh the ticker so the operator sees the model is actively
        // emitting tokens (even if the per-chunk lines are short and
        // scroll fast). No-op when no ticker is up (e.g. replay mode).
        self.tick_with("model streaming…");

        // Highlight as markdown. syntect retains state across the
        // chunk's internal newlines, so a chunk that opens a fenced
        // code block keeps the "code body" coloring even after
        // newlines. State is dropped at chunk boundaries — fine in
        // practice since the LLM rarely splits a single fenced block
        // across stream chunks.
        let highlighted = crate::cmd::ui::highlight_markdown(chunk, &self.prefs);

        // Wrap to (term_width - body_prefix_width). Compute once per
        // chunk; indent depth is stable for the duration of the
        // chunk.
        let avail = self
            .term_width
            .saturating_sub(self.body_prefix_visual_width() as u16)
            .max(20) as usize;

        for line in highlighted.split('\n') {
            // Visible-len 0 means truly blank line. Strip ANSI to
            // verify rather than checking byte-len, which a
            // colored-but-empty rendering still has > 0. Use the
            // body-blank prefix (rail + body bar) instead of the
            // rail-only one — paragraph breaks inside an agent
            // response need to KEEP the body bar so the visual
            // column doesn't dissolve mid-thought.
            if visible_len(line) == 0 {
                self.print_body_blank();
                continue;
            }
            for piece in wrap_with_ansi(line, avail) {
                let mut buf = String::new();
                self.push_body_prefix(&mut buf);
                buf.push_str(&piece);
                self.out(&buf);
            }
        }
    }

    /// Tool call: `│  → <tool>  <summary>`
    pub fn tool_call(&mut self, tool: &str, summary: &str) {
        // Update the bottom-row ticker with the current tool's name so
        // the operator sees what's running while the call is in flight.
        self.tick_with(format!("running tool {tool}…"));
        let mut buf = String::new();
        self.push_content_prefix(&mut buf);
        let arrow = "→ ";
        let _ = palette::write_colored(&mut buf, arrow, TOOL_ARROW);
        buf.push_str(tool);
        if !summary.is_empty() {
            buf.push_str("  ");
            let _ = palette::write_colored(&mut buf, summary, DIM);
        }
        self.out(&buf);
    }

    /// Sideband status event emitted by a live attached workflow, used by
    /// callers like autoflow to surface tool-driven issue/PR updates in
    /// real time while preserving ticker coordination.
    pub fn sideband_event(&mut self, status: Status, label: &str, detail: Option<&str>) {
        let mut buf = String::new();
        self.push_content_prefix(&mut buf);
        let _ = palette::write_bold_colored(&mut buf, &status.glyph().to_string(), status.color());
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, label, status.color());
        if let Some(detail) = detail.filter(|detail| !detail.trim().is_empty()) {
            buf.push_str("  ");
            let _ = palette::write_colored(&mut buf, detail, DIM);
        }
        self.out(&buf);
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
        self.stop_ticker();
        let elapsed = self
            .step_start
            .take()
            .map(|s| s.elapsed())
            .unwrap_or(duration);
        let dur_str = format_duration(elapsed);
        let mut buf = String::new();
        self.push_frame_close(&mut buf);
        buf.push(' ');
        let (glyph, color) = if success {
            ("✓", COMPLETE)
        } else {
            ("✗", FAILED)
        };
        let _ = palette::write_bold_colored(&mut buf, glyph, color);
        buf.push(' ');
        // Closure word + tally + duration in the success/failure
        // color so the panel footer reads as one unmistakable line.
        let closure = if success { "done" } else { "failed" };
        let tally = if findings_count == 1 {
            format!("{closure} · 1 finding · {dur_str}")
        } else {
            format!("{closure} · {findings_count} findings · {dur_str}")
        };
        let _ = palette::write_colored(&mut buf, &tally, color);
        let _ = palette::write_colored(&mut buf, &format!("  ({step_id})"), DIM);
        self.out(&buf);
        self.print_rail_only();
    }

    /// `│  ✓ <step_id>  done · <duration> · <tokens> tokens`
    ///
    /// Step closure footer — the entire line renders in `COMPLETE`
    /// green so the eye registers it as a cleared phase, not a
    /// continuation. Header glyph (`◐`) intentionally stays static
    /// per v0.4.8 lessons (cursor-save/restore fights with the
    /// print thread); the prominent footer is the closure cue.
    pub fn step_done(&mut self, step_id: &str, duration: Duration, total_tokens: u64) {
        self.stop_ticker();
        let elapsed = self
            .step_start
            .take()
            .map(|s| s.elapsed())
            .unwrap_or(duration);
        let dur_str = format_duration(elapsed);
        // Frame closer: `├─╰─ ✓ done · <dur> · <tokens>`. The `╰`
        // sits directly under the opener's `╭` so the agent's frame
        // reads as one continuous shape from open through body to
        // close. Whole footer in COMPLETE so the closure reads as a
        // signal line, not a scrollable meta row.
        let mut buf = String::new();
        self.push_frame_close(&mut buf);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, "✓", COMPLETE);
        buf.push(' ');
        let meta = if total_tokens > 0 {
            format!("done · {dur_str} · {total_tokens} tokens")
        } else {
            format!("done · {dur_str}")
        };
        let _ = palette::write_colored(&mut buf, &meta, COMPLETE);
        // step_id only useful when it differs from the headline (panels,
        // utility steps without an agent). Trailing dim tail.
        let _ = palette::write_colored(&mut buf, &format!("  ({step_id})"), DIM);
        self.out(&buf);
        self.print_rail_only();
    }

    /// `│  ✗ <step_id>  failed: <reason>`
    ///
    /// Step failure footer — entire line in `FAILED` red so the
    /// failure is unmissable. Same prominence treatment as
    /// [`Self::step_done`].
    pub fn step_failed(&mut self, step_id: &str, reason: &str) {
        self.stop_ticker();
        self.step_start = None;
        let mut buf = String::new();
        self.push_frame_close(&mut buf);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, "✗", FAILED);
        buf.push(' ');
        let msg = format!("failed: {reason}");
        let _ = palette::write_bold_colored(&mut buf, &msg, FAILED);
        let _ = palette::write_colored(&mut buf, &format!("  ({step_id})"), DIM);
        self.out(&buf);
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
        // Suspend the ticker — it could overwrite the inline `[a/r/v/q]:`
        // tail or eat the user's typed character before crossterm reads it.
        // We're paused waiting on input; nothing's happening that needs a
        // ticker anyway. The next step will start a fresh one.
        self.stop_ticker();
        let mut buf = String::new();
        self.push_prefix(&mut buf, BRANCH);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, "⏸", AWAITING);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, step_id, AWAITING);
        self.out(&buf);

        for line in prompt.split('\n') {
            if line.is_empty() {
                self.print_rail_only();
            } else {
                let mut b = String::new();
                self.push_content_prefix(&mut b);
                b.push_str(line);
                self.out(&b);
            }
        }

        // Options + key affordance, both amber.
        let options_str = "[a] approve   [r] reject   [v] view findings   [q] detach";
        let mut b = String::new();
        self.push_content_prefix(&mut b);
        let _ = palette::write_colored(&mut b, options_str, AWAITING);
        self.out(&b);

        // Inline marker that names the keys — no `> ` (which reads as
        // "type something here").
        let mut b = String::new();
        self.push_content_prefix(&mut b);
        let _ = palette::write_bold_colored(&mut b, "[a/r/v/q]: ", AWAITING);
        self.out_partial(&b);

        let ch = if io::stdin().is_terminal() {
            read_single_key()?
        } else {
            read_line_first_char()?
        };

        // Echo the chosen character and a newline.
        let mut echo = String::new();
        let _ = palette::write_bold_colored(&mut echo, &ch.to_string(), AWAITING);
        self.out(&echo);
        Ok(ch)
    }

    /// Optional reason prompt for reject. Returns the typed line.
    /// Line-buffered: the user really does need to type a sentence here.
    pub fn reject_reason_prompt(&mut self) -> io::Result<String> {
        let mut b = String::new();
        self.push_content_prefix(&mut b);
        b.push_str("Reason (optional, Enter to skip): ");
        self.out_partial(&b);
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
            self.out(&b);
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
        self.out(&hdr);
        self.print_rail_only();

        for (step_id, findings) in groups {
            if groups.len() > 1 && !step_id.is_empty() {
                // Step subhead.
                let mut sub = String::new();
                self.push_content_prefix(&mut sub);
                let label = format!("from {} ({})", step_id, findings.len());
                let _ = palette::write_colored(&mut sub, &label, DIM);
                self.out(&sub);
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
                let _ =
                    palette::write_bold_colored(&mut b, &f.title, palette::Status::Active.color());
                self.out(&b);

                // Source.
                if !f.source.is_empty() {
                    let mut src = String::new();
                    self.push_content_prefix(&mut src);
                    src.push_str("    ");
                    let stext = format!("source: {}", f.source);
                    let _ = palette::write_colored(&mut src, &stext, DIM);
                    self.out(&src);
                }

                // Body (wrapped).
                if !f.body.is_empty() {
                    for line in wrap_text(&f.body, 76) {
                        let mut bl = String::new();
                        self.push_content_prefix(&mut bl);
                        bl.push_str("    ");
                        let _ = palette::write_colored(&mut bl, &line, DIM);
                        self.out(&bl);
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
        self.stop_ticker();
        self.out("");
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
        self.out(&buf);
    }

    /// `✗ <workflow_name> failed  <run_id>  error: <error>`
    pub fn workflow_failed(&mut self, workflow_name: &str, run_id: &str, error: &str) {
        self.stop_ticker();
        self.out("");
        let mut buf = String::new();
        let _ = palette::write_bold_colored(&mut buf, "✗", FAILED);
        buf.push(' ');
        let _ = palette::write_bold_colored(&mut buf, workflow_name, FAILED);
        buf.push_str(" failed  ");
        let _ = palette::write_colored(&mut buf, run_id, DIM);
        buf.push_str("  ");
        let msg = format!("error: {error}");
        let _ = palette::write_colored(&mut buf, &msg, FAILED);
        self.out(&buf);
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
        // Color the branch character in the same BRAND_300 as the
        // vertical thread so the timeline reads as one continuous
        // brand-tinted skeleton (was uncolored, rendering in the
        // terminal default fg and visually breaking the gutter).
        let _ = palette::write_colored(buf, branch, BRAND_300);
    }

    /// `├─╭─` — frame opener, fused to the workflow rail. Produces a
    /// 4-cell prefix that places `╭` in column 3, directly above the
    /// body bar `┃` so the open/body/close form one continuous frame.
    fn push_frame_open(&self, buf: &mut String) {
        self.push_indent_pipes(buf);
        let _ = palette::write_colored(buf, BRANCH, BRAND_300);
        let _ = palette::write_colored(buf, FRAME_TOP, BRAND_300);
    }

    /// `│ ╰─` — frame closer that continues the body's vertical
    /// column rather than branching off the parent rail. The earlier
    /// shape `├─╰─` placed `├` at col 3·N (with a rightward `─` stub
    /// at col 3·N+1) and `╰` at col 3·N+2 — column-correct on paper
    /// but the horizontal `─` strokes broke the eye's vertical
    /// reading of the body bar `┃`. The new shape keeps the inner
    /// rail `│` at col 3·N (matching the body lines above) and bends
    /// `╰─` from col 3·N+2 (where `┃` was) into the close content.
    /// Same prefix width (4 visible cells past indent_pipes), same
    /// content column — pure visual cleanup, callers don't change.
    fn push_frame_close(&self, buf: &mut String) {
        self.push_indent_pipes(buf);
        let _ = palette::write_colored(buf, PIPE, BRAND_300);
        buf.push(' ');
        let _ = palette::write_colored(buf, FRAME_BOT, BRAND_300);
    }

    fn push_content_prefix(&self, buf: &mut String) {
        self.push_indent_pipes(buf);
        let _ = palette::write_colored(buf, PIPE, BRAND_300);
        buf.push_str("  ");
    }

    /// Body-content prefix — rail + frame-bar (`│ ┃  `) in BRAND_300/BRAND.
    /// The rail keeps the workflow timeline continuous; the heavier `┃`
    /// sits directly under the frame opener's `╭` (column 3) so the
    /// agent's callout reads as one unbroken shape from open to close.
    /// Used by [`Self::assistant_chunk`].
    fn push_body_prefix(&self, buf: &mut String) {
        self.push_indent_pipes(buf);
        let _ = palette::write_colored(buf, PIPE, BRAND_300);
        buf.push(' ');
        let _ = palette::write_colored(buf, BAR_HEAVY, BRAND);
        buf.push_str("  ");
    }

    /// Visible-character width consumed by the body prefix at the
    /// current indent level. ANSI escape sequences are zero-width;
    /// only the actual glyphs count. Used for terminal-width-aware
    /// wrap math.
    fn body_prefix_visual_width(&self) -> usize {
        // Each indent level is `│` + 1 space = 2 visible cells (sized
        // so the inner rail at indent N+1 lands in the body-bar column
        // of the frame at indent N — see SPACE doc). Body prefix adds
        // `│ ┃  ` = 5 visible cells (rail + space + bar + 2 spaces).
        self.indent * 2 + 5
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
        self.out(&buf);
    }

    /// Print the body prefix (`<indent_pipes>│ ┃  `) with no content,
    /// used for blank lines INSIDE an agent's response so the body
    /// bar `┃` stays visually continuous through paragraph breaks.
    /// `print_rail_only` would drop the body bar (rendering only
    /// `<indent_pipes>│`), which makes a paragraph gap look like the
    /// frame ended — confusing when the agent is mid-response.
    fn print_body_blank(&self) {
        let mut buf = String::new();
        self.push_body_prefix(&mut buf);
        // Trim the trailing `  ` from push_body_prefix's output so a
        // blank body line doesn't leave invisible whitespace at EOL
        // that some terminals render as a soft underline.
        let trimmed = buf.trim_end_matches(' ').to_string();
        self.out(&trimmed);
    }
}

// ── Formatting helpers ────────────────────────────────────────────────────────

/// Build the indicatif `ProgressStyle` template for the bottom-row
/// ticker, body-prefix-aligned. The trailing 2-space content margin
/// of `body_prefix` is replaced with a single space so the spinner
/// glyph sits where the first body-text character would — the
/// spinner IS the row's "first content character". Three live slots:
/// `{spinner}` (rotating glyph), `{msg}` (caller-supplied via
/// [`LineStreamPrinter::tick_with`]), `{elapsed}` (auto-rendered by
/// indicatif). Free function so we can unit-test the literal shape
/// without spinning up a real terminal.
fn build_ticker_template(body_prefix: &str) -> String {
    let trimmed = body_prefix.trim_end_matches(' ');
    let dim = palette::themed(DIM);
    let spinner = palette::themed(RUNNING);
    let dim_open = format!("\x1b[38;2;{};{};{}m", dim.0, dim.1, dim.2);
    let spinner_open = format!("\x1b[38;2;{};{};{}m", spinner.0, spinner.1, spinner.2);
    format!(
        "{trimmed} {spinner_open}{{spinner}}{DIM_CLOSE} {{msg}}  {dim_open}· {{elapsed}}{DIM_CLOSE}"
    )
}

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
        "high" => (palette::SEV_HIGH, true),
        "medium" => (palette::SEV_MEDIUM, false),
        "low" => (palette::SEV_LOW, false),
        _ => (palette::SEV_INFO, false),
    }
}

/// Naïve word-wrap: split on whitespace, re-join into lines ≤ `width` chars.
/// Visible (printed) char count of a string that may contain ANSI
/// CSI sequences (`ESC [ … m`). Each `ESC[…m` run is zero-width;
/// every other `char` counts once. Conservative — doesn't try to
/// account for double-width CJK / emoji glyphs (treated as 1).
fn visible_len(s: &str) -> usize {
    let mut n = 0usize;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Eat the rest of the CSI: `[` + parameters + final 'm'.
            // Tolerant of malformed sequences — any char that arrives
            // before a closing `m` is just consumed.
            for inner in chars.by_ref() {
                if inner == 'm' {
                    break;
                }
            }
        } else {
            n += 1;
        }
    }
    n
}

/// Wrap a possibly-ANSI-colored string into pieces of at most
/// `width` *visible* characters, preserving in-place SGR (color)
/// state across the cuts.
///
/// Each emitted piece ends with a `\x1b[0m` reset (so a downstream
/// prefix in a different color doesn't pick up the leftover style)
/// and the next piece replays the latest active SGR at its start
/// (so continued colored text stays colored on every row).
///
/// Hard-break only — splits at exactly `width` visible chars
/// regardless of word boundaries. This keeps the implementation
/// short and predictable; the alternative (word-break preferred)
/// is a polish add-on we can layer on later if matt asks.
fn wrap_with_ansi(line: &str, width: usize) -> Vec<String> {
    if width == 0 || visible_len(line) <= width {
        return vec![line.to_string()];
    }

    let mut pieces: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_visible: usize = 0;
    let mut active_sgr = String::new(); // last SGR string we saw, e.g. "\x1b[38;2;…m"

    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Capture the full CSI run (assumes terminating `m`).
            let mut sgr = String::from("\x1b");
            for inner in chars.by_ref() {
                sgr.push(inner);
                if inner == 'm' {
                    break;
                }
            }
            // Reset clears the active style; everything else replaces it.
            if sgr == "\x1b[0m" {
                active_sgr.clear();
            } else {
                active_sgr = sgr.clone();
            }
            current.push_str(&sgr);
            continue;
        }

        if current_visible == width {
            // Adding `c` would overflow — close the current piece and
            // start a fresh one.
            if !active_sgr.is_empty() {
                current.push_str("\x1b[0m");
            }
            pieces.push(std::mem::take(&mut current));
            current_visible = 0;
            if !active_sgr.is_empty() {
                current.push_str(&active_sgr);
            }
        }

        current.push(c);
        current_visible += 1;
    }

    if !current.is_empty() {
        if !active_sgr.is_empty() {
            current.push_str("\x1b[0m");
        }
        pieces.push(current);
    }

    if pieces.is_empty() {
        pieces.push(String::new());
    }
    pieces
}

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
        // Indent 1 = `│ ` (2 cells, parent rail), then content prefix
        // `│  ` = total 5 cells. The inner `│` lands at col 2 — same
        // column as the parent frame's body bar `┃`.
        assert_eq!(buf, "│ │  ");
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
        // Indent 1: parent rail `│ ` (2 cells), then `├─`. The `├`
        // lands at col 2 — exactly where the parent panel's body bar
        // `┃` sits, so the branch reads as a clean tap into the
        // parent's body column.
        assert_eq!(buf, "│ ├─");
    }

    #[test]
    fn test_frame_open_at_indent0() {
        no_color();
        let p = LineStreamPrinter::new();
        let mut buf = String::new();
        p.push_frame_open(&mut buf);
        assert_eq!(buf, "├─╭─");
    }

    #[test]
    fn test_frame_close_at_indent0() {
        no_color();
        let p = LineStreamPrinter::new();
        let mut buf = String::new();
        p.push_frame_close(&mut buf);
        // `│ ╰─` continues the body's vertical column (col 0 = `│`,
        // col 2 = `╰`) instead of branching off the parent rail with
        // `├─`. Same width (4 visible cells) so callers don't shift.
        assert_eq!(buf, "│ ╰─");
    }

    #[test]
    fn test_frame_close_at_indent1() {
        no_color();
        let mut p = LineStreamPrinter::new();
        p.push_indent();
        let mut buf = String::new();
        p.push_frame_close(&mut buf);
        // At indent=1: parent rail (2 cells `│ `), then the inner
        // rail `│ ` (2 cells, lands at col 2 = parent body column),
        // then `╰─` bending into content at col 4. `╰` lands directly
        // under the indent-1 body bar `┃`.
        assert_eq!(buf, "│ │ ╰─");
    }

    #[test]
    fn test_body_prefix_at_indent1_aligns_under_indent1_frame_top() {
        // Frame open at indent=1 is `│ ├─╭─` (parent rail + branch
        // tap + frame top). The body bar `┃` should sit at col 4
        // (under the `╭`) so the indent=1 frame reads as one
        // continuous shape from open to body to close.
        no_color();
        let mut p = LineStreamPrinter::new();
        p.push_indent();
        let mut frame = String::new();
        p.push_frame_open(&mut frame);
        let mut body = String::new();
        p.push_body_prefix(&mut body);

        let frame_chars: Vec<char> = frame.chars().collect();
        let body_chars: Vec<char> = body.chars().collect();
        // Col 4: `╭` in the opener, `┃` in the body prefix.
        assert_eq!(frame_chars[4], '╭');
        assert_eq!(body_chars[4], '┃');
    }

    #[test]
    fn test_frame_open_at_indent1() {
        no_color();
        let mut p = LineStreamPrinter::new();
        p.push_indent();
        let mut buf = String::new();
        p.push_frame_open(&mut buf);
        // Indent 1: parent rail `│ ` (2 cells) directly feeds into
        // the branch `├─` at col 2 — same column as the parent's
        // body bar `┃` would occupy. The frame top `╭─` follows.
        assert_eq!(buf, "│ ├─╭─");
    }

    #[test]
    fn test_body_prefix_aligns_under_frame_top() {
        // The body bar `┃` must sit at column 3 — directly under the
        // frame opener's `╭` (also at column 3 in `├─╭─`). Anything
        // else breaks the visual continuity of the frame.
        no_color();
        let p = LineStreamPrinter::new();
        let mut frame = String::new();
        p.push_frame_open(&mut frame);
        let mut body = String::new();
        p.push_body_prefix(&mut body);

        // Visible-width must match (5 cells either way).
        assert_eq!(visible_len(&frame), 4); // ├─╭─ (no trailing space yet)
        assert_eq!(visible_len(&body), 5); // │ ┃ + 2 trailing spaces

        // Column-3 character: `╭` in the opener, `┃` in the body.
        let frame_chars: Vec<char> = frame.chars().collect();
        let body_chars: Vec<char> = body.chars().collect();
        assert_eq!(frame_chars[2], '╭');
        assert_eq!(body_chars[2], '┃');
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

    #[test]
    fn visible_len_strips_ansi() {
        // 12 visible chars, plus a 24-bit color sequence + reset.
        let s = "\x1b[38;2;163;190;140mhello, world\x1b[0m";
        assert_eq!(visible_len(s), 12);
    }

    #[test]
    fn visible_len_handles_plain_text() {
        assert_eq!(visible_len("hello"), 5);
        assert_eq!(visible_len(""), 0);
    }

    #[test]
    fn wrap_with_ansi_short_line_passes_through() {
        let pieces = wrap_with_ansi("hello world", 80);
        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0], "hello world");
    }

    #[test]
    fn wrap_with_ansi_hard_breaks_at_width() {
        // 13 visible chars wrapped at 7 → 7 + 6.
        let pieces = wrap_with_ansi("one two three", 7);
        assert_eq!(pieces.len(), 2);
        assert_eq!(visible_len(&pieces[0]), 7);
        assert_eq!(visible_len(&pieces[1]), 6);
    }

    #[test]
    fn wrap_with_ansi_preserves_active_color_across_wraps() {
        let s = "\x1b[38;2;100;100;100mhello world friend\x1b[0m";
        let pieces = wrap_with_ansi(s, 7);
        // 18 visible chars wrapped at 7 → 3 pieces (7 + 7 + 4).
        assert_eq!(pieces.len(), 3);
        for p in &pieces {
            assert!(p.starts_with("\x1b[38;2;100;100;100m"));
            assert!(p.ends_with("\x1b[0m"));
        }
    }

    #[test]
    fn wrap_with_ansi_three_segments_no_whitespace() {
        let pieces = wrap_with_ansi("abcdefghij", 4);
        assert_eq!(pieces.len(), 3);
        assert_eq!(visible_len(&pieces[0]), 4);
        assert_eq!(visible_len(&pieces[1]), 4);
        assert_eq!(visible_len(&pieces[2]), 2);
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

    // ── Ticker tests ─────────────────────────────────────────────────
    // Indicatif draws nothing under `cargo test` (stdout isn't a TTY,
    // and `MultiProgress::println` swallows output to a hidden draw
    // target), so we can't snapshot the rendered frames. What we CAN
    // verify is that the lifecycle methods don't panic in any order
    // and that `tick_with` is a safe no-op when no ticker is up.

    #[test]
    fn test_ticker_template_aligns_with_body_prefix() {
        // Ticker template should reuse the body prefix (rail + body
        // bar) so the spinner row visually continues the in-flight
        // step's frame, with the trailing 2-space content margin
        // collapsed to a single space (the spinner glyph IS the
        // first content character).
        no_color();
        let p = LineStreamPrinter::new();
        let mut prefix = String::new();
        p.push_body_prefix(&mut prefix);
        let tpl = build_ticker_template(&prefix);
        // Indent 0: body prefix is `│ ┃  ` (5 cells); template
        // collapses the trailing 2 spaces and wraps `{spinner}` in
        // themed ANSI escapes — net "│ ┃ {spinner} {msg}  · {elapsed}" (with ANSI
        // dim around the elapsed clause). Assert the salient slots
        // and the alignment cue (single space after `┃` before the
        // spinner placeholder).
        assert!(tpl.contains("│"));
        assert!(tpl.contains("┃"));
        assert!(tpl.contains("{spinner}"));
        assert!(tpl.contains("{msg}"));
        assert!(tpl.contains("{elapsed}"));
        // Body bar must be followed by exactly one space before the
        // spinner placeholder — anything else means the alignment
        // shift broke and the ticker no longer reads as the frame's
        // continuation.
        assert!(
            tpl.contains("┃ ") && tpl.contains("{spinner}"),
            "body bar should sit one space before the spinner placeholder: {tpl:?}"
        );
    }

    #[test]
    fn test_ticker_lifecycle_no_panic() {
        no_color();
        let mut p = LineStreamPrinter::new();
        p.start_ticker("running…");
        p.tick_with("now streaming…");
        p.stop_ticker();
        // Idempotent stop.
        p.stop_ticker();
    }

    #[test]
    fn test_ticker_double_start_updates_message() {
        no_color();
        let mut p = LineStreamPrinter::new();
        p.start_ticker("first message");
        // A second `start_ticker` call must NOT spawn a second
        // indicatif bar — it should update the existing one in place.
        // workflow_printer relies on this: it re-arms the ticker each
        // poll iteration to keep it alive across step boundaries, and
        // a step's own `start_ticker(running step1…)` overrides the
        // workflow-level message without spawning a second bar.
        p.start_ticker("second message");
        p.stop_ticker();
    }

    #[test]
    fn test_tick_with_no_ticker_is_noop() {
        no_color();
        let p = LineStreamPrinter::new();
        // Caller can blindly tick on every event without checking
        // whether a ticker is running — must not panic.
        p.tick_with("update");
    }

    #[test]
    fn test_step_done_clears_ticker_field() {
        // Under cargo-test stdout isn't a TTY so `start_ticker` is a
        // no-op; the assertion is that step_done leaves
        // `ticker = None` regardless of whether one was armed (the
        // teardown is idempotent).
        no_color();
        let mut p = LineStreamPrinter::new();
        let _h = p.step_start("step_a", None, None, None);
        p.step_done("step_a", Duration::from_secs(1), 100);
        assert!(p.ticker.is_none());
    }

    #[test]
    fn test_step_failed_clears_ticker_field() {
        no_color();
        let mut p = LineStreamPrinter::new();
        let _h = p.step_start("step_a", None, None, None);
        p.step_failed("step_a", "boom");
        assert!(p.ticker.is_none());
    }
}
