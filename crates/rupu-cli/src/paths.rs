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

/// Convenience: ensure a directory exists. Used to lazily create
/// `~/.rupu/cache/`, `~/.rupu/transcripts/`, etc. on first use.
pub fn ensure_dir(p: &Path) -> Result<()> {
    std::fs::create_dir_all(p).with_context(|| format!("create_dir_all {}", p.display()))?;
    Ok(())
}
