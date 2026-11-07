//! `RepoLister` port — lists repos from the logged-in SCM accounts. rupu-cp
//! defines it; rupu-cli's `cp serve` provides the registry-backed adapter.
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct RepoEntry {
    /// Platform id, e.g. "github" | "gitlab".
    pub platform: String,
    /// Which configured account this repo came from — the multi-account
    /// arc's fan-out tag (spec §6.2's "Account-scoped, no repo" row: a
    /// union across every account of a kind, tagged so it's legible).
    /// Pre-Arc-2 configs have exactly one account per kind, named after
    /// the platform itself (e.g. `"github"`), so this is never blank.
    pub account: String,
    /// "owner/name".
    pub repo: String,
    pub default_branch: String,
    pub private: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum RepoListError {
    #[error("failed to list repos: {0}")]
    Backend(String),
}

#[async_trait::async_trait]
pub trait RepoLister: Send + Sync {
    async fn list(&self) -> Result<Vec<RepoEntry>, RepoListError>;

    /// Explicit read-only query through this CP's owning account-selection rules.
    /// Kept optional for older/read-only adapters; never infer authorization from
    /// the presence of account configuration or a local checkout.
    async fn reviews(
        &self,
        _repo: &rupu_scm::RepoRef,
        _cwd: &std::path::Path,
        _branch: &str,
        _origin_host: &str,
    ) -> Result<serde_json::Value, RepoListError> {
        Err(RepoListError::Backend("review adapter unavailable".into()))
    }
}
