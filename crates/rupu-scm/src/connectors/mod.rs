//! RepoConnector + IssueConnector trait families.
//!
//! Each platform implements one or both. Trait objects (`Arc<dyn ...>`)
//! live behind [`crate::Registry`].

pub mod github;
pub mod gitlab;
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
