//! Clone-to-dir helper. Shared between rupu-cli's `--tmp` flag
//! and rupu-app's Launcher target=Clone path.

use std::path::Path;

use crate::{AccountError, AccountId, Registry, RepoRef, ScmError};

#[derive(Debug, thiserror::Error)]
pub enum CloneError {
    /// Account resolution failed — no account configured for `r`'s
    /// platform, or several are configured and no rule/explicit arg
    /// disambiguated. `AccountError`'s own `Display` carries the fix
    /// hint (`rupu auth login ...` / `rupu scm bind ...`).
    #[error(transparent)]
    Account(#[from] AccountError),
    #[error("clone failed: {0}")]
    Scm(#[from] ScmError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// Resolve which account serves `r` via the account rule engine
/// (`Registry::repo_for` — spec §6.2/§6.3: explicit `--account` ->
/// owner rule -> path rule -> sole account -> error), then call its
/// `RepoConnector::clone_to(target_dir)`. The caller owns `target_dir`
/// (may be a `tempfile::TempDir` path or any other location). The
/// parent directory must exist; `clone_to` implementations are
/// expected to create `target_dir` itself.
///
/// Was a direct `registry.repo(r.platform)` call — the same
/// account-arbitrary (bare-vendor-name-else-lexicographically-first)
/// shim `repo_for` replaced everywhere else in Arc 2. This is a
/// `RepoRef` in hand (the owner is already known), so it is exactly
/// the "Targeted" shape spec §6.2's table describes — the same shape
/// `cmd/workflow.rs`'s sibling `RunTarget::Issue` arm already migrated
/// to `issues_for` in an earlier task; this closes the matching gap on
/// the `Repo`/`Pr` clone path, which called into `rupu-scm`'s own
/// internals rather than a `rupu-cli`-owned call site, and so was
/// invisible to every earlier task's shim grep (each scoped to the
/// crate being migrated, not `rupu-scm` itself).
///
/// [`RepoConnector`]: crate::RepoConnector
pub async fn clone_repo_ref(
    registry: &Registry,
    r: &RepoRef,
    target_dir: &Path,
    cwd: Option<&Path>,
    explicit: Option<&AccountId>,
) -> Result<(), CloneError> {
    let (_account, conn) = registry.repo_for(r, cwd, explicit)?;
    conn.clone_to(r, target_dir).await?;
    Ok(())
}
