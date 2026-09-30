//! Per-crew role allocation + instance counters. Deterministic given the
//! same sequence of calls, which is what lets resume recompute static slots.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::hash::fnv1a64;
use crate::{Codename, ROLES};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CrewNamer {
    pub crew: String,
    /// agent def → words allocated to it, in order; `[0]` is canonical.
    #[serde(default)]
    roles: BTreeMap<String, Vec<String>>,
    /// `"<parent codename>><role>"` → last issued instance number.
    #[serde(default)]
    counters: BTreeMap<String, u32>,
    /// Set by every state-changing call; [`SharedNamer::with`] clears it and
    /// persists when it was set. Not part of the namer's value.
    #[serde(skip)]
    dirty: bool,
}

impl PartialEq for CrewNamer {
    fn eq(&self, other: &Self) -> bool {
        self.crew == other.crew && self.roles == other.roles && self.counters == other.counters
    }
}

impl Eq for CrewNamer {}

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
        self.dirty = true;
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
            self.dirty = true;
        }
    }

    pub fn next_instance(&mut self, parent: &Codename, role: &str) -> u32 {
        let c = self.counters.entry(format!("{parent}>{role}")).or_insert(0);
        *c += 1;
        let n = *c;
        self.dirty = true;
        n
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
            let guard = s.lock();
            s.persist(&guard);
        }
        s
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CrewNamer> {
        match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    /// Write `state` to the namer file. Callers hold the namer's lock, so
    /// in-process writes are ordered; the temp name is unique per process
    /// and call (`<file>.<pid>.<seq>.tmp`), so two processes sharing a run
    /// dir never interleave bytes in one temp file before the atomic rename.
    fn persist(&self, state: &CrewNamer) {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let Some(path) = &self.path else { return };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut tmp = path.clone().into_os_string();
            tmp.push(format!(".{}.{seq}.tmp", std::process::id()));
            let tmp = PathBuf::from(tmp);
            std::fs::write(&tmp, serde_json::to_vec(state)?)?;
            std::fs::rename(&tmp, path).inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })
        };
        if let Err(e) = write() {
            // Names are already stored on the records that carry them; the
            // file only protects resume/counters, so a failed write degrades
            // rather than fails the run.
            tracing::warn!(path = %path.display(), error = %e, "failed to persist codenames.json");
        }
    }

    /// Run `f` against the namer; persist (still holding the lock, so writes
    /// land in call order) if it changed state.
    pub fn with<R>(&self, f: impl FnOnce(&mut CrewNamer) -> R) -> R {
        let mut guard = self.lock();
        guard.dirty = false;
        let out = f(&mut guard);
        if std::mem::take(&mut guard.dirty) {
            self.persist(&guard);
        }
        out
    }

    pub fn crew(&self) -> String {
        self.lock().crew.clone()
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
    fn with_persists_only_on_change_compact_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("codenames.json");
        let s = SharedNamer::open_or_init(path.clone(), || CrewNamer::new("jade-reef"));
        s.with(|n| n.allocate_role("ag"));
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(!body.contains('\n'), "compact JSON: {body}");
        assert!(
            !body.contains("dirty"),
            "dirty flag is not persisted: {body}"
        );

        // A read-only call (the def already has a canonical word) does not
        // rewrite the file.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(s.with(|n| n.canonical_role("ag")), "hedgehog");
        assert!(!path.exists(), "unchanged namer must not be persisted");

        s.with(|n| n.seed_role("ag", "hedgehog")); // already present: no-op
        assert!(!path.exists());
        s.with(|n| n.seed_role("other", "lynx"));
        assert!(path.exists());
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
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
