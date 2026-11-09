//! `RepoLister` port — lists repos from the logged-in SCM accounts. rupu-cp
//! defines it; rupu-cli's `cp serve` provides the registry-backed adapter.
use serde::Serialize;
pub mod review_pages;

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
    #[error("review selection changed; refresh repository context")]
    SelectionChanged,
    #[error("failed to list repos: {0}")]
    Backend(String),
}

/// Captured read selection, never an account override. Resolution still follows
/// the owning checkout's configured rules before equality is checked.
pub struct ReviewSelection {
    pub account: String,
    pub branch: String,
    pub head: String,
    pub include_diff: bool,
}

#[async_trait::async_trait]
pub trait RepoLister: Send + Sync {
    fn forge_writer(&self) -> Option<&dyn crate::forge_writer::ForgeWriter> { None }
    fn issue_reader(&self) -> Option<&dyn crate::issue_reader::IssueReader> {
        None
    }
    async fn list(&self) -> Result<Vec<RepoEntry>, RepoListError>;

    fn supports_review_pages(&self) -> bool {
        false
    }

    async fn review_page_identity(
        &self,
        _repo: &rupu_scm::RepoRef,
        _cwd: &std::path::Path,
        _origin: &str,
        _number: u32,
        _selection: &ReviewSelection,
    ) -> Result<review_pages::Identity, RepoListError> {
        Err(RepoListError::Backend(
            "review continuation unavailable".into(),
        ))
    }

    async fn review_page(
        &self,
        _repo: &rupu_scm::RepoRef,
        _cwd: &std::path::Path,
        _origin: &str,
        _number: u32,
        _selection: &ReviewSelection,
        _expected: Option<&review_pages::Identity>,
        _request: &review_pages::Request,
    ) -> Result<review_pages::Page, RepoListError> {
        Err(RepoListError::Backend(
            "review continuation unavailable".into(),
        ))
    }

    /// Explicit read-only query through this CP's owning account-selection rules.
    /// Kept optional for older/read-only adapters; never infer authorization from
    /// the presence of account configuration or a local checkout.
    async fn review_detail(
        &self,
        _repo: &rupu_scm::RepoRef,
        _cwd: &std::path::Path,
        _origin_host: &str,
        _number: u32,
        _selection: &ReviewSelection,
    ) -> Result<serde_json::Value, RepoListError> {
        Err(RepoListError::Backend(
            "review detail adapter unavailable".into(),
        ))
    }

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
