//! `~/.rupu/` resolution + project `.rupu/` discovery.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

/// Resolve the global rupu directory. Honors `$RUPU_HOME` if set
/// (used by tests + by users who want a non-default location);
/// otherwise falls back to `~/.rupu/`.
pub fn global_dir() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("RUPU_HOME") {
        return Ok(PathBuf::from(p));
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow!("could not locate home directory"))?;
    Ok(home.join(".rupu"))
}

/// Walk up from `pwd` looking for the first `.rupu/` directory. Returns
/// `Some(path)` of the directory containing it, or `None` if not found.
pub fn project_root_for(pwd: &Path) -> Result<Option<PathBuf>> {
    let canonical = pwd
        .canonicalize()
        .with_context(|| format!("canonicalize {}", pwd.display()))?;
    let mut cursor: Option<&Path> = Some(&canonical);
    while let Some(dir) = cursor {
        if dir.join(".rupu").is_dir() {
            return Ok(Some(dir.to_path_buf()));
        }
        cursor = dir.parent();
    }
    Ok(None)
}

/// Pick the transcripts directory. Project-local when
/// `<project>/.rupu/transcripts/` exists; global default otherwise.
pub fn transcripts_dir(global: &Path, project_root: Option<&Path>) -> PathBuf {
    if let Some(p) = project_root {
        let local = p.join(".rupu/transcripts");
        if local.is_dir() {
            return local;
        }
    }
    global.join("transcripts")
}

/// Global repo registry directory.
pub fn repos_dir(global: &Path) -> PathBuf {
    global.join("repos")
}

/// Global autoflow state root.
pub fn autoflows_dir(global: &Path) -> PathBuf {
    global.join("autoflows")
}

/// Global autoflow claims directory.
pub fn autoflow_claims_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("claims")
}

/// Global autoflow worktrees directory.
pub fn autoflow_worktrees_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("worktrees")
}

/// Global autoflow event cursor directory.
pub fn autoflow_event_cursors_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("event-cursors")
}

/// Global autoflow wake queue root.
pub fn autoflow_wakes_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("wakes")
}

/// Global queued autoflow wake-record directory.
pub fn autoflow_wake_queue_dir(global: &Path) -> PathBuf {
    autoflow_wakes_dir(global).join("queue")
}

/// Global processed autoflow wake-record directory.
pub fn autoflow_wake_processed_dir(global: &Path) -> PathBuf {
    autoflow_wakes_dir(global).join("processed")
}

/// Global autoflow wake payload directory.
pub fn autoflow_wake_payloads_dir(global: &Path) -> PathBuf {
    autoflow_wakes_dir(global).join("payloads")
}

/// Global autoflow wake dedupe marker directory.
pub fn autoflow_wake_dedupe_dir(global: &Path) -> PathBuf {
    autoflow_wakes_dir(global).join("dedupe")
}

/// Convenience: ensure a directory exists. Used to lazily create
/// `~/.rupu/cache/`, `~/.rupu/transcripts/`, etc. on first use.
pub fn ensure_dir(p: &Path) -> Result<()> {
    std::fs::create_dir_all(p).with_context(|| format!("create_dir_all {}", p.display()))?;
    Ok(())
}
