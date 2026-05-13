//! Semantic CLI palette tokens plus the active runtime palette.
//!
//! Historical rendering code still references the default token colors
//! below (`RUNNING`, `BRAND`, …). `write_colored` / `write_bold_colored`
//! transparently remap those defaults through the currently active UI
//! palette, so commands can adopt themes incrementally without a giant
//! callsite rewrite.

use comfy_table::Color as TableColor;
use owo_colors::OwoColorize;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::{LazyLock, RwLock};

/// Default running / active token (blue-500 #3b82f6).
pub const RUNNING: owo_colors::Rgb = owo_colors::Rgb(59, 130, 246);
/// Default complete token (green-500 #22c55e).
pub const COMPLETE: owo_colors::Rgb = owo_colors::Rgb(34, 197, 94);
/// Default failed token (red-500 #ef4444).
pub const FAILED: owo_colors::Rgb = owo_colors::Rgb(239, 68, 68);
/// Default awaiting token (amber-400 #fbbf24).
pub const AWAITING: owo_colors::Rgb = owo_colors::Rgb(251, 191, 36);
/// Default skipped token (slate-300 #cbd5e1).
pub const SKIPPED: owo_colors::Rgb = owo_colors::Rgb(203, 213, 225);
/// Default soft-failed token (yellow-600 #ca8a04).
pub const SOFT_FAILED: owo_colors::Rgb = owo_colors::Rgb(202, 138, 4);
/// Default retrying token (violet-600 #7c3aed).
pub const RETRYING: owo_colors::Rgb = owo_colors::Rgb(124, 58, 237);
/// Default dim text token (slate-500 #64748b).
pub const DIM: owo_colors::Rgb = owo_colors::Rgb(100, 116, 139);
/// Visible default brand accent (violet-600 #7c3aed).
const BRAND_VISIBLE: owo_colors::Rgb = owo_colors::Rgb(124, 58, 237);
/// Semantic brand token (distinct from [`RETRYING`] so runtime theming
/// can remap each semantic bucket independently).
pub const BRAND: owo_colors::Rgb = owo_colors::Rgb(124, 58, 238);
/// Default subtle accent for guides / rails (violet-300 #a78bfa).
pub const BRAND_300: owo_colors::Rgb = owo_colors::Rgb(167, 139, 250);
/// Visible default tool arrow (same family as dim).
const TOOL_ARROW_VISIBLE: owo_colors::Rgb = owo_colors::Rgb(100, 116, 139);
/// Semantic tool-arrow token (distinct from [`DIM`] so runtime theming
/// can remap each semantic bucket independently).
pub const TOOL_ARROW: owo_colors::Rgb = owo_colors::Rgb(100, 116, 140);
/// Default separator line (slate-600 #475569).
pub const SEPARATOR: owo_colors::Rgb = owo_colors::Rgb(71, 85, 105);
/// Default critical severity.
pub const SEV_CRITICAL: owo_colors::Rgb = owo_colors::Rgb(147, 51, 234);
/// Default high severity.
pub const SEV_HIGH: owo_colors::Rgb = owo_colors::Rgb(220, 38, 38);
/// Default medium severity.
pub const SEV_MEDIUM: owo_colors::Rgb = owo_colors::Rgb(234, 88, 12);
/// Visible default low severity.
const SEV_LOW_VISIBLE: owo_colors::Rgb = owo_colors::Rgb(202, 138, 4);
/// Semantic low-severity token (distinct from [`SOFT_FAILED`]).
pub const SEV_LOW: owo_colors::Rgb = owo_colors::Rgb(202, 138, 5);
/// Visible default info severity.
const SEV_INFO_VISIBLE: owo_colors::Rgb = owo_colors::Rgb(100, 116, 139);
/// Semantic info-severity token (distinct from [`DIM`]).
pub const SEV_INFO: owo_colors::Rgb = owo_colors::Rgb(100, 116, 141);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn into_owo(self) -> owo_colors::Rgb {
        owo_colors::Rgb(self.r, self.g, self.b)
    }

    pub fn into_table(self) -> TableColor {
        TableColor::Rgb {
            r: self.r,
            g: self.g,
            b: self.b,
        }
    }
}

impl From<owo_colors::Rgb> for RgbColor {
    fn from(value: owo_colors::Rgb) -> Self {
        Self {
            r: value.0,
            g: value.1,
            b: value.2,
        }
    }
}

impl From<RgbColor> for owo_colors::Rgb {
    fn from(value: RgbColor) -> Self {
        value.into_owo()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiPaletteTheme {
    pub running: RgbColor,
    pub complete: RgbColor,
    pub failed: RgbColor,
    pub awaiting: RgbColor,
    pub skipped: RgbColor,
    pub soft_failed: RgbColor,
    pub retrying: RgbColor,
    pub dim: RgbColor,
    pub brand: RgbColor,
    pub brand_subtle: RgbColor,
    pub tool_arrow: RgbColor,
    pub separator: RgbColor,
    pub sev_critical: RgbColor,
    pub sev_high: RgbColor,
    pub sev_medium: RgbColor,
    pub sev_low: RgbColor,
    pub sev_info: RgbColor,
    pub label_palette: Vec<RgbColor>,
}

impl Default for UiPaletteTheme {
    fn default() -> Self {
        Self {
            running: RUNNING.into(),
            complete: COMPLETE.into(),
            failed: FAILED.into(),
            awaiting: AWAITING.into(),
            skipped: SKIPPED.into(),
            soft_failed: SOFT_FAILED.into(),
            retrying: RETRYING.into(),
            dim: DIM.into(),
            brand: BRAND_VISIBLE.into(),
            brand_subtle: BRAND_300.into(),
            tool_arrow: TOOL_ARROW_VISIBLE.into(),
            separator: SEPARATOR.into(),
            sev_critical: SEV_CRITICAL.into(),
            sev_high: SEV_HIGH.into(),
            sev_medium: SEV_MEDIUM.into(),
            sev_low: SEV_LOW_VISIBLE.into(),
            sev_info: SEV_INFO_VISIBLE.into(),
            label_palette: vec![
                RgbColor::new(0xd1, 0x4b, 0x4b),
                RgbColor::new(0xd1, 0x6a, 0x4b),
                RgbColor::new(0xd1, 0xa1, 0x4b),
                RgbColor::new(0xb8, 0xc1, 0x4b),
                RgbColor::new(0x6a, 0xc1, 0x4b),
                RgbColor::new(0x4b, 0xc1, 0x9e),
                RgbColor::new(0x4b, 0xa1, 0xc1),
                RgbColor::new(0x4b, 0x6a, 0xc1),
                RgbColor::new(0x6a, 0x4b, 0xc1),
                RgbColor::new(0xa1, 0x4b, 0xc1),
                RgbColor::new(0xc1, 0x4b, 0xa1),
                RgbColor::new(0x96, 0x70, 0x60),
            ],
        }
    }
}

static ACTIVE_PALETTE: LazyLock<RwLock<UiPaletteTheme>> =
    LazyLock::new(|| RwLock::new(UiPaletteTheme::default()));

pub fn set_active_palette(theme: UiPaletteTheme) {
    if let Ok(mut slot) = ACTIVE_PALETTE.write() {
        *slot = theme;
    }
}

pub fn active_palette() -> UiPaletteTheme {
    ACTIVE_PALETTE
        .read()
        .map(|value| value.clone())
        .unwrap_or_default()
}

pub fn themed(color: owo_colors::Rgb) -> owo_colors::Rgb {
    let palette = active_palette();
    match color {
        RUNNING => palette.running.into(),
        COMPLETE => palette.complete.into(),
        FAILED => palette.failed.into(),
        AWAITING => palette.awaiting.into(),
        SKIPPED => palette.skipped.into(),
        SOFT_FAILED => palette.soft_failed.into(),
        RETRYING => palette.retrying.into(),
        DIM => palette.dim.into(),
        BRAND => palette.brand.into(),
        BRAND_300 => palette.brand_subtle.into(),
        TOOL_ARROW => palette.tool_arrow.into(),
        SEPARATOR => palette.separator.into(),
        SEV_CRITICAL => palette.sev_critical.into(),
        SEV_HIGH => palette.sev_high.into(),
        SEV_MEDIUM => palette.sev_medium.into(),
        SEV_LOW => palette.sev_low.into(),
        SEV_INFO => palette.sev_info.into(),
        other => other,
    }
}

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
        let palette = active_palette();
        match self {
            Status::Waiting => palette.skipped.into(),
            Status::Active | Status::Working => palette.running.into(),
            Status::Complete => palette.complete.into(),
            Status::Failed => palette.failed.into(),
            Status::SoftFailed => palette.soft_failed.into(),
            Status::Awaiting => palette.awaiting.into(),
            Status::Retrying => palette.retrying.into(),
            Status::Skipped => palette.skipped.into(),
        }
    }
}

/// Write `text` using a semantic color token that remaps through the
/// active palette.
pub fn write_colored(f: &mut dyn fmt::Write, text: &str, color: owo_colors::Rgb) -> fmt::Result {
    let color = themed(color);
    let colored = text
        .if_supports_color(owo_colors::Stream::Stdout, |s| s.color(color))
        .to_string();
    f.write_str(&colored)
}

/// Write `text` bold + colored. Falls back to plain when colors are off.
pub fn write_bold_colored(
    f: &mut dyn fmt::Write,
    text: &str,
    color: owo_colors::Rgb,
) -> fmt::Result {
    let color = themed(color);
    let probe = text
        .if_supports_color(owo_colors::Stream::Stdout, |s| s.color(color))
        .to_string();
    let supports = probe != text;

    if supports {
        let owo_colors::Rgb(r, g, b) = color;
        write!(f, "\x1b[1;38;2;{r};{g};{b}m{text}\x1b[0m")
    } else {
        f.write_str(text)
    }
}
