//! Workspace record store. Lives at `~/.rupu/workspaces/`.
//!
//! Records are keyed by canonicalized path; on `upsert` we read every
//! record in the store dir and reuse the matching one rather than
//! generating a new id. New records auto-detect the git remote and
//! default branch (snapshot at workspace-creation time only — they are
//! not refreshed on subsequent runs).

use crate::record::{new_id, Workspace};
use chrono::Utc;
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
        Ok(out)
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
}

/// Look up an existing workspace for `path` (canonicalized) or create a
/// new one. Bumps `last_run_at` to "now" in either case.
///
/// On a new workspace, attempts to detect the git remote URL and the
/// current branch by shelling out to `git`. Failures are non-fatal —
/// the corresponding fields stay `None`.
pub fn upsert(store: &WorkspaceStore, path: &Path) -> Result<Workspace, StoreError> {
    let (canonical, canonical_str) = canonical_key(path)?;
    let now = Utc::now().to_rfc3339();
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
