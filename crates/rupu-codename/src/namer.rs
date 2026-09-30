//! Per-crew role allocation + instance counters. Deterministic given the
//! same sequence of calls, which is what lets resume recompute static slots.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::hash::fnv1a64;
use crate::{Codename, ROLES};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrewNamer {
    pub crew: String,
    /// agent def → words allocated to it, in order; `[0]` is canonical.
    #[serde(default)]
    roles: BTreeMap<String, Vec<String>>,
    /// `"<parent codename>><role>"` → last issued instance number.
    #[serde(default)]
    counters: BTreeMap<String, u32>,
}

impl CrewNamer {
    pub fn new(crew: impl Into<String>) -> Self {
        Self {
            crew: crew.into(),
            ..Self::default()
        }
    }

    fn taken(&self, word: &str) -> bool {
        self.roles.values().flatten().any(|w| w == word)
    }

    fn probe(&self, agent_def: &str) -> String {
        let start = (fnv1a64(agent_def) % ROLES.len() as u64) as usize;
        for i in 0..ROLES.len() {
            let w = ROLES[(start + i) % ROLES.len()];
            if !self.taken(w) {
                return w.to_string();
            }
        }
        // More distinct slots than words in one crew: number the base word.
        let base = ROLES[start];
        (2..)
            .map(|k| format!("{base}{k}"))
            .find(|w| !self.taken(w))
            .unwrap_or_else(|| base.to_string())
    }

    /// A new word for a new static slot of `agent_def`.
    pub fn allocate_role(&mut self, agent_def: &str) -> String {
        let w = self.probe(agent_def);
        self.roles
            .entry(agent_def.to_string())
            .or_default()
            .push(w.clone());
        w
    }

    /// The def's canonical word, allocating on first use (dynamic dispatch).
    pub fn canonical_role(&mut self, agent_def: &str) -> String {
        match self.roles.get(agent_def).and_then(|v| v.first()) {
            Some(w) => w.clone(),
            None => self.allocate_role(agent_def),
        }
    }

    /// Record a word decided elsewhere (a placed unit's coordinator) as canonical.
    pub fn seed_role(&mut self, agent_def: &str, word: &str) {
        let v = self.roles.entry(agent_def.to_string()).or_default();
        if !v.iter().any(|w| w == word) {
            v.insert(0, word.to_string());
        }
    }

    pub fn next_instance(&mut self, parent: &Codename, role: &str) -> u32 {
        let c = self.counters.entry(format!("{parent}>{role}")).or_insert(0);
        *c += 1;
        *c
    }
}

/// A `CrewNamer` shared by the orchestrator and the sub-agent dispatcher,
/// optionally persisted as JSON after every change.
#[derive(Debug, Clone)]
pub struct SharedNamer {
    inner: Arc<Mutex<CrewNamer>>,
    path: Option<PathBuf>,
}

impl SharedNamer {
    pub fn in_memory(namer: CrewNamer) -> Self {
        Self {
            inner: Arc::new(Mutex::new(namer)),
            path: None,
        }
    }

    /// Load `path` if it holds a namer; otherwise build one with `init` and
    /// persist it. A corrupt file is logged and replaced by `init()`.
    pub fn open_or_init(path: PathBuf, init: impl FnOnce() -> CrewNamer) -> Self {
        let loaded =
            std::fs::read(&path)
                .ok()
                .and_then(|b| match serde_json::from_slice::<CrewNamer>(&b) {
                    Ok(n) => Some(n),
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "codenames.json unreadable; reinitialising"
                        );
                        None
                    }
                });
        let fresh = loaded.is_none();
        let s = Self {
            inner: Arc::new(Mutex::new(loaded.unwrap_or_else(init))),
            path: Some(path),
        };
        if fresh {
            s.persist(&s.snapshot());
        }
        s
    }

    fn snapshot(&self) -> CrewNamer {
        match self.inner.lock() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    fn persist(&self, state: &CrewNamer) {
        let Some(path) = &self.path else { return };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
            std::fs::rename(&tmp, path)
        };
        if let Err(e) = write() {
            // Names are already stored on the records that carry them; the
            // file only protects resume/counters, so a failed write degrades
            // rather than fails the run.
            tracing::warn!(path = %path.display(), error = %e, "failed to persist codenames.json");
        }
    }

    /// Run `f` against the namer; persist if it changed state (while holding the lock).
    pub fn with<R>(&self, f: impl FnOnce(&mut CrewNamer) -> R) -> R {
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let before = guard.clone();
        let out = f(&mut guard);
        if *guard != before {
            self.persist_locked(&guard);
        }
        out
    }

    /// Persist while holding the MutexGuard to serialize writes.
    fn persist_locked(&self, state: &CrewNamer) {
        let Some(path) = &self.path else { return };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
            std::fs::rename(&tmp, path)
        };
        if let Err(e) = write() {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to persist codenames.json"
            );
        }
    }

    pub fn crew(&self) -> String {
        self.snapshot().crew
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_def_second_slot_gets_next_free_word() {
        let mut n = CrewNamer::new("jade-reef");
        assert_eq!(n.allocate_role("ag"), "hedgehog");
        assert_eq!(n.allocate_role("ag"), "heron"); // ROLES order: hedgehog, heron
        assert_eq!(n.canonical_role("ag"), "hedgehog");
    }

    #[test]
    fn different_defs_colliding_on_a_word_are_probed() {
        let mut n = CrewNamer::new("jade-reef");
        // Verify that security-reviewer's base word is ferret (no collision yet).
        assert_eq!(crate::role_word("security-reviewer"), "ferret");
        n.seed_role("other", "ferret");
        // Now ferret is taken, so canonical_role must probe to a different word.
        assert_ne!(n.canonical_role("security-reviewer"), "ferret");
    }

    #[test]
    fn instance_counters_are_per_parent_and_role() {
        let mut n = CrewNamer::new("jade-reef");
        let a: Codename = "jade-reef/heron#1".parse().unwrap();
        let b: Codename = "jade-reef/heron#2".parse().unwrap();
        assert_eq!(n.next_instance(&a, "lynx"), 1);
        assert_eq!(n.next_instance(&a, "lynx"), 2);
        assert_eq!(n.next_instance(&b, "lynx"), 1);
        assert_eq!(n.next_instance(&a, "otter"), 1);
    }

    #[test]
    fn exhausted_pool_falls_back_to_numbered_words() {
        let mut n = CrewNamer::new("jade-reef");
        for i in 0..crate::ROLES.len() {
            n.allocate_role(&format!("def{i}"));
        }
        let extra = n.allocate_role("one-more");
        assert!(extra.ends_with('2'), "{extra}");
    }

    #[test]
    fn shared_namer_persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run").join("codenames.json");
        let s = SharedNamer::open_or_init(path.clone(), || CrewNamer::new("jade-reef"));
        let parent: Codename = "jade-reef/heron".parse().unwrap();
        assert_eq!(s.with(|n| n.next_instance(&parent, "lynx")), 1);
        let again = SharedNamer::open_or_init(path, || panic!("must load, not init"));
        assert_eq!(again.with(|n| n.next_instance(&parent, "lynx")), 2);
        assert_eq!(again.crew(), "jade-reef");
    }

    #[test]
    fn concurrent_with_serializes_persists() {
        use std::sync::Arc;
        use std::thread;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run").join("codenames.json");
        let s = Arc::new(SharedNamer::open_or_init(path.clone(), || {
            CrewNamer::new("jade-reef")
        }));
        let parent: Codename = "jade-reef/heron".parse().unwrap();

        // Spawn 8 threads, each calling next_instance 25 times (200 total calls).
        let mut handles = vec![];
        for _ in 0..8 {
            let s_clone = Arc::clone(&s);
            let parent_clone = parent.clone();
            let handle = thread::spawn(move || {
                for _ in 0..25 {
                    s_clone.with(|n| n.next_instance(&parent_clone, "lynx"));
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Reload the file: next instance should be 201 (proof that all 200
        // increments were persisted without loss).
        let reloaded = SharedNamer::open_or_init(path, || panic!("must load, not init"));
        let next = reloaded.with(|n| n.next_instance(&parent, "lynx"));
        assert_eq!(next, 201);
    }
}
