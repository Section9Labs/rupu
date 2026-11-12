//! Optional read-only issue port; the owning CLI registry supplies credentials.
use rupu_scm::{operator_issues::IssueCommentPage, Issue, IssueTracker};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize)]
pub struct IssueReadAccount {
    pub tracker: IssueTracker,
    pub account: String,
    pub origin: Option<String>,
    pub body_supported: bool,
    pub comments_supported: bool,
    pub discussion_supported: bool,
    pub discovery_supported: bool,
    pub discovery_search_supported: bool,
    pub unavailable_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueReadTarget {
    pub tracker: IssueTracker,
    pub project: String,
    pub account: String,
    pub number: u64,
    pub origin: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueDiscoveryTarget {
    pub tracker: IssueTracker,
    pub project: String,
    pub account: String,
    pub origin: String,
}

#[derive(Clone, Copy, Debug)]
pub enum IssueReadError {
    AccountUnavailable,
    Unsupported,
    SelectionChanged,
    InvalidTarget,
    NotFound,
    Forbidden,
    RateLimited,
    UpstreamUnavailable,
}

#[async_trait::async_trait]
pub trait IssueReader: Send + Sync {
    fn accounts(&self) -> Vec<IssueReadAccount>;
    fn supports_discovery(&self) -> bool {
        false
    }
    fn discovery_epoch(&self, _target: &IssueDiscoveryTarget) -> Result<String, IssueReadError> {
        Err(IssueReadError::Unsupported)
    }
    async fn discovery_page(
        &self,
        _target: &IssueDiscoveryTarget,
        _request: &rupu_scm::issue_discovery::DiscoveryRequest,
    ) -> Result<rupu_scm::issue_discovery::DiscoveryPage, IssueReadError> {
        Err(IssueReadError::Unsupported)
    }
    fn supports_discussion(&self) -> bool {
        false
    }
    fn discussion_epoch(&self, _target: &IssueReadTarget) -> Result<String, IssueReadError> {
        Err(IssueReadError::Unsupported)
    }
    async fn discussion_page(
        &self,
        _target: &IssueReadTarget,
        _after: Option<&str>,
    ) -> Result<rupu_scm::issue_discussion::DiscussionPage, IssueReadError> {
        Err(IssueReadError::Unsupported)
    }

    fn comments_supported(&self, target: &IssueReadTarget) -> Result<bool, IssueReadError>;
    async fn issue(&self, target: &IssueReadTarget) -> Result<Issue, IssueReadError>;
    async fn comments(
        &self,
        target: &IssueReadTarget,
        page: u32,
    ) -> Result<IssueCommentPage, IssueReadError>;
}
