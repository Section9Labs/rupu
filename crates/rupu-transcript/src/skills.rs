//! One-line renderers for the skills events (`skill_index`, `skill_load`),
//! so every transcript view says the same thing.

use crate::event::SkillIndexEntry;

/// One line for a `skill_index`: how many skills the run bound, each marked
/// preloaded or not indexed when it is either.
pub fn skill_index_line(entries: &[SkillIndexEntry]) -> String {
    let names: Vec<String> = entries
        .iter()
        .map(|e| match (e.preload, e.indexed) {
            (true, _) => format!("{} (preloaded)", e.name),
            (false, true) => e.name.clone(),
            (false, false) => format!("{} (not indexed)", e.name),
        })
        .collect();
    let noun = if entries.len() == 1 {
        "skill"
    } else {
        "skills"
    };
    if names.is_empty() {
        return format!("0 {noun}");
    }
    format!("{} {noun}: {}", entries.len(), names.join(", "))
}

/// One line for a `skill_load`: the skill, a short content hash (when the
/// load recorded one) and how it was loaded.
pub fn skill_load_line(name: &str, via: &str, content_hash: &str) -> String {
    if content_hash.is_empty() {
        return format!("loaded skill {name} via {via}");
    }
    let short: String = content_hash.chars().take(12).collect();
    format!("loaded skill {name} ({short}) via {via}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines() {
        let e = |name: &str, preload, indexed| SkillIndexEntry {
            name: name.into(),
            layer: "global".into(),
            stamp: String::new(),
            preload,
            indexed,
            reason: "agent".into(),
        };
        assert_eq!(
            skill_index_line(&[
                e("a", true, false),
                e("b", false, true),
                e("c", false, false)
            ]),
            "3 skills: a (preloaded), b, c (not indexed)"
        );
        assert_eq!(skill_index_line(&[e("a", false, true)]), "1 skill: a");
        assert_eq!(
            skill_load_line("pdf", "model", &"ab".repeat(32)),
            "loaded skill pdf (abababababab) via model"
        );
    }

    #[test]
    fn empty_index_and_unhashed_load_read_cleanly() {
        assert_eq!(skill_index_line(&[]), "0 skills");
        assert_eq!(
            skill_load_line("pdf", "preload", ""),
            "loaded skill pdf via preload"
        );
    }
}
