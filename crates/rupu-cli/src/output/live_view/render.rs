//! Terminal rendering for the redesigned `workflow run` live view.
//!
//! [`LiveRenderer`] diffs a `Vec<Line>` frame against the previously drawn
//! one and emits only the rows that changed (cursor-addressed), replacing the
//! old full-screen reprint. [`style_ansi`] maps a [`Segment`]'s [`Style`] onto
//! ANSI via the existing `palette` / `rupu_codename` color surfaces, and
//! [`AltScreen`] is the RAII guard that owns raw mode + the alternate screen.

use std::io::{self, Write};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::style::Print;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, queue};
use owo_colors::Rgb;

use crate::output::live_view::row::{Line, Segment, Style};
use crate::output::palette::{self, write_bold_colored, write_colored};

/// Near-white for [`Style::Strong`] (slate-100). Terminals are treated as
/// dark throughout (`rupu_codename`'s `dark_rgb` makes the same call).
const STRONG: Rgb = Rgb(241, 245, 249);
/// Neutral cyan for [`Style::Meter`] (sky-400): distinct from every status hue.
const METER: Rgb = Rgb(56, 189, 248);

/// The color a [`Style`] resolves to, or `None` for "terminal default".
///
/// Pure (no color-support probing), so the style → hue mapping is unit
/// testable independent of the process-wide color switch. `Crew` with a word
/// that is not a known crew color falls back to `None` (plain).
fn style_color(style: &Style) -> Option<Rgb> {
    match style {
        Style::Plain => None,
        Style::Dim => Some(palette::DIM),
        Style::Strong => Some(STRONG),
        Style::Status(s) => Some(s.color()),
        Style::Crew(word) => rupu_codename::crew_tint(word).map(|t| {
            let (r, g, b) = t.dark_rgb();
            Rgb(r, g, b)
        }),
        Style::Role(word) => {
            let (r, g, b) = rupu_codename::role_badge(word).tint.dark_rgb();
            Some(Rgb(r, g, b))
        }
        Style::Meter => Some(METER),
        Style::Danger => Some(palette::FAILED),
        Style::Good => Some(palette::COMPLETE),
    }
}

/// Append `seg.text` to `buf`, colored per `seg.style`.
///
/// Every escape goes through `palette::write_colored` /
/// `write_bold_colored`, so `NO_COLOR`, `--no-color`, `[ui].color = "never"`
/// and non-color terminals all yield the raw text with no escapes.
pub fn style_ansi(buf: &mut String, seg: &Segment) {
    // Writing into a `String` cannot fail; the `fmt::Result` is vestigial.
    let _ = match (style_color(&seg.style), &seg.style) {
        (None, _) => {
            buf.push_str(&seg.text);
            Ok(())
        }
        (Some(color), Style::Strong) => write_bold_colored(buf, &seg.text, color),
        (Some(color), _) => write_colored(buf, &seg.text, color),
    };
}

fn render_line(line: &Line) -> String {
    let mut s = String::new();
    for seg in &line.segments {
        style_ansi(&mut s, seg);
    }
    s
}

/// Diff-renders `Vec<Line>` frames: remembers the ANSI string drawn for each
/// row and, on the next frame, repaints only the rows that changed.
#[derive(Debug, Default)]
pub struct LiveRenderer {
    last: Vec<String>,
    /// Set by [`LiveRenderer::invalidate`]: the next `draw` clears the whole
    /// screen once and repaints every row.
    force_clear: bool,
}

impl LiveRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget what is on screen (e.g. after a terminal resize, which can
    /// reflow or discard rows behind the renderer's back): the next
    /// [`LiveRenderer::draw`] clears the screen once and repaints in full,
    /// then returns to diffing.
    pub fn invalidate(&mut self) {
        self.last.clear();
        self.force_clear = true;
    }

    /// Draw `frame`, emitting cursor-addressed output only for rows whose
    /// rendered text differs from the previous frame, plus a clear for each
    /// trailing row the previous frame had and this one does not. Rows are
    /// emitted as-is — width-bounding is the layout's job.
    pub fn draw(&mut self, out: &mut impl Write, frame: &[Line]) -> io::Result<()> {
        if self.force_clear {
            queue!(out, Clear(ClearType::All))?;
            self.force_clear = false;
        }
        let rows: Vec<String> = frame.iter().map(render_line).collect();
        for (i, row) in rows.iter().enumerate() {
            if self.last.get(i) == Some(row) {
                continue;
            }
            queue!(
                out,
                MoveTo(0, row_u16(i)),
                Print(row),
                Clear(ClearType::UntilNewLine)
            )?;
        }
        for i in rows.len()..self.last.len() {
            queue!(out, MoveTo(0, row_u16(i)), Clear(ClearType::UntilNewLine))?;
        }
        self.last = rows;
        out.flush()
    }
}

fn row_u16(row: usize) -> u16 {
    u16::try_from(row).unwrap_or(u16::MAX)
}

/// RAII guard that owns the alternate screen (+ raw mode) for the live view.
/// On entry it enables raw mode, switches to the alt screen, and hides the
/// cursor; on `Drop` it restores the normal terminal — on EVERY exit path
/// (normal return, early return, panic, Ctrl-C).
///
/// Raw mode is required for Esc-to-pause handling: without it, keystrokes are
/// line-buffered by the terminal driver and Esc would not reach
/// `crossterm::event::read` until Enter was pressed.
#[derive(Debug)]
pub struct AltScreen {
    _private: (),
}

impl AltScreen {
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(e) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            // Don't strand the terminal in raw mode if the switch failed.
            let _ = disable_raw_mode();
            return Err(e);
        }
        Ok(Self { _private: () })
    }
}

impl Drop for AltScreen {
    fn drop(&mut self) {
        // Best-effort: there is nothing useful to do if restoring fails.
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::live_view::row::{Line, Segment, Style};
    use crate::output::palette::{self, Status};

    /// Force colorless output through the explicit process-wide override
    /// (not env vars: owo-colors consults `FORCE_COLOR` before `NO_COLOR` and
    /// caches the answer, so env mutation is both too late and too weak).
    fn no_color() {
        palette::disable_color();
    }

    fn seg(style: Style, text: &str) -> Segment {
        Segment {
            text: text.to_string(),
            style,
        }
    }

    #[test]
    fn colorless_styles_render_raw_text_without_escapes() {
        no_color();
        for style in [
            Style::Plain,
            Style::Dim,
            Style::Strong,
            Style::Status(Status::Complete),
            Style::Status(Status::Failed),
            Style::Crew("cobalt-harbor".to_string()),
            Style::Crew("not-a-real-crew".to_string()),
            Style::Role("heron".to_string()),
            Style::Meter,
            Style::Danger,
            Style::Good,
        ] {
            let mut buf = String::new();
            style_ansi(&mut buf, &seg(style.clone(), "hello"));
            assert_eq!(buf, "hello", "style {style:?} must be raw when colorless");
            assert!(!buf.contains('\x1b'), "style {style:?} leaked an escape");
        }
    }

    #[test]
    fn style_ansi_appends_rather_than_overwriting() {
        no_color();
        let mut buf = String::from("pre:");
        style_ansi(&mut buf, &seg(Style::Danger, "x"));
        style_ansi(&mut buf, &seg(Style::Good, "y"));
        assert_eq!(buf, "pre:xy");
    }

    #[test]
    fn style_color_maps_each_style_to_its_role() {
        assert_eq!(style_color(&Style::Plain), None);
        assert_eq!(
            style_color(&Style::Status(Status::Failed)),
            Some(Status::Failed.color())
        );
        assert_eq!(style_color(&Style::Danger), Some(palette::FAILED));
        assert_eq!(style_color(&Style::Good), Some(palette::COMPLETE));
        assert_eq!(style_color(&Style::Dim), Some(palette::DIM));
        assert!(style_color(&Style::Strong).is_some());
        assert!(style_color(&Style::Meter).is_some());

        let (r, g, b) = rupu_codename::crew_tint("cobalt-harbor")
            .expect("cobalt is a crew color")
            .dark_rgb();
        assert_eq!(
            style_color(&Style::Crew("cobalt-harbor".to_string())),
            Some(owo_colors::Rgb(r, g, b))
        );
        let (r, g, b) = rupu_codename::role_badge("heron").tint.dark_rgb();
        assert_eq!(
            style_color(&Style::Role("heron".to_string())),
            Some(owo_colors::Rgb(r, g, b))
        );
    }

    #[test]
    fn unknown_crew_falls_back_to_plain() {
        assert_eq!(style_color(&Style::Crew("zzz-nothing".to_string())), None);
    }

    fn frame(rows: &[&str]) -> Vec<Line> {
        rows.iter().map(|r| Line::new().plain(*r)).collect()
    }

    fn draw_to_string(r: &mut LiveRenderer, rows: &[&str]) -> String {
        let mut out: Vec<u8> = Vec::new();
        r.draw(&mut out, &frame(rows)).expect("draw to a Vec");
        String::from_utf8(out).expect("utf8")
    }

    /// `ESC [ row+1 ; 1 H` — crossterm's `MoveTo(0, row)` encoding.
    fn move_to(row: u16) -> String {
        format!("\x1b[{};1H", row + 1)
    }

    const CLEAR_EOL: &str = "\x1b[K";
    const CLEAR_ALL: &str = "\x1b[2J";

    #[test]
    fn first_draw_paints_every_row() {
        no_color();
        let mut r = LiveRenderer::new();
        let out = draw_to_string(&mut r, &["alpha", "bravo", "charlie"]);
        for (i, text) in ["alpha", "bravo", "charlie"].iter().enumerate() {
            assert!(out.contains(&move_to(i as u16)), "missing MoveTo row {i}");
            assert!(out.contains(text), "missing row text {text}");
        }
        assert!(!out.contains(CLEAR_ALL), "no full-screen clear");
    }

    #[test]
    fn second_draw_emits_only_the_changed_row() {
        no_color();
        let mut r = LiveRenderer::new();
        let _ = draw_to_string(&mut r, &["alpha", "bravo", "charlie"]);
        let out = draw_to_string(&mut r, &["alpha", "BRAVO2", "charlie"]);
        assert!(out.contains(&move_to(1)), "MoveTo the changed row");
        assert!(out.contains("BRAVO2"));
        assert!(out.contains(CLEAR_EOL), "changed row is cleared to EOL");
        assert!(
            !out.contains("alpha"),
            "unchanged row must not be re-emitted"
        );
        assert!(
            !out.contains("charlie"),
            "unchanged row must not be re-emitted"
        );
        assert!(!out.contains(&move_to(0)));
        assert!(!out.contains(&move_to(2)));
        assert!(!out.contains(CLEAR_ALL), "no full-screen clear");
    }

    #[test]
    fn identical_frame_emits_nothing() {
        no_color();
        let mut r = LiveRenderer::new();
        let _ = draw_to_string(&mut r, &["alpha", "bravo"]);
        let out = draw_to_string(&mut r, &["alpha", "bravo"]);
        assert!(out.is_empty(), "no changes => no bytes, got {out:?}");
    }

    #[test]
    fn shorter_frame_clears_trailing_rows() {
        no_color();
        let mut r = LiveRenderer::new();
        let _ = draw_to_string(&mut r, &["alpha", "bravo", "charlie", "delta"]);
        let out = draw_to_string(&mut r, &["alpha", "bravo"]);
        assert!(out.contains(&move_to(2)), "stale row 2 cleared");
        assert!(out.contains(&move_to(3)), "stale row 3 cleared");
        assert_eq!(out.matches(CLEAR_EOL).count(), 2, "one clear per stale row");
        assert!(!out.contains("charlie") && !out.contains("delta"));
        assert!(!out.contains("alpha") && !out.contains("bravo"));
        assert!(!out.contains(CLEAR_ALL), "no full-screen clear");

        // A further identical draw is a no-op: the stale rows are forgotten.
        let again = draw_to_string(&mut r, &["alpha", "bravo"]);
        assert!(again.is_empty(), "got {again:?}");
    }

    #[test]
    fn longer_frame_paints_only_the_new_rows() {
        no_color();
        let mut r = LiveRenderer::new();
        let _ = draw_to_string(&mut r, &["alpha"]);
        let out = draw_to_string(&mut r, &["alpha", "bravo"]);
        assert!(out.contains(&move_to(1)) && out.contains("bravo"));
        assert!(!out.contains("alpha"));
    }

    #[test]
    fn invalidate_forces_one_full_repaint() {
        no_color();
        let mut r = LiveRenderer::new();
        let _ = draw_to_string(&mut r, &["alpha", "bravo"]);
        r.invalidate();
        let out = draw_to_string(&mut r, &["alpha", "bravo"]);
        assert!(out.contains(CLEAR_ALL), "resize repaint clears once");
        assert!(out.contains("alpha") && out.contains("bravo"));
        let after = draw_to_string(&mut r, &["alpha", "bravo"]);
        assert!(after.is_empty(), "back to diffing, got {after:?}");
    }
}
