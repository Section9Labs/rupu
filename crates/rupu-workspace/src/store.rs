//! Workspace record store. Lives at `~/.rupu/workspaces/`. Implemented
//! in Task 11 of Plan 1.

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
