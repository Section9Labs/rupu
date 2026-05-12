//! Recent-workspaces listing.
//!
//! Walks `storage::workspaces_dir()`, parses each `*.toml`, sorts by
//! `opened_at` descending. Errors on individual files are logged and
//! skipped — a corrupt manifest shouldn't block the list of valid ones.

use crate::workspace::{manifest::WorkspaceManifest, storage};
use anyhow::Result;
use std::cmp::Reverse;

/// All workspaces with valid manifests, newest-opened first.
pub fn list() -> Result<Vec<WorkspaceManifest>> {
    let dir = storage::workspaces_dir()?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(_) => return Ok(Vec::new()),
    };

    let mut out: Vec<WorkspaceManifest> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("toml"))
        .filter_map(|p| {
            let bytes = std::fs::read_to_string(&p).ok()?;
            match toml::from_str::<WorkspaceManifest>(&bytes) {
                Ok(m) => Some(m),
                Err(e) => {
                    tracing::warn!(path = %p.display(), error = %e, "skip unreadable manifest");
                    None
                }
            }
        })
        .collect();

    out.sort_by_key(|m| Reverse(m.opened_at));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::manifest::{AttachedHost, UiState, WorkspaceColor, WorkspaceManifest};
    use chrono::TimeZone;
    use serial_test::serial;
    use std::env;
    use tempfile::TempDir;

    fn sandbox() -> TempDir {
        let tmp = TempDir::new().unwrap();
        env::set_var("HOME", tmp.path());
        env::set_var("XDG_CONFIG_HOME", tmp.path().join(".config"));
        tmp
    }

    fn mk(id: &str, opened_at_secs: i64) -> WorkspaceManifest {
        WorkspaceManifest {
            id: id.into(),
            name: id.into(),
            color: WorkspaceColor::Purple,
            path: format!("/tmp/{id}"),
            opened_at: chrono::Utc.timestamp_opt(opened_at_secs, 0).unwrap(),
            repos: vec![],
            attached_hosts: vec![AttachedHost::Local],
            ui: UiState::default(),
        }
    }

    #[test]
    #[serial]
    fn list_returns_newest_first() {
        let _tmp = sandbox();
        storage::save(&mk("ws_a", 1_000)).unwrap();
        storage::save(&mk("ws_b", 3_000)).unwrap();
        storage::save(&mk("ws_c", 2_000)).unwrap();

        let recents = list().expect("list");
        assert_eq!(recents.len(), 3);
        assert_eq!(recents[0].id, "ws_b");
        assert_eq!(recents[1].id, "ws_c");
        assert_eq!(recents[2].id, "ws_a");
    }

    #[test]
    #[serial]
    fn list_skips_non_toml_files() {
        let _tmp = sandbox();
        storage::save(&mk("ws_real", 1)).unwrap();
        let dir = storage::workspaces_dir().unwrap();
        std::fs::write(dir.join("garbage.txt"), "not a manifest").unwrap();
        std::fs::write(dir.join(".DS_Store"), "macos junk").unwrap();

        let recents = list().expect("list");
        assert_eq!(recents.len(), 1);
        assert_eq!(recents[0].id, "ws_real");
    }
}
