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
        _project: &str,
        _filter: IssueFilter,
    ) -> Result<Vec<Issue>, ScmError> {
        let _ = &self.client;
        Err(ScmError::Transient(anyhow::anyhow!(
            "github::list_issues not yet implemented (Task 12)"
        )))
    }

    async fn get_issue(&self, _i: &IssueRef) -> Result<Issue, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
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
