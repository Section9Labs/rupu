//! `Workspace` handle — the runtime object the GPUI window layers
//! consume. Wraps a `WorkspaceManifest` with its discovered project
//! + global asset sets. Constructing a handle is a fallible IO
//!   operation (manifest load/create + asset walk); the GPUI window
//!   constructor calls `open` and bails on error.

use crate::workspace::{
    discovery::{self, AssetSet},
    manifest::{AttachedHost, UiState, WorkspaceColor, WorkspaceManifest},
    recents, storage,
};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Workspace {
    pub manifest: WorkspaceManifest,
    pub project_assets: AssetSet,
    pub global_assets: AssetSet,
}

impl Workspace {
    /// Open a workspace for a given project directory. If a manifest
    /// already exists for this path (in `recents()`), reuse its id and
    /// settings; otherwise create a fresh manifest with default name
    /// (directory basename) and default color (Purple).
    ///
    /// Either way the `opened_at` timestamp is bumped to `now()` and
    /// the manifest is persisted before returning.
    pub fn open(project_dir: &Path) -> Result<Self> {
        let absolute = project_dir
            .canonicalize()
            .with_context(|| format!("canonicalize {}", project_dir.display()))?;

        let mut manifest = match find_existing(&absolute)? {
            Some(m) => m,
            None => fresh_manifest(&absolute),
        };
        manifest.opened_at = chrono::Utc::now();
        storage::save(&manifest).context("persist manifest on open")?;

        let project_assets = discovery::discover_project(&absolute);
        let global_assets = discovery::discover_global();

        Ok(Workspace {
            manifest,
            project_assets,
            global_assets,
        })
    }
}

/// Look through the recents list for a manifest whose path canonicalizes
/// to the requested directory. Returns the first match, if any.
fn find_existing(absolute_path: &Path) -> Result<Option<WorkspaceManifest>> {
    let want = absolute_path.to_string_lossy();
    for m in recents::list()? {
        if PathBuf::from(&m.path)
            .canonicalize()
            .map(|p| p.to_string_lossy() == want)
            .unwrap_or(false)
        {
            return Ok(Some(m));
        }
    }
    Ok(None)
}

/// Build a default-shaped manifest for a previously-unseen directory.
fn fresh_manifest(absolute_path: &Path) -> WorkspaceManifest {
    let name = absolute_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("workspace")
        .to_string();
    WorkspaceManifest {
        id: format!("ws_{}", ulid::Ulid::new()),
        name,
        color: WorkspaceColor::Purple,
        path: absolute_path.to_string_lossy().to_string(),
        opened_at: chrono::Utc::now(),
        repos: vec![],
        attached_hosts: vec![AttachedHost::Local],
        ui: UiState::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;
    use tempfile::TempDir;

    fn sandbox() -> TempDir {
        let tmp = TempDir::new().unwrap();
        env::set_var("HOME", tmp.path());
        env::set_var("XDG_CONFIG_HOME", tmp.path().join(".config"));
        tmp
    }

    #[test]
    #[serial]
    fn open_directory_creates_manifest_on_first_open() {
        let _home = sandbox();
        let project = TempDir::new().unwrap();
        let ws = Workspace::open(project.path()).expect("open");

        assert!(ws.manifest.id.starts_with("ws_"));
        assert_eq!(
            ws.manifest.path,
            project.path().canonicalize().unwrap().to_string_lossy().to_string()
        );
        assert_eq!(ws.manifest.color, WorkspaceColor::Purple);
        let basename = project
            .path()
            .canonicalize()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(ws.manifest.name, basename);

        let loaded = storage::load(&ws.manifest.id).expect("load");
        assert_eq!(loaded.id, ws.manifest.id);
    }

    #[test]
    #[serial]
    fn reopen_reuses_existing_manifest() {
        let _home = sandbox();
        let project = TempDir::new().unwrap();

        let first = Workspace::open(project.path()).expect("first open");
        let second = Workspace::open(project.path()).expect("second open");

        assert_eq!(first.manifest.id, second.manifest.id);
    }
}
