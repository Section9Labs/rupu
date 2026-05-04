//! Shared test helpers.

use std::sync::Arc;

use httpmock::MockServer;
use rupu_scm::{IssueConnector, RepoConnector};

/// Build a GitHub `RepoConnector` whose API base points at `server`.
#[allow(dead_code)]
pub fn github_connector_against(server: &MockServer) -> Arc<dyn RepoConnector> {
    use rupu_scm::connectors::github::GithubClient;
    let client = GithubClient::new("ghp_test".into(), Some(server.base_url()), Some(2));
    Arc::new(rupu_scm::connectors::github::repo::GithubRepoConnector::new(client))
}

/// Build a GitHub `IssueConnector` whose API base points at `server`.
#[allow(dead_code)]
pub fn github_issue_connector_against(server: &MockServer) -> Arc<dyn IssueConnector> {
    use rupu_scm::connectors::github::GithubClient;
    let client = GithubClient::new("ghp_test".into(), Some(server.base_url()), Some(2));
    Arc::new(rupu_scm::connectors::github::issues::GithubIssueConnector::new(client))
}

/// Build a GitLab `RepoConnector` whose API base points at `server`.
#[allow(dead_code)]
pub fn gitlab_repo_connector_against(server: &MockServer) -> Arc<dyn RepoConnector> {
    use rupu_scm::connectors::gitlab::client::GitlabClient;
    let client = GitlabClient::new("glpat-test".into(), Some(server.base_url()), Some(2));
    Arc::new(rupu_scm::connectors::gitlab::repo::GitlabRepoConnector::new(client))
}

/// Build a GitLab `IssueConnector` whose API base points at `server`.
#[allow(dead_code)]
pub fn gitlab_issue_connector_against(server: &MockServer) -> Arc<dyn IssueConnector> {
    use rupu_scm::connectors::gitlab::client::GitlabClient;
    let client = GitlabClient::new("glpat-test".into(), Some(server.base_url()), Some(2));
    Arc::new(rupu_scm::connectors::gitlab::issues::GitlabIssueConnector::new(client))
}
