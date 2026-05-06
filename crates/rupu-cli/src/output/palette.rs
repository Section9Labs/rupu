//! Okesu color palette constants for the line-stream printer.
//!
//! These are the canonical token colors used by the Okesu SaaS dashboard.
//! The printer applies them via `owo_colors::OwoColorize::if_supports_color`
//! so they auto-degrade when stdout is not a TTY or `NO_COLOR=1` is set.

use owo_colors::OwoColorize;
use std::fmt;

// ── Okesu palette RGB values ────────────────────────────────────────────────

/// Running / active (blue-500 #3b82f6).
pub const RUNNING: owo_colors::Rgb = owo_colors::Rgb(59, 130, 246);

/// Complete (green-500 #22c55e).
pub const COMPLETE: owo_colors::Rgb = owo_colors::Rgb(34, 197, 94);

/// Failed (red-500 #ef4444).
pub const FAILED: owo_colors::Rgb = owo_colors::Rgb(239, 68, 68);

/// Awaiting approval (amber-400 #fbbf24).
pub const AWAITING: owo_colors::Rgb = owo_colors::Rgb(251, 191, 36);

/// Skipped (slate-300 #cbd5e1).
pub const SKIPPED: owo_colors::Rgb = owo_colors::Rgb(203, 213, 225);

/// Soft-failed (yellow-600 #ca8a04).
pub const SOFT_FAILED: owo_colors::Rgb = owo_colors::Rgb(202, 138, 4);

/// Retrying (brand-500 #7c3aed).
pub const RETRYING: owo_colors::Rgb = owo_colors::Rgb(124, 58, 237);

/// Dim text — timestamps, run id, metadata (slate-500 #64748b).
pub const DIM: owo_colors::Rgb = owo_colors::Rgb(100, 116, 139);

/// Brand accent (brand-500 #7c3aed) — header `▶`, workflow name.
pub const BRAND: owo_colors::Rgb = owo_colors::Rgb(124, 58, 237);

/// Subtle brand tint for indent guides (brand-300 #a78bfa).
/// Gives the vertical thread a soft purple warmth without competing with
/// the status glyphs.
pub const BRAND_300: owo_colors::Rgb = owo_colors::Rgb(167, 139, 250);

/// Tool arrow (slate-500).
pub const TOOL_ARROW: owo_colors::Rgb = owo_colors::Rgb(100, 116, 139);

/// Phase separator line (slate-600 #475569) — slightly dimmer than DIM.
pub const SEPARATOR: owo_colors::Rgb = owo_colors::Rgb(71, 85, 105);

// ── Severity colors ─────────────────────────────────────────────────────────

/// Critical severity (#9333ea — brand-600 purple, bold).
pub const SEV_CRITICAL: owo_colors::Rgb = owo_colors::Rgb(147, 51, 234);
/// High severity (#dc2626 — red-600, bold).
pub const SEV_HIGH: owo_colors::Rgb = owo_colors::Rgb(220, 38, 38);
/// Medium severity (#ea580c — orange-600).
pub const SEV_MEDIUM: owo_colors::Rgb = owo_colors::Rgb(234, 88, 12);
/// Low severity — same as SOFT_FAILED (#ca8a04).
pub const SEV_LOW: owo_colors::Rgb = SOFT_FAILED;
/// Info severity — same as DIM (#64748b).
pub const SEV_INFO: owo_colors::Rgb = DIM;

// ── Status glyphs ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Waiting,
    Active,
    Working,
    Complete,
    Failed,
    SoftFailed,
    Awaiting,
    Retrying,
    Skipped,
}

impl Status {
    pub fn glyph(self) -> char {
        match self {
            Status::Waiting => '○',
            Status::Active => '●',
            Status::Working => '◐',
            Status::Complete => '✓',
            Status::Failed => '✗',
            Status::SoftFailed => '!',
            Status::Awaiting => '⏸',
            Status::Retrying => '↺',
            Status::Skipped => '⊘',
        }
    }

    pub fn color(self) -> owo_colors::Rgb {
        match self {
            Status::Waiting => SKIPPED,
            Status::Active | Status::Working => RUNNING,
            Status::Complete => COMPLETE,
            Status::Failed => FAILED,
            Status::SoftFailed => SOFT_FAILED,
            Status::Awaiting => AWAITING,
            Status::Retrying => RETRYING,
            Status::Skipped => SKIPPED,
        }
    }
}

// ── Convenience wrappers ────────────────────────────────────────────────────

/// Write `text` to `f` using the given Okesu RGB color, or plain if
/// `supports-colors` says the stream does not support colors.
pub fn write_colored(
    f: &mut dyn fmt::Write,
    text: &str,
    color: owo_colors::Rgb,
) -> fmt::Result {
    // owo_colors' if_supports_color checks both NO_COLOR and the stream.
    // We use Stdout as the stream sentinel — the printer always writes to
    // stdout so this is accurate.
    let colored = text
        .if_supports_color(owo_colors::Stream::Stdout, |s| s.color(color))
        .to_string();
    f.write_str(&colored)
}

/// Write `text` bold + colored. Falls back to plain when colors are off.
///
/// We build the ANSI string manually to avoid lifetime issues with chained
/// owo-colors combinators (`.color(c).bold()` borrows the temporary).
pub fn write_bold_colored(
    f: &mut dyn fmt::Write,
    text: &str,
    color: owo_colors::Rgb,
) -> fmt::Result {
    // Check NO_COLOR / stream support the same way write_colored does.
    let probe = text
        .if_supports_color(owo_colors::Stream::Stdout, |s| s.color(color))
        .to_string();
    let supports = probe != text;

    if supports {
        let owo_colors::Rgb(r, g, b) = color;
        // CSI 1 = bold, CSI 38;2;r;g;b = RGB foreground, CSI 0 = reset.
        write!(f, "\x1b[1;38;2;{r};{g};{b}m{text}\x1b[0m")
    } else {
        f.write_str(text)
    }
}
