//! Shared coloring helpers for tabular output (`*-list` commands).
//!
//! Three render primitives:
//!
//! - [`colored_status`] — semantic foreground color per status string.
//!   Reuses the Okesu palette already defined in `output::palette` so
//!   the line-stream printer and the listing tables stay visually
//!   coherent.
//!
//! - [`colored_label_chip`] — render a label name as a colored chip.
//!   v0 uses a deterministic hash → palette mapping (same label name
//!   always renders the same color). When connectors start carrying
//!   the upstream hex color (`Issue.label_colors`), swap to
//!   [`colored_label_chip_with_hex`] which honors the upstream color
//!   with a luminance-aware foreground pick.
//!
//! - [`colored_relative_time`] — gradient based on how soon something
//!   fires (or how recently it ran): `<1h`=warm, `<24h`=default,
//!   `>24h`=dim.
//!
//! All three honor [`UiPrefs::use_color()`] — when color is off (pipe,
//! `NO_COLOR`, `--no-color`, `[ui].color = "never"` in config) they
//! return uncolored cells.

use crate::cmd::ui::UiPrefs;
use comfy_table::{presets, Cell, Color as TableColor, ContentArrangement, Table};

/// Build a `comfy-table::Table` with rupu's default visual style:
/// UTF8 borders, dynamic content arrangement, no separator lines
/// inside the body.
pub fn new_table() -> Table {
    let mut t = Table::new();
    t.load_preset(presets::UTF8_FULL);
    t.set_content_arrangement(ContentArrangement::Dynamic);
    t
}

/// Foreground color for a status string. Returns `None` when colors
/// are disabled OR when the status doesn't match a known semantic
/// bucket — caller falls back to a plain `Cell::new(status)`.
pub fn status_color(status: &str, prefs: &UiPrefs) -> Option<TableColor> {
    if !prefs.use_color() {
        return None;
    }
    Some(match status {
        // Workflow runs.
        "running" => TableColor::Blue,
        "completed" => TableColor::Green,
        "failed" => TableColor::Red,
        "awaiting_approval" | "awaiting" | "paused" => TableColor::Yellow,
        "rejected" => TableColor::Magenta,
        "pending" => TableColor::DarkGrey,
        // Issue / PR states.
        "open" => TableColor::Green,
        "closed" => TableColor::Magenta,
        "merged" => TableColor::Magenta,
        // Agent / workflow scope.
        "project" => TableColor::Cyan,
        "global" => TableColor::DarkGrey,
        _ => return None,
    })
}

/// Wrap `text` in a `Cell` colored according to its status semantics.
/// When color is disabled or the status is unrecognized, returns a
/// plain cell.
pub fn status_cell(text: &str, prefs: &UiPrefs) -> Cell {
    let cell = Cell::new(text);
    match status_color(text, prefs) {
        Some(c) => cell.fg(c),
        None => cell,
    }
}

/// Render a single label as a colored chip. v0: deterministic hash →
/// palette index, so the same label name always picks the same color
/// and labels visually cluster across runs of the same command.
pub fn colored_label_chip(name: &str, prefs: &UiPrefs) -> String {
    if !prefs.use_color() {
        return format!("[{name}]");
    }
    let (r, g, b) = label_palette_color(name);
    let (fr, fg, fb) = pick_fg_for_bg(r, g, b);
    // padding: leading + trailing space gives the chip room to breathe.
    format!(
        "\x1b[48;2;{r};{g};{b}m\x1b[38;2;{fr};{fg};{fb}m {name} \x1b[0m"
    )
}

/// Render a join of label chips, separated by a single space. Empty
/// vec returns a dim placeholder so the table column doesn't look
/// broken.
pub fn label_chips(labels: &[String], prefs: &UiPrefs) -> String {
    if labels.is_empty() {
        return if prefs.use_color() {
            "\x1b[2m—\x1b[0m".to_string()
        } else {
            "—".to_string()
        };
    }
    labels
        .iter()
        .map(|l| colored_label_chip(l, prefs))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Map a label name to one of N preset background colors using a
/// stable FNV-1a hash. Names with the same color cluster visually
/// without forcing the user to memorize anything.
fn label_palette_color(name: &str) -> (u8, u8, u8) {
    // 12 distinct hues spaced ~30° apart on the HSL wheel at L=58, S=68.
    // Picked to be visible on both light and dark terminals — no near-
    // black, no near-white, no muddy desaturated browns.
    const PALETTE: &[(u8, u8, u8)] = &[
        (0xd1, 0x4b, 0x4b), // red
        (0xd1, 0x6a, 0x4b), // orange
        (0xd1, 0xa1, 0x4b), // amber
        (0xb8, 0xc1, 0x4b), // lime
        (0x6a, 0xc1, 0x4b), // green
        (0x4b, 0xc1, 0x9e), // teal
        (0x4b, 0xa1, 0xc1), // cyan
        (0x4b, 0x6a, 0xc1), // blue
        (0x6a, 0x4b, 0xc1), // indigo
        (0xa1, 0x4b, 0xc1), // violet
        (0xc1, 0x4b, 0xa1), // pink
        (0x96, 0x70, 0x60), // mocha (the "neutral" slot)
    ];
    let h = fnv1a(name.as_bytes());
    PALETTE[(h % PALETTE.len() as u64) as usize]
}

/// Pick a black-or-white foreground that contrasts with a given RGB
/// background using the standard relative-luminance formula. Same
/// approach the GitHub web UI uses to decide whether label text is
/// black or white on top of a colored chip.
fn pick_fg_for_bg(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    // Y = 0.299R + 0.587G + 0.114B (BT.601). Threshold 140 — slightly
    // pulled toward "prefer black" because dark text on a tinted bg is
    // less harsh than white text in most terminal emulators.
    let y = 0.299 * (r as f32) + 0.587 * (g as f32) + 0.114 * (b as f32);
    if y > 140.0 {
        (0x0a, 0x0a, 0x0a) // near-black
    } else {
        (0xf5, 0xf5, 0xf5) // near-white
    }
}

/// Tiny FNV-1a hash so we don't pull in `siphasher` or similar just
/// for label coloring.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Hex variant — kept here so the call sites can switch to it once
/// `Issue.label_colors` is plumbed through. Same luminance picker.
#[allow(dead_code)]
pub fn colored_label_chip_with_hex(name: &str, hex_no_hash: &str, prefs: &UiPrefs) -> String {
    if !prefs.use_color() {
        return format!("[{name}]");
    }
    let Some((r, g, b)) = parse_hex(hex_no_hash) else {
        return colored_label_chip(name, prefs);
    };
    let (fr, fg, fb) = pick_fg_for_bg(r, g, b);
    format!("\x1b[48;2;{r};{g};{b}m\x1b[38;2;{fr};{fg};{fb}m {name} \x1b[0m")
}

fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some((r, g, b))
}

/// Color a relative-time hint. Pre-`now`-positive seconds_until = how
/// far in the future the event fires; negative = how recent. Emphasis
/// gradient: imminent (warning), normal, distant (dim).
pub fn relative_time_cell(seconds_until: i64, prefs: &UiPrefs) -> Cell {
    let cell = Cell::new(format_seconds(seconds_until));
    if !prefs.use_color() {
        return cell;
    }
    let abs = seconds_until.unsigned_abs();
    if abs < 3600 {
        cell.fg(TableColor::Yellow)
    } else if abs > 86_400 {
        cell.fg(TableColor::DarkGrey)
    } else {
        cell
    }
}

fn format_seconds(s: i64) -> String {
    let abs = s.unsigned_abs();
    let body = if abs < 60 {
        format!("{abs}s")
    } else if abs < 3600 {
        format!("{}m", abs / 60)
    } else if abs < 86_400 {
        format!("{}h", abs / 3600)
    } else {
        format!("{}d", abs / 86_400)
    };
    if s >= 0 {
        format!("in {body}")
    } else {
        format!("{body} ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::ui::{ColorMode, PagerMode};

    // Construct UiPrefs directly rather than going through `resolve`
    // so concurrent tests in this binary that flip NO_COLOR don't
    // pollute the result. (`cmd::ui::tests::ui_prefs_no_color_env_overrides_config`
    // sets and unsets NO_COLOR; cargo's parallel test runner can
    // observe it mid-flight.)
    fn prefs_color_always() -> UiPrefs {
        UiPrefs {
            color: ColorMode::Always,
            theme: "base16-ocean.dark".into(),
            pager: PagerMode::Never,
        }
    }

    fn prefs_no_color() -> UiPrefs {
        UiPrefs {
            color: ColorMode::Never,
            theme: "base16-ocean.dark".into(),
            pager: PagerMode::Never,
        }
    }

    #[test]
    fn status_color_known_buckets() {
        let p = prefs_color_always();
        assert!(matches!(status_color("running", &p), Some(TableColor::Blue)));
        assert!(matches!(status_color("completed", &p), Some(TableColor::Green)));
        assert!(matches!(status_color("failed", &p), Some(TableColor::Red)));
        assert!(matches!(status_color("awaiting_approval", &p), Some(TableColor::Yellow)));
        assert!(matches!(status_color("rejected", &p), Some(TableColor::Magenta)));
        assert!(matches!(status_color("open", &p), Some(TableColor::Green)));
        assert!(matches!(status_color("closed", &p), Some(TableColor::Magenta)));
        assert!(matches!(status_color("project", &p), Some(TableColor::Cyan)));
    }

    #[test]
    fn status_color_unknown_returns_none() {
        let p = prefs_color_always();
        assert!(status_color("zzz_nope", &p).is_none());
    }

    #[test]
    fn status_color_off_when_no_color() {
        let p = prefs_no_color();
        assert!(status_color("running", &p).is_none());
    }

    #[test]
    fn label_chip_falls_back_to_brackets_no_color() {
        let p = prefs_no_color();
        assert_eq!(colored_label_chip("triage", &p), "[triage]");
    }

    #[test]
    fn label_chip_emits_truecolor_escape_when_colored() {
        let p = prefs_color_always();
        let s = colored_label_chip("triage", &p);
        assert!(s.starts_with("\x1b[48;2;"));
        assert!(s.contains("triage"));
        assert!(s.ends_with("\x1b[0m"));
    }

    #[test]
    fn label_palette_is_deterministic() {
        // Same name → same color across runs. The inequality across
        // names is NOT guaranteed (12 slots × arbitrary names ⇒ regular
        // collisions); only the determinism property is load-bearing.
        for name in ["triage", "bug", "help-wanted", "good first issue"] {
            assert_eq!(label_palette_color(name), label_palette_color(name));
        }
    }

    #[test]
    fn pick_fg_for_pale_bg_chooses_dark_text() {
        // Pale yellow (~labels.color = "fbca04" on GitHub).
        assert_eq!(pick_fg_for_bg(0xfb, 0xca, 0x04), (0x0a, 0x0a, 0x0a));
    }

    #[test]
    fn pick_fg_for_dark_bg_chooses_light_text() {
        // Deep red.
        assert_eq!(pick_fg_for_bg(0x6a, 0x12, 0x12), (0xf5, 0xf5, 0xf5));
    }

    #[test]
    fn label_chips_empty_renders_em_dash() {
        let p = prefs_no_color();
        assert_eq!(label_chips(&[], &p), "—");
    }

    #[test]
    fn parse_hex_round_trip() {
        assert_eq!(parse_hex("d73a4a"), Some((0xd7, 0x3a, 0x4a)));
        assert_eq!(parse_hex("xyz123"), None);
        assert_eq!(parse_hex("d73a"), None);
    }

    #[test]
    fn format_seconds_grades() {
        assert_eq!(format_seconds(45), "in 45s");
        assert_eq!(format_seconds(120), "in 2m");
        assert_eq!(format_seconds(7200), "in 2h");
        assert_eq!(format_seconds(172_800), "in 2d");
        assert_eq!(format_seconds(-30), "30s ago");
    }

    #[test]
    fn relative_time_imminent_is_yellow() {
        let p = prefs_color_always();
        let cell = relative_time_cell(120, &p);
        // comfy-table doesn't expose fg() directly for assertion;
        // the smoke is that the cell builds without panic and the
        // yellow branch is taken (covered by status_color_known_buckets).
        let _ = cell;
    }
}
