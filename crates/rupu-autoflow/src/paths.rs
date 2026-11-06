//! Where autoflow state lives under `RUPU_HOME` (`<global>/autoflows/…`)
//! and the tracked-repo registry beside it.

use anyhow::Context as _;
use std::path::{Path, PathBuf};

/// The tracked-repo registry: `<global>/repos`.
pub fn repos_dir(global: &Path) -> PathBuf {
    global.join("repos")
}

/// The autoflow state root: `<global>/autoflows`.
pub fn autoflows_dir(global: &Path) -> PathBuf {
    global.join("autoflows")
}

/// Claims, one directory per issue.
pub fn autoflow_claims_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("claims")
}

/// Per-issue worktrees.
pub fn autoflow_worktrees_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("worktrees")
}

/// The worker registry.
pub fn autoflow_workers_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("workers")
}

/// Event-source cursors.
pub fn autoflow_event_cursors_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("event-cursors")
}

/// The wake queue root.
pub fn autoflow_wakes_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("wakes")
}

/// Cycle and event history.
pub fn autoflow_history_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("history")
}

/// Queued wake records.
pub fn autoflow_wake_queue_dir(global: &Path) -> PathBuf {
    autoflow_wakes_dir(global).join("queue")
}

/// Processed wake records.
pub fn autoflow_wake_processed_dir(global: &Path) -> PathBuf {
    autoflow_wakes_dir(global).join("processed")
}

/// Wake payloads.
pub fn autoflow_wake_payloads_dir(global: &Path) -> PathBuf {
    autoflow_wakes_dir(global).join("payloads")
}

/// Wake dedupe markers.
pub fn autoflow_wake_dedupe_dir(global: &Path) -> PathBuf {
    autoflow_wakes_dir(global).join("dedupe")
}

/// The project a directory belongs to ([`rupu_workspace::project_root_for`]),
/// the canonicalize failure naming the directory.
pub fn project_root_for(dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    rupu_workspace::project_root_for(dir).with_context(|| format!("canonicalize {}", dir.display()))
}
