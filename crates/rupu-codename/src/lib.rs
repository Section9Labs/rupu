//! Human codenames for rupu runs, sessions and agent instances —
//! `cobalt-harbor/heron#412>lynx#3`. Leaf crate: no rupu deps.
//! Spec: docs/superpowers/specs/2026-09-29-rupu-agent-codenames-design.md

mod codename;
mod hash;
mod palette;
mod words;

pub use codename::{Codename, ParseCodenameError, Segment};
pub use palette::{crew_tint, role_badge, Badge, Shape, Tint, COLORS};
pub use words::{NOUNS, ROLES};

use hash::fnv1a64;

/// Crew name for a top-level id (workflow run, standalone run, session).
pub fn crew_for(id: &str) -> String {
    let h = fnv1a64(id);
    let c = COLORS.len() as u64;
    let color = COLORS[(h % c) as usize].0;
    let noun = NOUNS[((h / c) % NOUNS.len() as u64) as usize];
    format!("{color}-{noun}")
}

/// Base role word for an agent definition. No collision handling — use
/// [`CrewNamer`] when minting; this is for singletons and legacy derivation.
pub fn role_word(agent_def: &str) -> &'static str {
    ROLES[(fnv1a64(agent_def) % ROLES.len() as u64) as usize]
}

/// Name for a record written before codenames existed. Only `rupu-cp` DTOs
/// and CLI listings call this; it is never stored.
pub fn derive_legacy(id: &str, agent: Option<&str>) -> String {
    let crew = Codename::crew_only(crew_for(id));
    match agent {
        Some(a) => crew.child(role_word(a), None).to_string(),
        None => crew.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_crews() {
        // Pinned: a change here means the word lists or hash changed, which
        // renames every legacy (derive-on-read) record. Don't "fix" by updating.
        assert_eq!(crew_for("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W"), "jade-reef");
        assert_eq!(crew_for("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2X"), "peach-prairie");
        assert_eq!(crew_for("ses_01J9ZQ3K4M5N6P7Q8R9S0T1V2W"), "olive-pine");
    }

    #[test]
    fn golden_roles() {
        assert_eq!(role_word("security-reviewer"), "ferret");
        assert_eq!(role_word("triage"), "numbat");
        assert_eq!(role_word("ag"), "hedgehog");
    }

    #[test]
    fn whole_id_is_hashed() {
        // Same-millisecond ULIDs share their first 10 chars; only the tail differs.
        assert_ne!(
            crew_for("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W"),
            crew_for("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2X")
        );
    }

    #[test]
    fn word_list_hygiene() {
        use std::collections::HashSet;
        let colors: Vec<&str> = COLORS.iter().map(|c| c.0).collect();
        for (name, list, min) in [
            ("colors", &colors[..], 48),
            ("nouns", NOUNS, 200),
            ("roles", ROLES, 250),
        ] {
            assert!(list.len() >= min, "{name}: {} < {min}", list.len());
            let set: HashSet<_> = list.iter().collect();
            assert_eq!(set.len(), list.len(), "{name} has duplicates");
            for w in list {
                assert!(
                    w.len() <= 8 && w.chars().all(|c| c.is_ascii_lowercase()),
                    "{name}: bad word {w:?}"
                );
            }
        }
        let c: HashSet<_> = colors.iter().copied().collect();
        let n: HashSet<_> = NOUNS.iter().copied().collect();
        let r: HashSet<_> = ROLES.iter().copied().collect();
        assert!(c.is_disjoint(&n) && c.is_disjoint(&r) && n.is_disjoint(&r));
    }

    #[test]
    fn derive_legacy_shapes() {
        assert_eq!(
            derive_legacy("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", None),
            "jade-reef"
        );
        assert_eq!(
            derive_legacy("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", Some("triage")),
            "jade-reef/numbat"
        );
    }
}
