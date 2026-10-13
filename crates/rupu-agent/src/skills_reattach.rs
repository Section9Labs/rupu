//! Putting a conversation's loaded skills back after a compaction
//! summarised them away (spec 2026-10-12 §6). One helper serves the
//! runner's in-turn compaction and a session's own compaction paths, so
//! they re-attach the same skills the same way.

use crate::runner::LoadedSkill;
use rupu_skills::render::{render_reattach, ReattachItem};
use rupu_skills::ResolvedSkills;

/// What a re-attach puts back and what it leaves out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reattach {
    /// The text block for the rebuilt first message; it starts with
    /// [`rupu_skills::render::REATTACH_MARKER`].
    pub block: String,
    /// The skills whose bodies are in `block`, in load order, at their
    /// current version.
    pub reattached: Vec<LoadedSkill>,
    /// Of `reattached`, the names whose version differs from the one loaded.
    pub changed: Vec<String>,
    /// Loaded skills no longer installed where they were found, most
    /// recently loaded first.
    pub missing: Vec<String>,
    /// Loaded skills left out for space, most recently loaded first.
    pub omitted: Vec<String>,
    /// The loaded set after the re-attach: every entry of the input, with
    /// the re-attached ones moved to the end at their current version. A
    /// missing or omitted skill stays, so a reinstall or a later re-attach
    /// brings it back.
    pub loaded: Vec<LoadedSkill>,
}

/// Re-reads each loaded skill at its current version and renders the
/// block that puts them back, most recently loaded first, while their
/// bodies fit in `cap_chars`. `None` when nothing is loaded.
///
/// A skill is read from where the set found it and counts as missing when
/// it is gone, no longer valid, or moved (its files would be outside the
/// run's read roots). Reading the trees and hashing their files is
/// blocking work: an async caller runs this on `spawn_blocking`.
pub fn reattach_block(
    loaded: &[LoadedSkill],
    skills: &ResolvedSkills,
    cap_chars: usize,
) -> Option<Reattach> {
    if loaded.is_empty() {
        return None;
    }
    let current: Vec<_> = loaded
        .iter()
        .map(|l| {
            let g = skills.get(&l.name)?;
            let skill = rupu_skills::load_skill(&g.skill.dir, g.skill.layer).ok()?;
            let hash = skill.content_hash().ok()?;
            (skill.root == g.skill.root).then_some((skill, hash))
        })
        .collect();
    let mut fresh = Vec::new();
    let mut omitted = Vec::new();
    let mut missing = Vec::new();
    let mut used = 0usize;
    for (at, (l, current)) in loaded.iter().zip(current).enumerate().rev() {
        let Some((skill, hash)) = current else {
            missing.push(l.name.clone());
            continue;
        };
        if used + skill.body.len() > cap_chars {
            omitted.push(l.name.clone());
            continue;
        }
        used += skill.body.len();
        fresh.push((at, skill, hash));
    }
    let items: Vec<ReattachItem<'_>> = fresh
        .iter()
        .map(|(_, s, _)| ReattachItem {
            name: s.name.as_str(),
            base_dir: &s.root,
            body: &s.body,
        })
        .collect();
    let omitted_ref: Vec<&str> = omitted.iter().map(String::as_str).collect();
    let missing_ref: Vec<&str> = missing.iter().map(String::as_str).collect();
    let block = render_reattach(&items, &omitted_ref, &missing_ref);

    fresh.sort_by_key(|(at, ..)| *at);
    let mut changed = Vec::new();
    let mut reattached = Vec::new();
    for (at, skill, hash) in fresh {
        let name = skill.name.as_str().to_owned();
        let hash = hash.hex();
        if loaded[at].content_hash != hash {
            changed.push(name.clone());
        }
        reattached.push(LoadedSkill {
            name,
            content_hash: hash,
        });
    }
    let mut after: Vec<LoadedSkill> = loaded
        .iter()
        .filter(|l| !reattached.iter().any(|r| r.name == l.name))
        .cloned()
        .collect();
    after.extend(reattached.iter().cloned());
    Some(Reattach {
        block,
        reattached,
        changed,
        missing,
        omitted,
        loaded: after,
    })
}
