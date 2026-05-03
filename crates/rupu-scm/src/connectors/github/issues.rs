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

    async fn comment_issue(&self, _i: &IssueRef, _body: &str) -> Result<Comment, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn create_issue(&self, _project: &str, _opts: CreateIssue) -> Result<Issue, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn update_issue_state(&self, _i: &IssueRef, _state: IssueState) -> Result<(), ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
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
        labels: item.labels.into_iter().map(|l| l.name).collect(),
        author: item.user.login,
        created_at: item.created_at,
        updated_at: item.updated_at,
    }
}
