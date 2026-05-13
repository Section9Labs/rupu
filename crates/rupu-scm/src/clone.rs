//! Clone-to-dir helper. Shared between rupu-cli's `--tmp` flag
//! and rupu-app's Launcher target=Clone path.

use std::path::Path;

use crate::{Platform, RepoRef, Registry, ScmError};

#[derive(Debug, thiserror::Error)]
pub enum CloneError {
    #[error("no {0} credential — run `rupu auth login --provider {0}`")]
    MissingConnector(Platform),
    #[error("clone failed: {0}")]
    Scm(#[from] ScmError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// Look up the [`RepoConnector`] for `r.platform`, then call its
/// `clone_to(target_dir)`. The caller owns `target_dir` (may be a
/// `tempfile::TempDir` path or any other location). The parent
/// directory must exist; `clone_to` implementations are expected to
/// create `target_dir` itself.
///
/// [`RepoConnector`]: crate::RepoConnector
pub async fn clone_repo_ref(
    registry: &Registry,
    r: &RepoRef,
    target_dir: &Path,
) -> Result<(), CloneError> {
    let conn = registry
        .repo(r.platform)
        .ok_or(CloneError::MissingConnector(r.platform))?;
    conn.clone_to(r, target_dir).await?;
    Ok(())
}
