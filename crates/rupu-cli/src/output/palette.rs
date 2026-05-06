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

/// Dim text — timestamps, run id (slate-500 #64748b).
pub const DIM: owo_colors::Rgb = owo_colors::Rgb(100, 116, 139);

/// Brand accent (brand-500 #7c3aed) — header `▶`.
pub const BRAND: owo_colors::Rgb = owo_colors::Rgb(124, 58, 237);

/// Tool arrow (slate-500).
pub const TOOL_ARROW: owo_colors::Rgb = owo_colors::Rgb(100, 116, 139);

// ── Severity colors ─────────────────────────────────────────────────────────

pub const SEV_CRITICAL: owo_colors::Rgb = owo_colors::Rgb(147, 51, 234);
pub const SEV_HIGH: owo_colors::Rgb = owo_colors::Rgb(220, 38, 38);
pub const SEV_MEDIUM: owo_colors::Rgb = owo_colors::Rgb(234, 88, 12);
/// Low = same as SOFT_FAILED.
pub const SEV_LOW: owo_colors::Rgb = SOFT_FAILED;
/// Info = same as DIM.
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

// ── Convenience wrapper: a piece of colored text ────────────────────────────

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
