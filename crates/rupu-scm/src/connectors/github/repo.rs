//! GitHub RepoConnector. Implementation lands in Task 12.

use async_trait::async_trait;

use crate::connectors::RepoConnector;
use crate::error::ScmError;
use crate::platform::Platform;
use crate::types::{
    Branch, Comment, CreatePr, Diff, FileContent, Pr, PrFilter, PrRef, Repo, RepoRef,
};

use super::client::GithubClient;

pub struct GithubRepoConnector {
    client: GithubClient,
}

impl GithubRepoConnector {
    pub fn new(client: GithubClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl RepoConnector for GithubRepoConnector {
    fn platform(&self) -> Platform {
        Platform::Github
    }

    async fn list_repos(&self) -> Result<Vec<Repo>, ScmError> {
        let _ = &self.client;
        Err(ScmError::Transient(anyhow::anyhow!(
            "github::list_repos not yet implemented (Task 12)"
        )))
    }

    async fn get_repo(&self, _r: &RepoRef) -> Result<Repo, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn list_branches(&self, _r: &RepoRef) -> Result<Vec<Branch>, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn create_branch(
        &self,
        _r: &RepoRef,
        _name: &str,
        _from_sha: &str,
    ) -> Result<Branch, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn read_file(
        &self,
        _r: &RepoRef,
        _path: &str,
        _ref_: Option<&str>,
    ) -> Result<FileContent, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn list_prs(&self, _r: &RepoRef, _filter: PrFilter) -> Result<Vec<Pr>, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn get_pr(&self, _p: &PrRef) -> Result<Pr, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn diff_pr(&self, _p: &PrRef) -> Result<Diff, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn comment_pr(&self, _p: &PrRef, _body: &str) -> Result<Comment, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn create_pr(&self, _r: &RepoRef, _opts: CreatePr) -> Result<Pr, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn clone_to(&self, _r: &RepoRef, _dir: &std::path::Path) -> Result<(), ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }
}
