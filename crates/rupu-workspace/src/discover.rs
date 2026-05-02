//! Project discovery: walk up from `$PWD` to find the first `.rupu/`
//! directory. Implemented in Task 10 of Plan 1.

use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors from workspace discovery.
#[derive(Debug, Error)]
pub enum DiscoverError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Result of walking up from `pwd` looking for a `.rupu/` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    /// First ancestor of `canonical_pwd` that contains a `.rupu/` dir,
    /// or `None` if none was found (no project root).
    pub project_root: Option<PathBuf>,
    /// Canonicalized form of the input `pwd`.
    pub canonical_pwd: PathBuf,
}

/// Implemented in Task 10.
pub fn discover(_pwd: &Path) -> Result<Discovery, DiscoverError> {
    todo!("discover lands in Task 10")
}
