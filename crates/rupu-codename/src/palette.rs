/// Crew colors: `(name, light-theme hex, dark-theme hex)`. Both hexes clear
/// 3:1 non-text contrast against `#ffffff` / `#0f1115` (`tests::palette_contrast`).
/// Pure status red/green are deliberately absent.
pub const COLORS: &[(&str, &str, &str)] = &[
    ("amber", "#b45309", "#fbbf24"),
    ("apricot", "#c2410c", "#fdba74"),
    ("azure", "#0369a1", "#7dd3fc"),
    ("bronze", "#92400e", "#d6a064"),
    ("cedar", "#7c4a2d", "#c8906a"),
    ("cerise", "#be185d", "#f472b6"),
    ("cobalt", "#1d4ed8", "#93b4fd"),
    ("copper", "#9a3412", "#f0a070"),
    ("coral", "#c2412d", "#fb8f78"),
    ("cyan", "#0e7490", "#67e8f9"),
    ("denim", "#1e40af", "#8fa8e8"),
    ("ebony", "#3f3f46", "#a1a1aa"),
    ("fawn", "#8a6a4a", "#d8b894"),
    ("fuchsia", "#a21caf", "#f0abfc"),
    ("ginger", "#b45f06", "#f5a55a"),
    ("gold", "#a16207", "#fde047"),
    ("graphite", "#52525b", "#d4d4d8"),
    ("hazel", "#7a5c2e", "#c9a66b"),
    ("indigo", "#4338ca", "#a5b4fc"),
    ("iris", "#5b4bc4", "#b4a9f5"),
    ("jade", "#0f766e", "#5eead4"),
    ("khaki", "#6b6a2a", "#d0cc8a"),
    ("lapis", "#1e3a8a", "#8ea6f0"),
    ("lavender", "#7c5cc4", "#c4b5fd"),
    ("lilac", "#9061a8", "#dcb8f0"),
    ("magenta", "#a3137a", "#f58ad8"),
    ("mauve", "#8e5a7a", "#d9a5c6"),
    ("mint", "#0f7a5a", "#86efcf"),
    ("ochre", "#9a6b0a", "#e6b34a"),
    ("olive", "#5c6b1f", "#bccb6a"),
    ("onyx", "#27272a", "#b8b8c0"),
    ("orchid", "#9d3fa8", "#e9a0f0"),
    ("pearl", "#6e7380", "#e6e8ee"),
    ("peach", "#c2562a", "#fdba9a"),
    ("pewter", "#5f6b73", "#bcc6cc"),
    ("plum", "#7e2a6e", "#e0a0d6"),
    ("rust", "#9a3a12", "#e88a5a"),
    ("saffron", "#b7791f", "#fcd05a"),
    ("sage", "#56705a", "#b5ccb0"),
    ("sand", "#8a7350", "#e2cfa8"),
    ("sepia", "#704214", "#c9a07a"),
    ("sienna", "#a0522d", "#e3a07a"),
    ("silver", "#6b7280", "#d1d5db"),
    ("slate", "#475569", "#cbd5e1"),
    ("steel", "#4a6078", "#a8bdd0"),
    ("tan", "#8a6440", "#d8b48c"),
    ("teal", "#0d7377", "#5fd4d8"),
    ("topaz", "#b8860b", "#f7d070"),
    ("umber", "#6b4423", "#c49a78"),
    ("violet", "#6d28d9", "#c4b5fd"),
    ("wine", "#7a1f3d", "#e38aa8"),
];

use crate::hash::fnv1a64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tint {
    pub light: &'static str,
    pub dark: &'static str,
}

impl Tint {
    /// Terminal rendering uses the dark-theme hex (terminals are overwhelmingly dark).
    pub fn dark_rgb(&self) -> (u8, u8, u8) {
        let p = |i: usize| u8::from_str_radix(&self.dark[i..i + 2], 16).unwrap_or(0);
        (p(1), p(3), p(5))
    }
}

fn tint_named(name: &str) -> Option<Tint> {
    COLORS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, light, dark)| Tint { light, dark })
}

/// The crew's tint is its color word: `cobalt-harbor` → cobalt.
pub fn crew_tint(crew: &str) -> Option<Tint> {
    tint_named(crew.split('-').next()?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    Circle,
    Triangle,
    Square,
    Diamond,
    Pentagon,
    Hexagon,
    Star,
    Cross,
    Ring,
    Chevron,
    InvTriangle,
    Half,
}

const SHAPES: [Shape; 12] = [
    Shape::Circle,
    Shape::Triangle,
    Shape::Square,
    Shape::Diamond,
    Shape::Pentagon,
    Shape::Hexagon,
    Shape::Star,
    Shape::Cross,
    Shape::Ring,
    Shape::Chevron,
    Shape::InvTriangle,
    Shape::Half,
];

impl Shape {
    /// Single-column glyph for terminal output.
    pub fn glyph(self) -> char {
        match self {
            Shape::Circle => '●',
            Shape::Triangle => '▲',
            Shape::Square => '■',
            Shape::Diamond => '◆',
            Shape::Pentagon => '⬟',
            Shape::Hexagon => '⬢',
            Shape::Star => '★',
            Shape::Cross => '✚',
            Shape::Ring => '◯',
            Shape::Chevron => '❯',
            Shape::InvTriangle => '▼',
            Shape::Half => '◐',
        }
    }
}

/// Badge hues: a spread subset of `COLORS`, so badges and crew tints share one palette.
const BADGE_HUES: [&str; 12] = [
    "cobalt", "jade", "amber", "fuchsia", "cyan", "violet", "ochre", "cerise", "teal", "indigo",
    "copper", "olive",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Badge {
    pub shape: Shape,
    pub tint: Tint,
}

/// Stable per role word: shape from the hash, hue from the hash's next digit.
pub fn role_badge(role: &str) -> Badge {
    let h = fnv1a64(role);
    let shape = SHAPES[(h % 12) as usize];
    let hue = BADGE_HUES[((h / 12) % 12) as usize];
    Badge {
        shape,
        tint: tint_named(hue).unwrap_or(Tint {
            light: "#6b7280",
            dark: "#d1d5db",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lum(hex: &str) -> f64 {
        let ch = |i: usize| {
            let c = u8::from_str_radix(&hex[i..i + 2], 16).unwrap() as f64 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(1) + 0.7152 * ch(3) + 0.0722 * ch(5)
    }
    fn contrast(a: &str, b: &str) -> f64 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn palette_contrast() {
        for (name, light, dark) in COLORS {
            assert!(contrast(light, "#ffffff") >= 3.0, "{name} light too faint");
            assert!(contrast(dark, "#0f1115") >= 3.0, "{name} dark too faint");
        }
    }

    #[test]
    fn crew_tint_uses_the_color_word() {
        let t = crew_tint("cobalt-harbor").unwrap();
        assert_eq!(t.light, "#1d4ed8");
        assert_eq!(t.dark_rgb(), (0x93, 0xb4, 0xfd));
        assert!(crew_tint("nope-harbor").is_none());
    }

    #[test]
    fn badges_are_stable_and_varied() {
        assert_eq!(role_badge("heron"), role_badge("heron"));
        let distinct: std::collections::HashSet<_> = crate::ROLES
            .iter()
            .map(|r| {
                let b = role_badge(r);
                (b.shape, b.tint.light)
            })
            .collect();
        assert!(
            distinct.len() >= 100,
            "only {} distinct badges",
            distinct.len()
        );
    }
}
