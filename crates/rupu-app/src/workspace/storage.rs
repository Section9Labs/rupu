//! Paths + manifest load/save for workspaces.
//!
//! macOS layout (via `directories::ProjectDirs`):
//!     ~/Library/Application Support/rupu.app/workspaces/<id>.toml
//!
//! The `ProjectDirs` qualifier triplet ("dev", "rupu", "rupu.app")
//! yields `rupu.app` as the leaf directory; this matches what every
//! macOS app bundle would create natively.

use crate::workspace::manifest::WorkspaceManifest;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::path::PathBuf;

/// `~/Library/Application Support/rupu.app/workspaces/`. Creates the
/// directory on first call so callers can write to it unconditionally.
pub fn workspaces_dir() -> Result<PathBuf> {
    let proj = ProjectDirs::from("dev", "rupu", "rupu.app")
        .context("could not resolve user app-support dir for rupu.app")?;
    let dir = proj.config_dir().join("workspaces");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

/// Returns `<cache>/rupu.app/clones/`. Created on first use.
pub fn clones_dir() -> Result<PathBuf> {
    let proj = ProjectDirs::from("dev", "rupu", "rupu.app")
        .ok_or_else(|| anyhow::anyhow!("no platform cache dir"))?;
    let dir = proj.cache_dir().join("clones");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Path to a specific workspace's manifest file.
pub fn manifest_path(workspace_id: &str) -> Result<PathBuf> {
    Ok(workspaces_dir()?.join(format!("{workspace_id}.toml")))
}

/// Load a manifest from disk. Missing file → error.
pub fn load(workspace_id: &str) -> Result<WorkspaceManifest> {
    let path = manifest_path(workspace_id)?;
    let bytes =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&bytes).with_context(|| format!("parse {}", path.display()))
}

/// Save a manifest to disk. Overwrites any existing file at the same
/// path. Atomic write: serialize to a tempfile then rename.
pub fn save(m: &WorkspaceManifest) -> Result<()> {
    let path = manifest_path(&m.id)?;
    let tmp = path.with_extension("toml.tmp");
    let body = toml::to_string(m).context("serialize manifest")?;
    std::fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("rename {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspaces_dir_is_under_app_support() {
        let dir = workspaces_dir().expect("xdg lookup");
        let s = dir.to_string_lossy();
        assert!(s.contains("rupu.app"), "{s} should contain 'rupu.app'");
        assert!(s.contains("workspaces"), "{s} should contain 'workspaces'");
    }

    #[test]
    fn manifest_path_uses_id() {
        let p = manifest_path("ws_01H8X").expect("xdg lookup");
        assert!(
            p.ends_with("ws_01H8X.toml"),
            "manifest path should be <id>.toml: {p:?}"
        );
    }
}
