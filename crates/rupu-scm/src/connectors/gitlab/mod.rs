//! GitLab connector — implements RepoConnector + IssueConnector.

pub mod client;
pub mod issues;
pub mod repo;

pub use client::GitlabClient;
pub use issues::GitlabIssueConnector;
pub use repo::GitlabRepoConnector;
