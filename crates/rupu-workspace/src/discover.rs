//! Project discovery: walk up from `$PWD` looking for the first `.rupu/`
//! directory (mirrors how `git` finds `.git`).

use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors from workspace discovery.
#[derive(Debug, Error)]
pub enum DiscoverError {
    #[error("io canonicalizing {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// Result of walking up from `pwd` looking for a `.rupu/` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    /// First ancestor of `canonical_pwd` (inclusive) that contains a
    /// `.rupu/` directory, or `None` if none was found (no project).
    pub project_root: Option<PathBuf>,
    /// Canonicalized form of the input `pwd` (symlinks resolved).
    pub canonical_pwd: PathBuf,
}

/// Walk up from `pwd` looking for the first `.rupu/` directory. Returns
/// the canonicalized `pwd` and (if found) the project root.
///
/// The walk is inclusive of `pwd` itself: if `pwd` contains a `.rupu/`
/// dir, `pwd` is the project root.
///
/// Errors only on `pwd.canonicalize()` failure — typically because
/// `pwd` does not exist or is not accessible.
pub fn discover(pwd: &Path) -> Result<Discovery, DiscoverError> {
    let canonical_pwd = pwd.canonicalize().map_err(|e| DiscoverError::Io {
        path: pwd.display().to_string(),
        source: e,
    })?;

    let mut cursor: Option<&Path> = Some(&canonical_pwd);
    while let Some(dir) = cursor {
        if dir.join(".rupu").is_dir() {
            return Ok(Discovery {
                project_root: Some(dir.to_path_buf()),
                canonical_pwd: canonical_pwd.clone(),
            });
        }
        cursor = dir.parent();
    }

    Ok(Discovery {
        project_root: None,
        canonical_pwd,
    })
}
