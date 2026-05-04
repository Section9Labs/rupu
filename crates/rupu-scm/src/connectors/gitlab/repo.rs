//! GitlabRepoConnector. Implementation lands in Plan 2 Tasks 4 + 7.

use std::path::Path;

use async_trait::async_trait;

use crate::connectors::RepoConnector;
use crate::error::ScmError;
use crate::platform::Platform;
use crate::types::{
    Branch, Comment, CreatePr, Diff, FileContent, Pr, PrFilter, PrRef, Repo, RepoRef,
};

use super::client::GitlabClient;

#[allow(dead_code)]
pub struct GitlabRepoConnector {
    client: GitlabClient,
}

impl GitlabRepoConnector {
    pub fn new(client: GitlabClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl RepoConnector for GitlabRepoConnector {
    fn platform(&self) -> Platform {
        Platform::Gitlab
    }

    async fn list_repos(&self) -> Result<Vec<Repo>, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!(
            "not yet implemented (Task 4)"
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
    async fn clone_to(&self, _r: &RepoRef, _dir: &Path) -> Result<(), ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }
}
