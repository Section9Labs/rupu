//! Codename rendering for CLI output (spec §7).

use super::palette;
use chrono::{DateTime, Utc};
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

/// One run/session that a crew name might refer to.
#[derive(Debug, Clone)]
pub struct CrewCandidate {
    pub id: String,
    pub crew: String,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrewResolution {
    NotFound,
    /// The most recent match, plus other matches from the last 30 days.
    Resolved {
        id: String,
        others: Vec<String>,
    },
}

/// Resolve a crew name to its most recent candidate. Crew names repeat over
/// time, so this never fails on ambiguity; `others` lists the additional
/// matches from the last 30 days (newest first) for a stderr note.
pub fn resolve_crew(
    candidates: &[CrewCandidate],
    crew: &str,
    now: DateTime<Utc>,
) -> CrewResolution {
    let mut hits: Vec<&CrewCandidate> = candidates.iter().filter(|c| c.crew == crew).collect();
    hits.sort_by_key(|c| std::cmp::Reverse(c.started_at));
    let Some(first) = hits.first() else {
        return CrewResolution::NotFound;
    };
    let cutoff = now - chrono::Duration::days(30);
    let others = hits[1..]
        .iter()
        .filter(|c| c.started_at >= cutoff)
        .map(|c| c.id.clone())
        .collect();
    CrewResolution::Resolved {
        id: first.id.clone(),
        others,
    }
}

/// The stderr note printed when a crew name matched more than one recent run.
pub fn ambiguity_note(crew: &str, chosen: &str, others: &[String]) -> String {
    format!(
        "note: `{crew}` also named {} in the last 30 days; using the most recent ({chosen})",
        others.join(", ")
    )
}

/// Resolve `fragment` as a codename against `candidates`, printing the
/// ambiguity note to stderr. `None` when it is not a codename or no crew matches.
pub fn resolve_codename_fragment(candidates: &[CrewCandidate], fragment: &str) -> Option<String> {
    let cn = fragment.parse::<Codename>().ok()?;
    match resolve_crew(candidates, &cn.crew, Utc::now()) {
        CrewResolution::NotFound => None,
        CrewResolution::Resolved { id, others } => {
            if !others.is_empty() {
                eprintln!("{}", ambiguity_note(&cn.crew, &id, &others));
            }
            Some(id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crew_resolves_to_most_recent_and_lists_recent_others() {
        use chrono::{Duration, TimeZone, Utc};
        let now = Utc.with_ymd_and_hms(2026, 9, 29, 12, 0, 0).unwrap();
        let c = |id: &str, crew: &str, days: i64| CrewCandidate {
            id: id.into(),
            crew: crew.into(),
            started_at: now - Duration::days(days),
        };
        let cands = vec![
            c("run_old", "jade-reef", 90),
            c("run_mid", "jade-reef", 10),
            c("run_new", "jade-reef", 1),
            c("run_x", "olive-pine", 0),
        ];
        match resolve_crew(&cands, "jade-reef", now) {
            CrewResolution::Resolved { id, others } => {
                assert_eq!(id, "run_new");
                assert_eq!(others, vec!["run_mid".to_string()]);
            }
            CrewResolution::NotFound => panic!(),
        }
        assert!(matches!(
            resolve_crew(&cands, "amber-lake", now),
            CrewResolution::NotFound
        ));
    }

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
