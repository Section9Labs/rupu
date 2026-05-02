//! Workspace record store. Lives at `~/.rupu/workspaces/`. Implemented
//! in Task 11 of Plan 1.
//!
//! NOTE FOR TASK 11 IMPLEMENTER:
//! - The `StoreError` variants below are placeholder shapes (`#[from]`
//!   tuple variants). Task 11 restructures them to struct variants
//!   carrying path/action context — see the layered-fix pattern in
//!   `rupu-config::layer::LayerError`. This is a deliberate breaking
//!   change at the type level; nothing else in the workspace yet
//!   matches on `StoreError`.
//! - When converting the canonicalized path to a string for the
//!   `Workspace.path` field, prefer `path.to_str().ok_or(...)` (or
//!   `path.to_string_lossy().into_owned()` with an explicit comment)
//!   over `path.display().to_string()`. The latter inserts replacement
//!   characters on non-UTF-8 paths and would make `upsert`'s
//!   "same-path-already-recorded" lookup miss.

use crate::Workspace;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors from the workspace record store.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("serialize: {0}")]
    Ser(#[from] toml::ser::Error),
}

/// Handle to the on-disk workspace store directory.
#[derive(Debug, Clone)]
pub struct WorkspaceStore {
    /// Root directory of the store (typically `~/.rupu/workspaces/`).
    pub root: PathBuf,
}

/// Implemented in Task 11.
pub fn upsert(_store: &WorkspaceStore, _path: &Path) -> Result<Workspace, StoreError> {
    todo!("upsert lands in Task 11")
}
