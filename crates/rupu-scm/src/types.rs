//! Vendor-neutral types returned by [`crate::RepoConnector`] and
//! [`crate::IssueConnector`]. Per-platform adapters translate their
//! native SDK shapes into these.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::platform::{IssueTracker, Platform};

/// Reference to a repository on a specific platform.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoRef {
    pub platform: Platform,
    pub owner: String,
    pub repo: String,
}

/// Reference to a pull/merge request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrRef {
    pub repo: RepoRef,
    pub number: u32,
}

/// Reference to an issue.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IssueRef {
    pub tracker: IssueTracker,
    /// Tracker-native project identifier. For GitHub Issues:
    /// "owner/repo". For Linear: workspace UUID. Etc.
    pub project: String,
    pub number: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueState {
    Open,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileEncoding {
    Utf8,
    Base64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    pub r: RepoRef,
    pub default_branch: String,
    pub clone_url_https: String,
    pub clone_url_ssh: String,
    pub private: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    pub sha: String,
    pub protected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileContent {
    pub path: String,
    /// Resolved ref the content was fetched at (commit sha or branch tip).
    pub ref_: String,
    pub content: String,
    pub encoding: FileEncoding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pr {
    pub r: PrRef,
    pub title: String,
    pub body: String,
    pub state: PrState,
    pub head_branch: String,
    pub base_branch: String,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub r: IssueRef,
    pub title: String,
    pub body: String,
    pub state: IssueState,
    pub labels: Vec<String>,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diff {
    /// Full unified-diff patch text.
    pub patch: String,
    pub files_changed: u32,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrFilter {
    pub state: Option<PrState>,
    pub author: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueFilter {
    pub state: Option<IssueState>,
    pub labels: Vec<String>,
    pub author: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePr {
    pub title: String,
    pub body: String,
    pub head: String,
    pub base: String,
    pub draft: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateIssue {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_ref_serde_roundtrip() {
        let r = RepoRef {
            platform: Platform::Github,
            owner: "section9labs".into(),
            repo: "rupu".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: RepoRef = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn pr_state_serde_lowercase() {
        for (v, s) in [
            (PrState::Open, "\"open\""),
            (PrState::Closed, "\"closed\""),
            (PrState::Merged, "\"merged\""),
        ] {
            let json = serde_json::to_string(&v).unwrap();
            assert_eq!(json, s);
            let back: PrState = serde_json::from_str(&json).unwrap();
            assert_eq!(back, v);
        }
    }

    #[test]
    fn pr_filter_default_is_empty() {
        let f = PrFilter::default();
        assert!(f.state.is_none());
        assert!(f.author.is_none());
        assert!(f.limit.is_none());
    }
}
