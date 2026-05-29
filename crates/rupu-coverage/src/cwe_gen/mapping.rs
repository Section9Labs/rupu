//! Maps `RawWeakness` + view membership into `Concern` records that
//! the rupu-coverage catalog system can consume.

use crate::catalog::types::{Concern, Severity, TouchStrength};
use crate::cwe_gen::xml::{ParsedCwe, RawWeakness};

/// Map a parsed CWE corpus + a view ID into a list of Concern records.
/// Returns `None` if the view ID isn't found in the parsed corpus.
pub fn map_view_to_concerns(
    parsed: &ParsedCwe,
    view_id: u32,
    namespace: &str,
) -> Option<Vec<Concern>> {
    let view = parsed.views.iter().find(|v| v.id == view_id)?;

    // A weakness belongs to the view if EITHER:
    //  (a) it is reachable through the view's category `<Has_Member>`
    //      membership (used by views like CWE-699 Software Development), OR
    //  (b) it declares the view in its own `<Related_Weaknesses>` via a
    //      `View_ID` (used by graph views like CWE-1000 Research Concepts,
    //      whose `<Members>` only list the top-level pillars).
    let mut member_ids: std::collections::BTreeSet<u32> =
        view.member_weakness_ids.iter().copied().collect();
    for w in &parsed.weaknesses {
        if w.member_of_views.contains(&view_id) {
            member_ids.insert(w.id);
        }
    }

    let weakness_by_id: std::collections::HashMap<u32, &RawWeakness> =
        parsed.weaknesses.iter().map(|w| (w.id, w)).collect();
    let mut concerns: Vec<Concern> = member_ids
        .iter()
        .filter_map(|id| weakness_by_id.get(id).copied())
        .map(|w| map_weakness(w, namespace))
        .collect();
    concerns.sort_by(|a, b| a.id.cmp(&b.id));
    Some(concerns)
}

fn map_weakness(w: &RawWeakness, namespace: &str) -> Concern {
    let id = format!("{namespace}:cwe-{}-{}", w.id, slug(&w.name));
    let description = compose_description(w);
    Concern {
        id,
        name: format!("CWE-{} — {}", w.id, w.name),
        description,
        severity: severity_from_impact(&w.impact_tags),
        applicable_globs: globs_from_languages(&w.applicable_languages),
        min_strength: TouchStrength::Read,
        references: vec![format!("https://cwe.mitre.org/data/definitions/{}.html", w.id)],
        tags: tags_from_languages(&w.applicable_languages),
    }
}

fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.extend(ch.to_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn compose_description(w: &RawWeakness) -> String {
    let mut out = w.description.clone();
    if let Some(extended) = &w.extended_description {
        if !extended.is_empty() {
            out.push_str("\n\n");
            out.push_str(extended);
        }
    }
    // Cap to 600 chars (UTF-8-safe); full body available via
    // coverage_concerns_detail.
    if out.len() > 600 {
        let mut cut = 597;
        while cut > 0 && !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push_str("...");
    }
    out
}

fn severity_from_impact(impact_tags: &[String]) -> Severity {
    let s: String = impact_tags.join(" ").to_lowercase();
    if s.contains("execute unauthorized code")
        || s.contains("gain privileges")
        || s.contains("bypass protection")
        || s.contains("modify memory")
    {
        Severity::Critical
    } else if s.contains("read application data")
        || s.contains("modify application data")
        || s.contains("read memory")
        || s.contains("hide activities")
    {
        Severity::High
    } else if s.contains("dos")
        || s.contains("denial of service")
        || s.contains("resource consumption")
    {
        Severity::Medium
    } else {
        // No recognized impact (or empty) → Medium default.
        Severity::Medium
    }
}

fn globs_from_languages(langs: &[String]) -> Vec<String> {
    if langs.is_empty()
        || langs
            .iter()
            .any(|l| l.eq_ignore_ascii_case("Not Language-Specific"))
    {
        return vec!["**".to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for lang in langs {
        for glob in language_to_globs(lang) {
            if seen.insert((*glob).to_string()) {
                out.push((*glob).to_string());
            }
        }
    }
    if out.is_empty() {
        vec!["**".to_string()]
    } else {
        out
    }
}

fn language_to_globs(lang: &str) -> &'static [&'static str] {
    match lang.to_lowercase().as_str() {
        "c" | "c++" => &["**/*.c", "**/*.cpp", "**/*.h", "**/*.hpp", "**/*.cc", "**/*.cxx"],
        "rust" => &["**/*.rs"],
        "python" => &["**/*.py"],
        "java" => &["**/*.java"],
        "javascript" => &["**/*.js", "**/*.jsx", "**/*.mjs"],
        "typescript" => &["**/*.ts", "**/*.tsx"],
        "go" => &["**/*.go"],
        "ruby" => &["**/*.rb"],
        "php" => &["**/*.php"],
        "c#" => &["**/*.cs"],
        "swift" => &["**/*.swift"],
        "kotlin" => &["**/*.kt", "**/*.kts"],
        _ => &[],
    }
}

fn tags_from_languages(langs: &[String]) -> Vec<String> {
    langs
        .iter()
        .map(|l| format!("lang:{}", l.to_lowercase().replace(' ', "-")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_weakness() -> RawWeakness {
        RawWeakness {
            id: 787,
            name: "Out-of-bounds Write".to_string(),
            description: "The code writes data past the end of the intended buffer.".to_string(),
            extended_description: None,
            impact_tags: vec!["Modify Memory".to_string()],
            applicable_languages: vec!["C".to_string(), "C++".to_string()],
            member_of_views: vec![1000, 699],
        }
    }

    #[test]
    fn map_view_includes_weaknesses_by_related_weakness_membership() {
        use crate::cwe_gen::xml::{ParsedCwe, RawView};
        // A graph-type view whose <Members> list only the pillar, but two
        // weaknesses declare membership via member_of_views.
        let parsed = ParsedCwe {
            weaknesses: vec![
                RawWeakness {
                    id: 787,
                    name: "OOB Write".to_string(),
                    description: "d".to_string(),
                    extended_description: None,
                    impact_tags: vec![],
                    applicable_languages: vec![],
                    member_of_views: vec![1000],
                },
                RawWeakness {
                    id: 125,
                    name: "OOB Read".to_string(),
                    description: "d".to_string(),
                    extended_description: None,
                    impact_tags: vec![],
                    applicable_languages: vec![],
                    member_of_views: vec![1000],
                },
                RawWeakness {
                    id: 79,
                    name: "XSS".to_string(),
                    description: "d".to_string(),
                    extended_description: None,
                    impact_tags: vec![],
                    applicable_languages: vec![],
                    member_of_views: vec![699], // NOT in view 1000
                },
            ],
            views: vec![RawView {
                id: 1000,
                name: "Research".to_string(),
                member_weakness_ids: vec![], // pillars resolved to nothing here
            }],
        };
        let concerns = map_view_to_concerns(&parsed, 1000, "cwe-research").unwrap();
        // 787 and 125 declare view 1000; 79 does not.
        assert_eq!(concerns.len(), 2);
        assert!(concerns.iter().any(|c| c.id.contains("cwe-787-")));
        assert!(concerns.iter().any(|c| c.id.contains("cwe-125-")));
        assert!(!concerns.iter().any(|c| c.id.contains("cwe-79-")));
    }

    #[test]
    fn map_weakness_produces_expected_concern() {
        let c = map_weakness(&fixture_weakness(), "cwe-research");
        assert_eq!(c.id, "cwe-research:cwe-787-out-of-bounds-write");
        assert_eq!(c.name, "CWE-787 — Out-of-bounds Write");
        assert_eq!(c.severity, Severity::Critical); // "modify memory" → Critical
        assert!(c.applicable_globs.iter().any(|g| g == "**/*.c"));
        assert!(c.references[0].contains("787"));
        assert!(c.tags.iter().any(|t| t == "lang:c"));
    }

    #[test]
    fn slug_handles_special_chars() {
        assert_eq!(slug("Out-of-bounds Write"), "out-of-bounds-write");
        assert_eq!(slug("OS Command Injection"), "os-command-injection");
        assert_eq!(slug("XSS / Cross-site Scripting"), "xss-cross-site-scripting");
    }

    #[test]
    fn severity_heuristics_cover_known_impacts() {
        assert_eq!(
            severity_from_impact(&["Execute Unauthorized Code".to_string()]),
            Severity::Critical
        );
        assert_eq!(
            severity_from_impact(&["Read Application Data".to_string()]),
            Severity::High
        );
        assert_eq!(
            severity_from_impact(&["DoS: Resource Consumption".to_string()]),
            Severity::Medium
        );
        assert_eq!(severity_from_impact(&[]), Severity::Medium);
    }

    #[test]
    fn globs_default_to_double_star_when_unknown_language() {
        let g = globs_from_languages(&["NotALanguage".to_string()]);
        assert_eq!(g, vec!["**".to_string()]);
    }
}
