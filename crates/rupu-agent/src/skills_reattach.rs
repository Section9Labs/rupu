//! Putting a conversation's loaded skills back after a compaction
//! summarised them away (spec 2026-10-12 §6). One helper serves the
//! runner's in-turn compaction and a session's own compaction paths, so
//! they re-attach the same skills the same way.

use crate::runner::LoadedSkill;
use rupu_skills::render::{render_reattach, ReattachItem};
use rupu_skills::{Layer, ResolvedSkills};
use rupu_transcript::Event;
use std::path::PathBuf;

/// What a re-attach puts back and what it leaves out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reattach {
    /// The text block for the rebuilt first message; it starts with
    /// [`rupu_skills::render::REATTACH_MARKER`].
    pub block: String,
    /// The skills whose bodies are in `block`, in load order, at their
    /// current version.
    pub reattached: Vec<Reattached>,
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

impl Reattach {
    /// The transcript events that record this re-attach, for after the
    /// compaction that made it: a `skill_load { via: reattach }` per
    /// re-attached skill (in load order, so a replay folds the loaded set
    /// into [`Self::loaded`]'s order), each after its `skill_changed`
    /// notice, then a `skill_missing` notice per missing skill and one
    /// `skill_reattach_truncated` for those left out.
    pub fn events(&self) -> Vec<Event> {
        let mut events = Vec::new();
        for r in &self.reattached {
            let name = &r.name;
            if self.changed.contains(name) {
                events.push(Event::Notice {
                    kind: "skill_changed".into(),
                    message: format!(
                        "skill {name} changed on disk since it was loaded; its current version \
                         was re-attached"
                    ),
                });
            }
            events.push(Event::SkillLoad {
                name: name.clone(),
                layer: r.layer.as_str().to_owned(),
                content_hash: r.content_hash.clone(),
                base_dir: r.base_dir.display().to_string(),
                via: "reattach".into(),
            });
        }
        for name in &self.missing {
            events.push(Event::Notice {
                kind: "skill_missing".into(),
                message: format!("skill {name} is no longer installed and was not re-attached"),
            });
        }
        if !self.omitted.is_empty() {
            events.push(Event::Notice {
                kind: "skill_reattach_truncated".into(),
                message: format!("not re-attached for space: {}", self.omitted.join(", ")),
            });
        }
        events
    }
}

/// A skill [`Reattach::block`] put back: what its `skill_load
/// { via: reattach }` records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reattached {
    pub name: String,
    pub layer: Layer,
    pub content_hash: String,
    pub base_dir: PathBuf,
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
        reattached.push(Reattached {
            name,
            layer: skill.layer,
            content_hash: hash,
            base_dir: skill.root,
        });
    }
    let mut after: Vec<LoadedSkill> = loaded
        .iter()
        .filter(|l| !reattached.iter().any(|r| r.name == l.name))
        .cloned()
        .collect();
    after.extend(reattached.iter().map(|r| LoadedSkill {
        name: r.name.clone(),
        content_hash: r.content_hash.clone(),
    }));
    Some(Reattach {
        block,
        reattached,
        changed,
        missing,
        omitted,
        loaded: after,
    })
}
