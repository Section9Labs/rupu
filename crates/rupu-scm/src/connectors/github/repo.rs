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

    async fn get_repo(&self, r: &RepoRef) -> Result<Repo, ScmError> {
        let _permit = self.client.permit().await;
        let inner = self.client.inner.clone();
        let owner = r.owner.clone();
        let repo = r.repo.clone();
        let model = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                async move {
                    inner
                        .repos(&owner, &repo)
                        .get()
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        repo_from_octocrab(model).ok_or_else(|| {
            ScmError::Transient(anyhow::anyhow!("malformed repo response from github"))
        })
    }

    async fn list_branches(&self, r: &RepoRef) -> Result<Vec<Branch>, ScmError> {
        let _permit = self.client.permit().await;
        let inner = self.client.inner.clone();
        let owner = r.owner.clone();
        let repo = r.repo.clone();
        self.client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                async move {
                    let pages = inner
                        .repos(&owner, &repo)
                        .list_branches()
                        .send()
                        .await
                        .map_err(super::client::classify_octocrab_error)?;
                    let all = inner
                        .all_pages(pages)
                        .await
                        .map_err(super::client::classify_octocrab_error)?;
                    Ok(all
                        .into_iter()
                        .map(|b| Branch {
                            name: b.name,
                            sha: b.commit.sha,
                            protected: b.protected,
                        })
                        .collect::<Vec<_>>())
                }
            })
            .await
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
        r: &RepoRef,
        path: &str,
        ref_: Option<&str>,
    ) -> Result<FileContent, ScmError> {
        let _permit = self.client.permit().await;
        let inner = self.client.inner.clone();
        let owner = r.owner.clone();
        let repo = r.repo.clone();
        let path_owned = path.to_string();
        let ref_owned = ref_.map(|s| s.to_string());
        let mut items = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                let path = path_owned.clone();
                let ref_ = ref_owned.clone();
                async move {
                    let handler = inner.repos(&owner, &repo);
                    let mut builder = handler.get_content().path(path);
                    if let Some(r) = ref_ {
                        builder = builder.r#ref(r);
                    }
                    builder
                        .send()
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        let first = items
            .items
            .pop()
            .ok_or_else(|| ScmError::NotFound { what: path.into() })?;
        let content = first.decoded_content().ok_or_else(|| {
            ScmError::Transient(anyhow::anyhow!("github: content not decodable for {path}"))
        })?;
        Ok(FileContent {
            path: first.path,
            ref_: ref_.unwrap_or("HEAD").to_string(),
            content,
            encoding: crate::types::FileEncoding::Utf8,
        })
    }

    async fn list_prs(&self, r: &RepoRef, filter: PrFilter) -> Result<Vec<Pr>, ScmError> {
        let _permit = self.client.permit().await;
        let inner = self.client.inner.clone();
        let owner = r.owner.clone();
        let repo = r.repo.clone();
        let repo_ref = r.clone();
        let state = filter.state;
        self.client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                let repo_ref = repo_ref.clone();
                async move {
                    let pulls_handler = inner.pulls(&owner, &repo);
                    let mut req = pulls_handler.list();
                    if let Some(s) = state {
                        req = req.state(match s {
                            crate::types::PrState::Open => octocrab::params::State::Open,
                            crate::types::PrState::Closed | crate::types::PrState::Merged => {
                                octocrab::params::State::Closed
                            }
                        });
                    }
                    let pages = req
                        .send()
                        .await
                        .map_err(super::client::classify_octocrab_error)?;
                    let all = inner
                        .all_pages(pages)
                        .await
                        .map_err(super::client::classify_octocrab_error)?;
                    Ok(all
                        .into_iter()
                        .map(|p| pr_from_octocrab(repo_ref.clone(), p))
                        .collect::<Vec<_>>())
                }
            })
            .await
    }

    async fn get_pr(&self, p: &PrRef) -> Result<Pr, ScmError> {
        let _permit = self.client.permit().await;
        let inner = self.client.inner.clone();
        let owner = p.repo.owner.clone();
        let repo = p.repo.repo.clone();
        let number = p.number;
        let pr = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                async move {
                    inner
                        .pulls(&owner, &repo)
                        .get(number as u64)
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(pr_from_octocrab(p.repo.clone(), pr))
    }

    async fn diff_pr(&self, p: &PrRef) -> Result<Diff, ScmError> {
        let _permit = self.client.permit().await;
        let path = format!("/repos/{}/{}/pulls/{}", p.repo.owner, p.repo.repo, p.number);
        let inner = self.client.inner.clone();
        let patch = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let path = path.clone();
                async move {
                    let mut headers = http::header::HeaderMap::new();
                    headers.insert(
                        http::header::ACCEPT,
                        http::header::HeaderValue::from_static("application/vnd.github.v3.diff"),
                    );
                    let response = inner
                        ._get_with_headers(&path as &str, Some(headers))
                        .await
                        .map_err(super::client::classify_octocrab_error)?;
                    // Check for error status before reading body.
                    let response = octocrab::map_github_error(response)
                        .await
                        .map_err(super::client::classify_octocrab_error)?;
                    inner
                        .body_to_string(response)
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        let files_changed = patch
            .lines()
            .filter(|l| l.starts_with("diff --git "))
            .count() as u32;
        let additions = patch
            .lines()
            .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
            .count() as u32;
        let deletions = patch
            .lines()
            .filter(|l| l.starts_with('-') && !l.starts_with("---"))
            .count() as u32;
        Ok(Diff {
            patch,
            files_changed,
            additions,
            deletions,
        })
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

fn pr_from_octocrab(repo: RepoRef, pr: octocrab::models::pulls::PullRequest) -> Pr {
    use crate::types::PrState;
    Pr {
        r: PrRef {
            repo,
            number: pr.number as u32,
        },
        title: pr.title.unwrap_or_default(),
        body: pr.body.unwrap_or_default(),
        state: match pr.state {
            Some(octocrab::models::IssueState::Open) => PrState::Open,
            _ if pr.merged_at.is_some() => PrState::Merged,
            _ => PrState::Closed,
        },
        head_branch: pr.head.ref_field,
        base_branch: pr.base.ref_field,
        author: pr.user.map(|u| u.login).unwrap_or_default(),
        created_at: pr.created_at.unwrap_or_else(chrono::Utc::now),
        updated_at: pr.updated_at.unwrap_or_else(chrono::Utc::now),
    }
}
