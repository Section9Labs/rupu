//! Themed download/install progress for `rupu update`.
//!
//! A single "minimal brand" element: a BRAND-purple determinate byte bar
//! during the download, which morphs into a steady-tick "installing…"
//! spinner while the verify + code-sign + swap stages run, then clears to a
//! two-line footer. Colors come from the active runtime palette
//! (`output::palette`), so the bar matches whatever theme the user has
//! configured — the same tokens the live workflow view uses.
//!
//! Everything degrades gracefully: no TTY → no bar (the caller prints a
//! plain success line instead); TTY but colors off → the same bar without
//! ANSI. The pure `*_template` / `*_lines` helpers below carry the format
//! and are unit-tested; the `ProgressBar` lifecycle is validated at runtime.

use std::io::IsTerminal;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use rupu_update::Channel;

use crate::cmd::ui::UiPrefs;
use crate::output::palette;

/// Steady-tick cadence for the download bar / installing spinner.
const TICK: Duration = Duration::from_millis(90);
/// Rendered bar width (cells).
const BAR_WIDTH: usize = 28;

/// Owns the update's progress bar (when interactive) and the themed flag.
pub struct UpdateProgress {
    bar: Option<ProgressBar>,
    themed: bool,
    from: String,
}

impl UpdateProgress {
    /// Start the download bar. Returns an inert handle (no bar) when stdout
    /// is not a TTY, so piped / CI runs fall back to the caller's plain
    /// output. `themed` follows the resolved color preference.
    pub fn start(from: &str, to: &str, channel: Channel, prefs: &UiPrefs) -> Self {
        if !std::io::stdout().is_terminal() {
            return Self {
                bar: None,
                themed: false,
                from: from.to_string(),
            };
        }
        let themed = prefs.use_color();
        let bar = ProgressBar::new(0);
        bar.set_draw_target(ProgressDrawTarget::stdout());
        bar.set_style(download_style(themed));
        bar.set_prefix(header_prefix(from, to, channel, themed));
        bar.enable_steady_tick(TICK);
        Self {
            bar: Some(bar),
            themed,
            from: from.to_string(),
        }
    }

    /// A clone of the underlying bar for the download closure / apply
    /// strategy to tick. `None` when non-interactive.
    pub fn bar(&self) -> Option<ProgressBar> {
        self.bar.clone()
    }

    pub fn themed(&self) -> bool {
        self.themed
    }

    /// Clear the bar and print the two-line themed footer. When
    /// non-interactive, print the plain legacy success line instead so
    /// scripts still see a stable "Updated rupu …" message.
    pub fn finish(&self, new: &str, channel: Channel) {
        match &self.bar {
            Some(bar) => {
                bar.finish_and_clear();
                let (l1, l2) = footer_lines(new, channel, self.themed);
                println!("{l1}");
                println!("{l2}");
            }
            None => {
                println!("Updated rupu {} → {new} ({channel}).", self.from);
            }
        }
    }
}

/// Morph the download bar into the "installing…" spinner. Called once the
/// binary bytes have all arrived; the verify/sign/swap work that follows is
/// not byte-measurable, so it reads as an indeterminate spinner.
pub fn switch_to_installing(bar: &ProgressBar, themed: bool) {
    if let Ok(style) = ProgressStyle::with_template(&installing_template(themed)) {
        bar.set_style(style.tick_strings(SPINNER_FRAMES));
    }
    bar.set_message("installing…");
}

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "✓"];

// ── Pure format helpers (unit-tested) ──────────────────────────────────────

/// Wrap `text` in a truecolor SGR sequence when `themed`, else return it
/// unchanged. `bold` adds the bold attribute (matches `palette`'s own
/// `write_bold_colored`).
fn paint(text: &str, token: owo_colors::Rgb, bold: bool, themed: bool) -> String {
    if !themed {
        return text.to_string();
    }
    let owo_colors::Rgb(r, g, b) = palette::themed(token);
    let attr = if bold { "1;" } else { "" };
    format!("\x1b[{attr}38;2;{r};{g};{b}m{text}\x1b[0m")
}

/// `rupu  <from> ⟶ <to>  (<channel>)` — the bar's header line.
pub fn header_prefix(from: &str, to: &str, channel: Channel, themed: bool) -> String {
    format!(
        "{}  {} {}  {}",
        paint("rupu", palette::BRAND, true, themed),
        paint(&format!("{from} ⟶"), palette::DIM, false, themed),
        paint(to, palette::COMPLETE, false, themed),
        paint(&format!("({channel})"), palette::DIM, false, themed),
    )
}

/// The two-line completion footer: `rupu  now <new>  (<channel>)` then
/// `✓ verified   ✓ installed`.
pub fn footer_lines(new: &str, channel: Channel, themed: bool) -> (String, String) {
    let line1 = format!(
        "  {}  {} {}  {}",
        paint("rupu", palette::BRAND, true, themed),
        paint("now", palette::DIM, false, themed),
        paint(new, palette::COMPLETE, false, themed),
        paint(&format!("({channel})"), palette::DIM, false, themed),
    );
    let check = paint("✓", palette::COMPLETE, false, themed);
    let line2 = format!("  {check} verified   {check} installed");
    (line1, line2)
}

/// indicatif template for the determinate download bar. The bar glyphs are
/// wrapped in a BRAND SGR sequence (indicatif measures width with ANSI
/// stripped, so this is safe — the same trick `output::printer` uses).
fn download_style(themed: bool) -> ProgressStyle {
    let bar = format!("{{bar:{BAR_WIDTH}}}");
    let template = format!(
        "  {{prefix}}\n  {}  {}\n  {}",
        paint(&bar, palette::BRAND, false, themed),
        paint("{percent}%", palette::DIM, false, themed),
        paint(
            "{bytes} / {total_bytes} · {binary_bytes_per_sec}",
            palette::DIM,
            false,
            themed,
        ),
    );
    ProgressStyle::with_template(&template)
        .expect("static download template is valid")
        .progress_chars("██░")
}

/// indicatif template for the installing spinner.
fn installing_template(themed: bool) -> String {
    format!(
        "  {}  {}",
        paint("{spinner}", palette::BRAND, false, themed),
        paint("installing…", palette::DIM, false, themed),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_helpers_carry_no_ansi() {
        let h = header_prefix("0.49.0", "0.50.0", Channel::Stable, false);
        assert_eq!(h, "rupu  0.49.0 ⟶ 0.50.0  (stable)");
        assert!(!h.contains('\x1b'));

        let (l1, l2) = footer_lines("0.50.0", Channel::Stable, false);
        assert_eq!(l1, "  rupu  now 0.50.0  (stable)");
        assert_eq!(l2, "  ✓ verified   ✓ installed");
        assert!(!l2.contains('\x1b'));
    }

    #[test]
    fn themed_helpers_emit_truecolor_and_reset() {
        let h = header_prefix("0.49.0", "0.50.0", Channel::Beta, true);
        // Still contains the human-readable content…
        assert!(h.contains("0.49.0 ⟶"));
        assert!(h.contains("0.50.0"));
        assert!(h.contains("(beta)"));
        // …wrapped in truecolor SGR that always resets.
        assert!(h.contains("\x1b[38;2;"));
        assert!(h.contains("\x1b[1;38;2;")); // "rupu" is bold-branded
        assert!(h.contains("\x1b[0m"));

        let (l1, l2) = footer_lines("0.50.0", Channel::Beta, true);
        assert!(l1.contains("\x1b[0m"));
        assert!(l2.contains("✓"));
        assert!(l2.contains("\x1b[38;2;"));
    }

    #[test]
    fn download_and_installing_templates_are_valid() {
        // `download_style` would panic on a bad template; call both modes.
        let _ = download_style(true);
        let _ = download_style(false);
        assert!(ProgressStyle::with_template(&installing_template(true)).is_ok());
        assert!(ProgressStyle::with_template(&installing_template(false)).is_ok());
    }

    #[test]
    fn paint_is_identity_when_unthemed() {
        assert_eq!(paint("x", palette::BRAND, true, false), "x");
        assert_eq!(paint("x", palette::DIM, false, false), "x");
    }
}
