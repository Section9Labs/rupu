//! GitlabIssueConnector. Implementation lands in Plan 2 Task 5.

use async_trait::async_trait;

use crate::connectors::IssueConnector;
use crate::error::ScmError;
use crate::platform::IssueTracker;
use crate::types::{Comment, CreateIssue, Issue, IssueFilter, IssueRef, IssueState};

use super::client::GitlabClient;

#[allow(dead_code)]
pub struct GitlabIssueConnector {
    client: GitlabClient,
}

impl GitlabIssueConnector {
    pub fn new(client: GitlabClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl IssueConnector for GitlabIssueConnector {
    fn tracker(&self) -> IssueTracker {
        IssueTracker::Gitlab
    }

    async fn list_issues(
        &self,
        _project: &str,
        _filter: IssueFilter,
    ) -> Result<Vec<Issue>, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!(
            "not yet implemented (Task 5)"
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
