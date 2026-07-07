//! RepoConnector + IssueConnector trait families.
//!
//! Each platform implements one or both. Trait objects (`Arc<dyn ...>`)
//! live behind [`crate::Registry`].

pub mod github;
pub mod gitlab;
pub mod jira;
pub mod linear;

use std::path::Path;

use async_trait::async_trait;

use crate::error::ScmError;
use crate::platform::{IssueTracker, Platform};
use crate::types::{
    Branch, Comment, CreateIssue, CreatePr, Diff, FileContent, Issue, IssueFilter, IssueRef,
    IssueState, Pr, PrFilter, PrRef, Repo, RepoRef,
};

#[async_trait]
pub trait RepoConnector: Send + Sync {
    fn platform(&self) -> Platform;

    async fn list_repos(&self) -> Result<Vec<Repo>, ScmError>;
    async fn get_repo(&self, r: &RepoRef) -> Result<Repo, ScmError>;
    async fn list_branches(&self, r: &RepoRef) -> Result<Vec<Branch>, ScmError>;
    async fn create_branch(
        &self,
        r: &RepoRef,
        name: &str,
        from_sha: &str,
    ) -> Result<Branch, ScmError>;
    async fn read_file(
        &self,
        r: &RepoRef,
        path: &str,
        ref_: Option<&str>,
    ) -> Result<FileContent, ScmError>;
    async fn list_prs(&self, r: &RepoRef, filter: PrFilter) -> Result<Vec<Pr>, ScmError>;
    async fn get_pr(&self, p: &PrRef) -> Result<Pr, ScmError>;
    async fn diff_pr(&self, p: &PrRef) -> Result<Diff, ScmError>;
    async fn comment_pr(&self, p: &PrRef, body: &str) -> Result<Comment, ScmError>;
    async fn create_pr(&self, r: &RepoRef, opts: CreatePr) -> Result<Pr, ScmError>;
    /// Clone the repo to a local directory using the platform's
    /// HTTPS clone URL with the connector's stored credential.
    async fn clone_to(&self, r: &RepoRef, dir: &Path) -> Result<(), ScmError>;

    /// Is `login` a collaborator on `r`? Backs the autoflow
    /// author-allowlist (dogfood autoflows spec): a workflow trigger
    /// fired by a PR/issue author who isn't a collaborator is dropped
    /// rather than run with elevated trust.
    ///
    /// Default is unimplemented — `ScmError` has no dedicated
    /// "unsupported operation" variant, so this returns the closest
    /// existing one (`BadRequest`, which is non-recoverable and won't
    /// get silently retried). Only GitHub implements this for now;
    /// platforms without an override fail closed rather than allowing
    /// an unverified author through.
    async fn is_collaborator(&self, r: &RepoRef, login: &str) -> Result<bool, ScmError> {
        let _ = (r, login);
        Err(ScmError::BadRequest {
            message: format!("is_collaborator is not supported by {}", self.platform()),
        })
    }

    /// Add labels to a pull request. Backs the autoflow author-allowlist
    /// `on_skip: label_needs_human` action, which flags a PR from a
    /// non-collaborator for human attention rather than running an agent
    /// on it. Default is unimplemented (returns the closest existing
    /// `ScmError`); only platforms that override it can label. Callers
    /// treat an `Err` here as a best-effort miss (log + still skip the
    /// PR) — labeling never gates the safety skip.
    async fn add_pr_labels(&self, p: &PrRef, labels: &[String]) -> Result<(), ScmError> {
        let _ = (p, labels);
        Err(ScmError::BadRequest {
            message: format!("add_pr_labels is not supported by {}", self.platform()),
        })
    }
}

#[async_trait]
pub trait IssueConnector: Send + Sync {
    fn tracker(&self) -> IssueTracker;

    async fn list_issues(&self, project: &str, filter: IssueFilter)
        -> Result<Vec<Issue>, ScmError>;
    async fn get_issue(&self, i: &IssueRef) -> Result<Issue, ScmError>;
    async fn comment_issue(&self, i: &IssueRef, body: &str) -> Result<Comment, ScmError>;
    async fn create_issue(&self, project: &str, opts: CreateIssue) -> Result<Issue, ScmError>;
    async fn update_issue_state(&self, i: &IssueRef, state: IssueState) -> Result<(), ScmError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Sanity check: traits are object-safe (i.e., can be used as
    // `Arc<dyn RepoConnector>`). The Registry depends on this.
    fn _assert_object_safe() {
        let _: Option<std::sync::Arc<dyn RepoConnector>> = None;
        let _: Option<std::sync::Arc<dyn IssueConnector>> = None;
    }
}
