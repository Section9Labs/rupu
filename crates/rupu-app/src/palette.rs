//! Color tokens for rupu.app. Mirrors the Okesu palette already in
//! rupu-cli/src/output/palette.rs so terminal output and the app
//! render the same colors. GPUI uses `Rgba` / `Hsla` types from its
//! color module; we expose `gpui::Rgba` constants here.

use gpui::Rgba;

/// Construct an opaque RGB color from 8-bit components. GPUI's `Rgba`
/// uses normalized floats internally, so we divide by 255.0.
const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

// ── Status colors ─────────────────────────────────────────────────────────
pub const RUNNING: Rgba = rgb(59, 130, 246); // blue-500
pub const COMPLETE: Rgba = rgb(34, 197, 94); // green-500
pub const FAILED: Rgba = rgb(239, 68, 68); // red-500
pub const AWAITING: Rgba = rgb(251, 191, 36); // amber-400
pub const SKIPPED: Rgba = rgb(203, 213, 225); // slate-300

// ── Chrome ────────────────────────────────────────────────────────────────
pub const DIM: Rgba = rgb(100, 116, 139); // slate-500
pub const BRAND: Rgba = rgb(124, 58, 237); // brand-500 (purple)
pub const BRAND_300: Rgba = rgb(167, 139, 250); // brand-300 (lighter purple)

// ── Window chrome ─────────────────────────────────────────────────────────
pub const BG_PRIMARY: Rgba = rgb(15, 15, 18); // window background (#0f0f12)
pub const BG_SIDEBAR: Rgba = rgb(24, 24, 27); // sidebar bg (#18181b)
pub const BG_TITLEBAR: Rgba = rgb(9, 9, 11); // titlebar bg (#09090b)
pub const BORDER: Rgba = rgb(31, 31, 35); // separator lines (#1f1f23)
pub const TEXT_PRIMARY: Rgba = rgb(250, 250, 250); // foreground text (#fafafa)
pub const TEXT_MUTED: Rgba = rgb(161, 161, 170); // secondary text (#a1a1aa)
pub const TEXT_DIMMEST: Rgba = rgb(82, 82, 91); // tertiary / section labels (#52525b)

// ── Workspace color chips (5 user-selectable accents) ─────────────────────
// Used for the color chip in titlebar + workspace switcher.
pub const CHIP_PURPLE: Rgba = BRAND;
pub const CHIP_BLUE: Rgba = rgb(59, 130, 246);
pub const CHIP_GREEN: Rgba = rgb(34, 197, 94);
pub const CHIP_AMBER: Rgba = rgb(251, 191, 36);
pub const CHIP_PINK: Rgba = rgb(236, 72, 153);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_purple_matches_cli_palette() {
        // rupu-cli/src/output/palette.rs::BRAND = Rgb(124, 58, 237).
        // Cross-surface coherence requires these stay in sync.
        assert_eq!(BRAND.r, 124.0 / 255.0);
        assert_eq!(BRAND.g, 58.0 / 255.0);
        assert_eq!(BRAND.b, 237.0 / 255.0);
        assert_eq!(BRAND.a, 1.0);
    }

    #[test]
    fn all_colors_are_opaque() {
        for color in [RUNNING, COMPLETE, FAILED, AWAITING, SKIPPED, DIM, BRAND] {
            assert_eq!(color.a, 1.0, "all palette colors should be fully opaque");
        }
    }
}
