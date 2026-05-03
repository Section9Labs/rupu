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
        let _permit = self.client.permit().await;
        let inner = self.client.inner.clone();
        self.client
            .with_retry(|| {
                let inner = inner.clone();
                async move {
                    let pages = inner
                        .current()
                        .list_repos_for_authenticated_user()
                        .per_page(100)
                        .send()
                        .await
                        .map_err(super::client::classify_octocrab_error)?;
                    let all = inner
                        .all_pages(pages)
                        .await
                        .map_err(super::client::classify_octocrab_error)?;
                    Ok(all
                        .into_iter()
                        .filter_map(repo_from_octocrab)
                        .collect::<Vec<_>>())
                }
            })
            .await
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

fn repo_from_octocrab(r: octocrab::models::Repository) -> Option<Repo> {
    let full = r.full_name?;
    let (owner, name) = full.split_once('/')?;
    Some(Repo {
        r: RepoRef {
            platform: Platform::Github,
            owner: owner.to_string(),
            repo: name.to_string(),
        },
        default_branch: r.default_branch.unwrap_or_else(|| "main".into()),
        clone_url_https: r.clone_url.map(|u| u.to_string()).unwrap_or_default(),
        clone_url_ssh: r.ssh_url.unwrap_or_default(),
        private: r.private.unwrap_or(false),
        description: r.description,
    })
}
