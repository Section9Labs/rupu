//! Workspace record store. Lives at `~/.rupu/workspaces/`.
//!
//! Records are keyed by canonicalized path; on `upsert` we read every
//! record in the store dir and reuse the matching one rather than
//! generating a new id. New records auto-detect the git remote and
//! default branch (snapshot at workspace-creation time only — they are
//! not refreshed on subsequent runs).
//!
//! Registration is idempotent by path under concurrency. Every launch
//! upserts, and a fan-out (or any two processes) can first-upsert one path at
//! the same instant; without coordination each read an empty store and minted
//! its own id, leaving two records with the IDENTICAL path. Every reader that
//! walks the store then read that path's coverage ledger twice and
//! double-counted its findings, assets and usage. Two guards close this:
//! [`with_registration_lock`] serializes the find-or-create across processes
//! so a second id is never minted, and [`dedup_by_path`] (applied by
//! [`WorkspaceStore::list`]) collapses any already-duplicated path to one
//! deterministic record so existing data — and anything that ever slips the
//! lock on a filesystem that cannot hold one — stops doubling for every
//! reader at once.

use crate::record::{new_id, Workspace};
use chrono::Utc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::warn;

/// Errors from the workspace record store.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io {action}: {source}")]
    Io {
        action: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("serialize: {0}")]
    Ser(#[from] toml::ser::Error),
    #[error("workspace path is not valid UTF-8: {path}")]
    NonUtf8Path { path: String },
}

/// Handle to the on-disk workspace store directory.
#[derive(Debug, Clone)]
pub struct WorkspaceStore {
    /// Root directory of the store (typically `~/.rupu/workspaces/`).
    pub root: PathBuf,
}

impl WorkspaceStore {
    fn ensure_root(&self) -> Result<(), StoreError> {
        std::fs::create_dir_all(&self.root).map_err(|e| StoreError::Io {
            action: format!("create_dir_all {}", self.root.display()),
            source: e,
        })
    }

    fn record_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.toml"))
    }

    /// `<root>/<id>.customer` — the customer-assignment sidecar
    /// (`crate::customers`). Not a `.toml` file, so [`Self::list`] skips it.
    pub(crate) fn customer_sidecar_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.customer"))
    }

    pub fn load(&self, id: &str) -> Result<Option<Workspace>, StoreError> {
        let path = self.record_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|e| StoreError::Io {
            action: format!("read_to_string {}", path.display()),
            source: e,
        })?;
        let ws = toml::from_str(&text).map_err(|e| StoreError::Parse {
            path: path.display().to_string(),
            source: e,
        })?;
        Ok(Some(ws))
    }

    pub fn list(&self) -> Result<Vec<Workspace>, StoreError> {
        if !self.root.exists() {
            return Ok(vec![]);
        }
        let mut out = vec![];
        for entry in std::fs::read_dir(&self.root).map_err(|e| StoreError::Io {
            action: format!("read_dir {}", self.root.display()),
            source: e,
        })? {
            let entry = entry.map_err(|e| StoreError::Io {
                action: "read_dir entry".into(),
                source: e,
            })?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    warn!(
                        path = %path.display(),
                        error = %e,
                        "skipping unreadable workspace record"
                    );
                    continue;
                }
            };
            let ws: Workspace = match toml::from_str(&text) {
                Ok(w) => w,
                Err(e) => {
                    warn!(
                        path = %path.display(),
                        error = %e,
                        "skipping corrupt workspace record"
                    );
                    continue;
                }
            };
            out.push(ws);
        }
        Ok(dedup_by_path(out))
    }

    /// Write the record atomically: serialize into a uniquely named temp
    /// file in the store dir, then rename it over `<id>.toml`. Every launch
    /// upserts, so two runs of one project write the same record at once; a
    /// shared fixed temp name would let them tear each other's bytes into a
    /// corrupt record (which `list` then skips and the customer lookup
    /// refuses). The temp name (`.tmpXXXX`) never ends in `.toml`, so `list`
    /// ignores it, and a failed write or rename removes it.
    fn write(&self, ws: &Workspace) -> Result<(), StoreError> {
        use std::io::Write;
        self.ensure_root()?;
        let body = toml::to_string(ws)?;
        let path = self.record_path(&ws.id);
        let mut tmp = tempfile::NamedTempFile::new_in(&self.root).map_err(|e| StoreError::Io {
            action: format!("create temp file in {}", self.root.display()),
            source: e,
        })?;
        tmp.write_all(body.as_bytes())
            .and_then(|()| tmp.as_file().sync_all())
            .map_err(|e| StoreError::Io {
                action: format!("write temp file for {}", path.display()),
                source: e,
            })?;
        tmp.persist(&path).map_err(|e| StoreError::Io {
            action: format!("rename temp file -> {}", path.display()),
            source: e.error,
        })?;
        Ok(())
    }
}

/// Canonicalize `path` and return it with its UTF-8 string form — the key a
/// workspace record is matched on. Shared by [`upsert`], [`find_by_path`]
/// and [`register`] so the three can never disagree on what "the same
/// workspace" means.
fn canonical_key(path: &Path) -> Result<(PathBuf, String), StoreError> {
    let canonical = path.canonicalize().map_err(|e| StoreError::Io {
        action: format!("canonicalize {}", path.display()),
        source: e,
    })?;
    // Use to_str() to avoid display()'s lossy replacement chars on
    // non-UTF-8 paths. The path is the lookup key for "same workspace
    // already recorded" — a mangled path here would create a duplicate
    // record on every run.
    let s = canonical
        .to_str()
        .ok_or_else(|| StoreError::NonUtf8Path {
            path: canonical.display().to_string(),
        })?
        .to_string();
    Ok((canonical, s))
}

fn find_canonical(
    store: &WorkspaceStore,
    canonical: &Path,
) -> Result<Option<Workspace>, StoreError> {
    Ok(store.list()?.into_iter().find(|w| {
        Path::new(&w.path)
            .canonicalize()
            .map(|p| p == canonical)
            .unwrap_or(false)
    }))
}

/// Collapse records that share a stored `path` to one deterministic winner:
/// the lexicographically-smallest id, a ULID, so the earliest-registered.
/// First-appearance order of each distinct path is preserved; reads stay pure
/// (no file is moved or removed — the losing record simply never surfaces).
///
/// A lost registration race leaves two records whose `path` strings are byte
/// identical (both written by [`canonical_key`]), so grouping on the stored
/// string collapses the real duplicates without a `canonicalize` syscall per
/// record on this hot read path.
fn dedup_by_path(records: Vec<Workspace>) -> Vec<Workspace> {
    let mut chosen: HashMap<String, Workspace> = HashMap::with_capacity(records.len());
    let mut order: Vec<String> = Vec::with_capacity(records.len());
    for w in records {
        match chosen.get(&w.path) {
            Some(existing) => {
                if w.id < existing.id {
                    chosen.insert(w.path.clone(), w);
                }
            }
            None => {
                order.push(w.path.clone());
                chosen.insert(w.path.clone(), w);
            }
        }
    }
    order
        .into_iter()
        .map(|p| chosen.remove(&p).expect("every ordered path was inserted"))
        .collect()
}

/// Sidecar in the store root whose exclusive lock serializes registration.
/// Not the record files themselves — those are replaced by rename, and a lock
/// held on a replaced file would not exclude the next writer. Skipped by
/// [`WorkspaceStore::list`] (not a `.toml`) and by the customer scan (not a
/// `.customer`).
const REGISTRATION_LOCK: &str = ".registration.lock";

/// Run the find-or-create `f` holding the store's registration lock, so two
/// processes first-upserting one path serialize and the second adopts the
/// record the first just wrote instead of minting a new id.
///
/// Where the lock cannot be taken — the file cannot be opened, or the
/// filesystem does not support locking — `f` runs unlocked after a warning
/// rather than failing a registration that used to succeed; [`dedup_by_path`]
/// then still collapses any duplicate the lost race creates. Released when the
/// returned handle drops, after `f` has written.
fn with_registration_lock<T>(
    store: &WorkspaceStore,
    f: impl FnOnce() -> Result<T, StoreError>,
) -> Result<T, StoreError> {
    store.ensure_root()?;
    let lock_path = store.root.join(REGISTRATION_LOCK);
    let lock = match std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
    {
        Ok(file) => match file.lock() {
            Ok(()) => Some(file),
            Err(e) => {
                warn!(
                    error = %e,
                    lock = %lock_path.display(),
                    "cannot lock the workspace registration sidecar; registering without it"
                );
                None
            }
        },
        Err(e) => {
            warn!(
                error = %e,
                lock = %lock_path.display(),
                "cannot open the workspace registration sidecar; registering without it"
            );
            None
        }
    };
    let out = f();
    // Released only after `f`'s write has landed.
    drop(lock);
    out
}

/// The workspace recorded for `path`, if any. Never creates one.
pub fn find_by_path(store: &WorkspaceStore, path: &Path) -> Result<Option<Workspace>, StoreError> {
    let (canonical, _) = canonical_key(path)?;
    find_canonical(store, &canonical)
}

/// The workspace for `path`, registering it if absent. Unlike [`upsert`]
/// this is not a run: it leaves `last_run_at` alone (unset on a new
/// record). Used by customer assignment, which may name a project rupu has
/// never run in.
pub fn register(store: &WorkspaceStore, path: &Path) -> Result<Workspace, StoreError> {
    let (canonical, canonical_str) = canonical_key(path)?;
    with_registration_lock(store, || {
        // Re-check under the lock: a concurrent registrant may have just
        // written the record, and we must adopt it rather than mint a new id.
        if let Some(w) = find_canonical(store, &canonical)? {
            return Ok(w);
        }
        let ws = Workspace {
            id: new_id(),
            path: canonical_str,
            repo_remote: detect_repo_remote(&canonical),
            initial_branch: detect_initial_branch(&canonical),
            created_at: Utc::now().to_rfc3339(),
            last_run_at: None,
        };
        store.write(&ws)?;
        Ok(ws)
    })
}

/// Look up an existing workspace for `path` (canonicalized) or create a
/// new one. Bumps `last_run_at` to "now" in either case.
///
/// On a new workspace, attempts to detect the git remote URL and the
/// current branch by shelling out to `git`. Failures are non-fatal —
/// the corresponding fields stay `None`.
pub fn upsert(store: &WorkspaceStore, path: &Path) -> Result<Workspace, StoreError> {
    let (canonical, canonical_str) = canonical_key(path)?;
    with_registration_lock(store, || {
        let now = Utc::now().to_rfc3339();
        // Re-check under the lock: a concurrent upsert may have just created
        // the record, and we must adopt its id rather than mint a new one.
        let ws = match find_canonical(store, &canonical)? {
            Some(mut w) => {
                w.last_run_at = Some(now);
                w
            }
            None => Workspace {
                id: new_id(),
                path: canonical_str,
                repo_remote: detect_repo_remote(&canonical),
                initial_branch: detect_initial_branch(&canonical),
                created_at: now.clone(),
                last_run_at: Some(now),
            },
        };
        store.write(&ws)?;
        Ok(ws)
    })
}

/// The `origin` remote URL of the git checkout at `path`, or `None` when
/// `path` is not a checkout or has no `origin`.
pub fn detect_repo_remote(path: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn detect_initial_branch(path: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}
