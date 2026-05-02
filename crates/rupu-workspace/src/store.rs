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

    fn list(&self) -> Result<Vec<Workspace>, StoreError> {
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

    /// Write the record atomically: serialize to `<id>.toml.tmp`, then
    /// rename. POSIX rename is atomic; readers never see a partial file.
    fn write(&self, ws: &Workspace) -> Result<(), StoreError> {
        self.ensure_root()?;
        let body = toml::to_string(ws)?;
        let path = self.record_path(&ws.id);
        let tmp_path = path.with_extension("toml.tmp");
        std::fs::write(&tmp_path, body).map_err(|e| StoreError::Io {
            action: format!("write {}", tmp_path.display()),
            source: e,
        })?;
        std::fs::rename(&tmp_path, &path).map_err(|e| StoreError::Io {
            action: format!("rename {} -> {}", tmp_path.display(), path.display()),
            source: e,
        })?;
        Ok(())
    }
}

/// Look up an existing workspace for `path` (canonicalized) or create a
/// new one. Bumps `last_run_at` to "now" in either case.
///
/// On a new workspace, attempts to detect the git remote URL and the
/// current branch by shelling out to `git`. Failures are non-fatal —
/// the corresponding fields stay `None`.
pub fn upsert(store: &WorkspaceStore, path: &Path) -> Result<Workspace, StoreError> {
    let canonical = path.canonicalize().map_err(|e| StoreError::Io {
        action: format!("canonicalize {}", path.display()),
        source: e,
    })?;
    // Use to_str() to avoid display()'s lossy replacement chars on
    // non-UTF-8 paths. The path is the lookup key for "same workspace
    // already recorded" — a mangled path here would create a duplicate
    // record on every run.
    let canonical_str = canonical
        .to_str()
        .ok_or_else(|| StoreError::NonUtf8Path {
            path: canonical.display().to_string(),
        })?
        .to_string();

    let now = Utc::now().to_rfc3339();
    let existing = store.list()?.into_iter().find(|w| {
        Path::new(&w.path)
            .canonicalize()
            .map(|p| p == canonical)
            .unwrap_or(false)
    });

    let ws = match existing {
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

fn detect_repo_remote(path: &Path) -> Option<String> {
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
