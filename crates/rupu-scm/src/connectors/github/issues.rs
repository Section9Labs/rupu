//! GitHub IssueConnector. Implementation lands in Task 12.

use async_trait::async_trait;

use crate::connectors::IssueConnector;
use crate::error::ScmError;
use crate::platform::IssueTracker;
use crate::types::{Comment, CreateIssue, Issue, IssueFilter, IssueRef, IssueState};

use super::client::GithubClient;

pub struct GithubIssueConnector {
    client: GithubClient,
}

impl GithubIssueConnector {
    pub fn new(client: GithubClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl IssueConnector for GithubIssueConnector {
    fn tracker(&self) -> IssueTracker {
        IssueTracker::Github
    }

    async fn list_issues(
        &self,
        project: &str,
        filter: IssueFilter,
    ) -> Result<Vec<Issue>, ScmError> {
        let _permit = self.client.permit().await;
        let (owner, repo) = parse_project(project)?;
        let inner = self.client.inner.clone();
        let labels: Vec<String> = filter.labels.clone();
        let project_str = project.to_string();
        let pages = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                let labels = labels.clone();
                async move {
                    let handler = inner.issues(&owner, &repo);
                    let mut req = handler.list();
                    if !labels.is_empty() {
                        req = req.labels(&labels);
                    }
                    req.send()
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(pages
            .items
            .into_iter()
            .map(|item| issue_from_octocrab(project_str.clone(), item))
            .collect())
    }

    async fn get_issue(&self, i: &IssueRef) -> Result<Issue, ScmError> {
        let _permit = self.client.permit().await;
        let (owner, repo) = parse_project(&i.project)?;
        let number = i.number;
        let project = i.project.clone();
        let inner = self.client.inner.clone();
        let model = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                async move {
                    inner
                        .issues(&owner, &repo)
                        .get(number)
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(issue_from_octocrab(project, model))
    }

    async fn comment_issue(&self, i: &IssueRef, body: &str) -> Result<Comment, ScmError> {
        let _permit = self.client.permit().await;
        let (owner, repo) = parse_project(&i.project)?;
        let number = i.number;
        let inner = self.client.inner.clone();
        let body = body.to_string();
        let model = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                let body = body.clone();
                async move {
                    inner
                        .issues(&owner, &repo)
                        .create_comment(number, &body)
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(Comment {
            id: model.id.to_string(),
            author: model.user.login,
            body,
            created_at: model.created_at,
        })
    }

    async fn create_issue(&self, project: &str, opts: CreateIssue) -> Result<Issue, ScmError> {
        let _permit = self.client.permit().await;
        let (owner, repo) = parse_project(project)?;
        let inner = self.client.inner.clone();
        let project_str = project.to_string();
        let model = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                let opts = opts.clone();
                async move {
                    inner
                        .issues(&owner, &repo)
                        .create(opts.title)
                        .body(opts.body)
                        .labels(opts.labels)
                        .send()
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(issue_from_octocrab(project_str, model))
    }

    async fn update_issue_state(&self, i: &IssueRef, state: IssueState) -> Result<(), ScmError> {
        let _permit = self.client.permit().await;
        let (owner, repo) = parse_project(&i.project)?;
        let number = i.number;
        let inner = self.client.inner.clone();
        self.client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                async move {
                    inner
                        .issues(&owner, &repo)
                        .update(number)
                        .state(match state {
                            IssueState::Open => octocrab::models::IssueState::Open,
                            IssueState::Closed => octocrab::models::IssueState::Closed,
                        })
                        .send()
                        .await
                        .map_err(super::client::classify_octocrab_error)?;
                    Ok(())
                }
            })
            .await
    }
}

fn parse_project(project: &str) -> Result<(String, String), ScmError> {
    let (o, r) = project
        .split_once('/')
        .ok_or_else(|| ScmError::BadRequest {
            message: format!("project must be `owner/repo`: {project}"),
        })?;
    Ok((o.to_string(), r.to_string()))
}

fn issue_from_octocrab(project: String, item: octocrab::models::issues::Issue) -> Issue {
    // Walk labels once, building both the name list and the
    // name->hex map. octocrab exposes `Label.color` as a hex string
    // without `#`; persist as-is so the renderer can reuse it
    // verbatim.
    let mut labels = Vec::with_capacity(item.labels.len());
    let mut label_colors = std::collections::BTreeMap::new();
    for l in item.labels {
        if !l.color.is_empty() {
            label_colors.insert(l.name.clone(), l.color);
        }
        labels.push(l.name);
    }
    Issue {
        r: IssueRef {
            tracker: IssueTracker::Github,
            project,
            number: item.number,
        },
        title: item.title,
        body: item.body.unwrap_or_default(),
        state: match item.state {
            octocrab::models::IssueState::Open => IssueState::Open,
            _ => IssueState::Closed,
        },
        labels,
        label_colors,
        author: item.user.login,
        created_at: item.created_at,
        updated_at: item.updated_at,
    }
}
