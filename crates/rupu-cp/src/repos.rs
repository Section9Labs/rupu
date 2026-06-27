//! `RepoLister` port — lists repos from the logged-in SCM accounts. rupu-cp
//! defines it; rupu-cli's `cp serve` provides the registry-backed adapter.
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct RepoEntry {
    /// Platform id, e.g. "github" | "gitlab".
    pub platform: String,
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
}
