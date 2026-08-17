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

/// Pick the netflow directory. Project-local when
/// `<project>/.rupu/netflow/` exists; global default otherwise.
///
/// Deliberately the same shape as [`transcripts_dir`] — a netflow ledger
/// has the same lifecycle as a transcript and the two resolutions must not
/// drift. The existence check is load-bearing: a repo that was never
/// `rupu init`'d falls back to global, so no ledger is ever written inside
/// a project that has not opted in.
pub fn netflow_dir(global: &Path, project_root: Option<&Path>) -> PathBuf {
    if let Some(p) = project_root {
        let local = p.join(".rupu/netflow");
        if local.is_dir() {
            return local;
        }
    }
    global.join("netflow")
}

/// Global repo registry directory.
pub fn repos_dir(global: &Path) -> PathBuf {
    global.join("repos")
}

/// Global session state root.
pub fn sessions_dir(global: &Path) -> PathBuf {
    global.join("sessions")
}

/// Global archived session state root.
pub fn archived_sessions_dir(global: &Path) -> PathBuf {
    global.join("sessions-archive")
}

/// Archive directory nested under a transcript root.
pub fn archived_transcripts_dir(transcripts_dir: &Path) -> PathBuf {
    transcripts_dir.join("archive")
}

/// Archive directory nested under a netflow root.
pub fn archived_netflow_dir(netflow_dir: &Path) -> PathBuf {
    netflow_dir.join("archive")
}

/// Global UI theme directory.
pub fn themes_dir(global: &Path) -> PathBuf {
    global.join("themes")
}

/// Project-local UI theme directory.
pub fn project_themes_dir(project_root: &Path) -> PathBuf {
    project_root.join(".rupu/themes")
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

/// Global autoflow worker registry directory.
pub fn autoflow_workers_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("workers")
}

/// Global autoflow event cursor directory.
pub fn autoflow_event_cursors_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("event-cursors")
}

/// Global autoflow wake queue root.
pub fn autoflow_wakes_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("wakes")
}

/// Global autoflow cycle/event history root.
pub fn autoflow_history_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("history")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netflow_dir_prefers_an_existing_project_local_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let global = tmp.path().join("global");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join(".rupu/netflow")).unwrap();

        assert_eq!(
            netflow_dir(&global, Some(&project)),
            project.join(".rupu/netflow")
        );
    }

    #[test]
    fn netflow_dir_falls_back_to_global_when_the_project_dir_does_not_exist() {
        // Load-bearing: a repo that was never `rupu init`'d must never get a
        // ledger written inside it. This is what closes the git-leak class
        // structurally rather than by patching ensure_dir.
        let tmp = tempfile::TempDir::new().unwrap();
        let global = tmp.path().join("global");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        assert_eq!(netflow_dir(&global, Some(&project)), global.join("netflow"));
    }

    #[test]
    fn netflow_dir_falls_back_to_global_with_no_project_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let global = tmp.path().join("global");
        assert_eq!(netflow_dir(&global, None), global.join("netflow"));
    }

    #[test]
    fn archived_netflow_dir_nests_under_the_netflow_root() {
        let dir = std::path::Path::new("/tmp/x/netflow");
        assert_eq!(archived_netflow_dir(dir), dir.join("archive"));
    }
}
