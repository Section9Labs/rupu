//! Codename rendering for CLI output (spec §7).

use super::palette;
use rupu_codename::{crew_tint, derive_legacy, role_badge, Codename, Tint};

/// The stored codename, else the legacy-derived one.
pub fn display_codename(stored: Option<&str>, id: &str, agent: Option<&str>) -> String {
    stored
        .map(str::to_string)
        .unwrap_or_else(|| derive_legacy(id, agent))
}

fn rgb(t: Tint) -> owo_colors::Rgb {
    let (r, g, b) = t.dark_rgb();
    owo_colors::Rgb(r, g, b)
}

/// Tint dot + bold crew name (plain under no-color; `palette` gates it).
pub fn write_crew(buf: &mut String, crew: &str) {
    if let Some(t) = crew_tint(crew) {
        let _ = palette::write_colored(buf, "●", rgb(t));
        buf.push(' ');
        let _ = palette::write_bold_colored(buf, crew, rgb(t));
    } else {
        buf.push_str(crew);
    }
}

/// Role badge glyph in its hue + the leaf (`heron#4`).
pub fn write_member(buf: &mut String, codename: &str) {
    let Ok(c) = codename.parse::<Codename>() else {
        buf.push_str(codename);
        return;
    };
    match c.segments.last() {
        Some(seg) => {
            let badge = role_badge(&seg.role);
            let _ = palette::write_colored(buf, &badge.shape.glyph().to_string(), rgb(badge.tint));
            buf.push(' ');
            buf.push_str(&c.leaf());
        }
        None => write_crew(buf, &c.crew),
    }
}

/// Plain (uncolored) role badge glyph for a codename's last segment, or `None`
/// for a crew-only / unparsable codename. Used where the surrounding text is
/// already colored as a whole (live graph rows).
pub fn badge_glyph(codename: &str) -> Option<char> {
    let c = codename.parse::<Codename>().ok()?;
    let seg = c.segments.last()?;
    Some(role_badge(&seg.role).shape.glyph())
}

/// Plain `leaf · agent · provider/model`; absent parts are omitted.
pub fn member_label(
    codename: Option<&str>,
    agent: &str,
    provider: Option<&str>,
    model: Option<&str>,
) -> String {
    let mut parts = Vec::new();
    if let Some(c) = codename.and_then(|c| c.parse::<Codename>().ok()) {
        parts.push(c.leaf());
    }
    parts.push(agent.to_string());
    match (provider, model) {
        (Some(p), Some(m)) => parts.push(format!("{p}/{m}")),
        (None, Some(m)) => parts.push(m.to_string()),
        _ => {}
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_prefers_stored_then_derives() {
        assert_eq!(
            display_codename(Some("cobalt-harbor"), "run_X", None),
            "cobalt-harbor"
        );
        let d = display_codename(None, "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", None);
        assert_eq!(
            d,
            rupu_codename::derive_legacy("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", None)
        );
    }

    #[test]
    fn member_label_joins_available_parts() {
        assert_eq!(
            member_label(
                Some("jade-reef/heron#4"),
                "security-reviewer",
                Some("anthropic"),
                Some("claude-opus-5-5")
            ),
            "heron#4 · security-reviewer · anthropic/claude-opus-5-5"
        );
        assert_eq!(member_label(None, "triage", None, None), "triage");
    }

    #[test]
    fn write_member_contains_glyph_and_leaf() {
        let mut s = String::new();
        write_member(&mut s, "jade-reef/heron#4");
        let glyph = rupu_codename::role_badge("heron").shape.glyph();
        assert!(s.contains(glyph) && s.contains("heron#4"), "{s:?}");
    }
}
