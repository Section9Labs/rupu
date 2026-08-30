//! Vendor-neutral types returned by [`crate::RepoConnector`] and
//! [`crate::IssueConnector`]. Per-platform adapters translate their
//! native SDK shapes into these.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::account::AccountId;
use crate::platform::{IssueTracker, Platform};

/// Reference to a repository on a specific platform.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoRef {
    pub platform: Platform,
    pub owner: String,
    pub repo: String,
}

/// Generic event source for polled trigger feeds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventSourceRef {
    Repo {
        repo: RepoRef,
    },
    TrackerProject {
        tracker: IssueTracker,
        project: String,
        /// Explicit account override (Arc 2 Task 6, spec §6.5). A
        /// `Repo`-sourced trigger infers its account from `repo.owner`
        /// via an owner rule; a `TrackerProject` source has no owner or
        /// path baked into `project` for Linear/Jira (`"ENG"`,
        /// `"team-123"` — not an `owner/repo` shape), so multi-account
        /// genuinely cannot be inferred there and this field is the only
        /// lever. For a repo-backed tracker (`github`/`gitlab`, where
        /// `project` IS `owner/repo`) [`tracker_project_repo`] splits the
        /// owner back out so the normal owner-rule tier still applies —
        /// this field is an override there, not the only lever.
        ///
        /// `#[serde(default)]` so a `TrackerProject` value serialized
        /// before this field existed still deserializes: unset becomes
        /// `None`, i.e. "resolve without an explicit account", which is
        /// exactly today's (pre-field) behavior.
        #[serde(default)]
        account: Option<AccountId>,
    },
}

impl EventSourceRef {
    pub fn vendor(&self) -> &'static str {
        match self {
            Self::Repo { repo } => repo.platform.as_str(),
            Self::TrackerProject { tracker, .. } => tracker.as_str(),
        }
    }

    pub fn repo(&self) -> Option<&RepoRef> {
        match self {
            Self::Repo { repo } => Some(repo),
            Self::TrackerProject { .. } => None,
        }
    }

    pub fn tracker(&self) -> Option<IssueTracker> {
        match self {
            Self::Repo { repo } => match repo.platform {
                Platform::Github => Some(IssueTracker::Github),
                Platform::Gitlab => Some(IssueTracker::Gitlab),
            },
            Self::TrackerProject { tracker, .. } => Some(*tracker),
        }
    }

    pub fn source_ref_text(&self) -> String {
        match self {
            Self::Repo { repo } => {
                format!("{}:{}/{}", repo.platform.as_str(), repo.owner, repo.repo)
            }
            Self::TrackerProject {
                tracker, project, ..
            } => {
                format!("{}:{project}", tracker.as_str())
            }
        }
    }
}

/// Split a tracker `project` string into a `RepoRef` for a repo-backed
/// tracker (`github`/`gitlab`, where `project` is `owner/repo`).
/// Returns `None` for `Linear`/`Jira` (no `owner/repo` shape to split)
/// or a `project` with no `/`.
///
/// Shared by every call site that needs to route a `tracker` + `project`
/// pair through the account rule engine's owner-rule tier —
/// `rupu-mcp`'s `tools/issues.rs::project_repo` and `rupu-cli`'s
/// `cmd/issues.rs::issue_ref_repo` had already written this split
/// independently before `Registry::events_for_source` (Task 6) needed a
/// third copy; this is the shared version they both now delegate to.
pub fn tracker_project_repo(tracker: IssueTracker, project: &str) -> Option<RepoRef> {
    let platform = match tracker {
        IssueTracker::Github => Platform::Github,
        IssueTracker::Gitlab => Platform::Gitlab,
        IssueTracker::Linear | IssueTracker::Jira => return None,
    };
    let (owner, repo) = project.split_once('/')?;
    Some(RepoRef {
        platform,
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

impl fmt::Display for EventSourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source_ref_text())
    }
}

impl From<RepoRef> for EventSourceRef {
    fn from(repo: RepoRef) -> Self {
        Self::Repo { repo }
    }
}

impl FromStr for EventSourceRef {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (kind, rest) = value
            .split_once(':')
            .ok_or_else(|| format!("invalid trigger source `{value}`"))?;
        if rest.is_empty() {
            return Err(format!("invalid trigger source `{value}`"));
        }
        match kind {
            "github" | "gitlab" => {
                let (owner, repo) = rest
                    .rsplit_once('/')
                    .ok_or_else(|| format!("invalid repo trigger source `{value}`"))?;
                if owner.is_empty() || repo.is_empty() {
                    return Err(format!("invalid repo trigger source `{value}`"));
                }
                let platform = Platform::from_str(kind)?;
                Ok(Self::Repo {
                    repo: RepoRef {
                        platform,
                        owner: owner.to_string(),
                        repo: repo.to_string(),
                    },
                })
            }
            "linear" | "jira" => Ok(Self::TrackerProject {
                tracker: IssueTracker::from_str(kind)?,
                project: rest.to_string(),
                // The compact `kind:project` string form has nowhere to
                // encode an account; callers that need one (cron's
                // `PollSourceSpec.account`) set it on the parsed value
                // afterward. See the field's doc.
                account: None,
            }),
            other => Err(format!("unknown trigger source kind: {other}")),
        }
    }
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

/// Best-effort subject identified for one polled event.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventSubjectRef {
    Repo { repo: RepoRef },
    Issue { issue: IssueRef },
    Pr { pr: PrRef },
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
    /// Head commit SHA. Backs the autoflow PR claim key `(repo,
    /// pr_number, head_sha)` (dogfood autoflows spec): a new head SHA is
    /// a fresh claim (re-review on push), an unchanged SHA is
    /// already-claimed (skip). Populated by the GitHub connector from
    /// `head.sha`; GitLab from the MR `sha`. Serde-default empty so
    /// pre-existing persisted `Pr`s deserialize unchanged.
    #[serde(default)]
    pub head_sha: String,
    /// Whether the PR is a draft. Backs the autoflow PR selector's
    /// `draft` filter. Serde-default `false` for backward compatibility.
    #[serde(default)]
    pub draft: bool,
    /// Label names attached to the PR. Backs the autoflow PR selector's
    /// label filters (`labels_all/any/none`). Serde-default empty.
    #[serde(default)]
    pub labels: Vec<String>,
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
    /// Vendor-supplied hex colors (no leading `#`) for each label.
    /// Optional — connectors that don't expose label colors leave this
    /// empty and the renderer falls back to a deterministic hash-based
    /// chip color. Persisted on `RunRecord.issue` so resume / replay
    /// keep the chip colors stable across rupu restarts. Backward-
    /// compatible: pre-existing serialized issues without this field
    /// deserialize as an empty map.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub label_colors: std::collections::BTreeMap<String, String>,
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
    /// GitHub's `author_association` for this comment (e.g. `"OWNER"`,
    /// `"COLLABORATOR"`, `"CONTRIBUTOR"`, `"NONE"`) — the signal a
    /// consumer needs to tell an authorized operator's directive apart
    /// from an arbitrary commenter's message. `None` when the connector
    /// cannot supply it (GitLab, Linear, Jira) rather than inventing a
    /// value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_association: Option<String>,
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

/// Args for `github.workflows_dispatch` (Plan 2 Task 13 surfaces it via MCP).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowDispatch {
    pub workflow: String,          // workflow file name or numeric ID
    pub ref_: String,              // branch/tag/sha
    pub inputs: serde_json::Value, // free-form, validated against workflow's `inputs:` schema
}

/// Args for `gitlab.pipeline_trigger` (Plan 2 Task 13 surfaces it via MCP).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineTrigger {
    pub ref_: String,
    pub variables: std::collections::BTreeMap<String, String>,
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
    fn event_source_ref_parses_repo_and_nested_gitlab_group() {
        let github = EventSourceRef::from_str("github:Section9Labs/rupu").unwrap();
        assert_eq!(github.source_ref_text(), "github:Section9Labs/rupu");
        let gitlab = EventSourceRef::from_str("gitlab:group/subgroup/repo").unwrap();
        assert_eq!(gitlab.source_ref_text(), "gitlab:group/subgroup/repo");
        let repo = gitlab.repo().unwrap();
        assert_eq!(repo.owner, "group/subgroup");
        assert_eq!(repo.repo, "repo");
    }

    #[test]
    fn event_source_ref_parses_tracker_project() {
        let linear = EventSourceRef::from_str("linear:workspace-123").unwrap();
        assert_eq!(linear.vendor(), "linear");
        assert_eq!(linear.source_ref_text(), "linear:workspace-123");
        assert!(linear.repo().is_none());
        assert_eq!(linear.tracker(), Some(IssueTracker::Linear));

        let jira = EventSourceRef::from_str("jira:acme.atlassian.net/ENG").unwrap();
        assert_eq!(jira.vendor(), "jira");
        assert_eq!(jira.source_ref_text(), "jira:acme.atlassian.net/ENG");
        assert!(jira.repo().is_none());
        assert_eq!(jira.tracker(), Some(IssueTracker::Jira));
    }

    #[test]
    fn tracker_project_from_str_has_no_explicit_account() {
        // The compact `kind:project` string form has nowhere to encode
        // an account (Task 6) — every parsed value starts `None`, and
        // callers (cron's `PollSourceSpec.account`) set it afterward.
        let linear = EventSourceRef::from_str("linear:workspace-123").unwrap();
        match linear {
            EventSourceRef::TrackerProject { account, .. } => assert_eq!(account, None),
            EventSourceRef::Repo { .. } => panic!("expected TrackerProject"),
        }
    }

    #[test]
    fn tracker_project_account_field_round_trips_through_serde() {
        let with_account = EventSourceRef::TrackerProject {
            tracker: IssueTracker::Linear,
            project: "team-123".into(),
            account: Some(AccountId::new("linear-work")),
        };
        let json = serde_json::to_string(&with_account).unwrap();
        let back: EventSourceRef = serde_json::from_str(&json).unwrap();
        assert_eq!(with_account, back);
    }

    #[test]
    fn tracker_project_deserializes_pre_account_field_json() {
        // Back-compat (Task 6 constraint): a `TrackerProject` value
        // serialized before the `account` field existed — i.e. missing
        // the key entirely — must still deserialize, as `None`.
        let legacy_json = r#"{"kind":"tracker_project","tracker":"jira","project":"ENG"}"#;
        let parsed: EventSourceRef = serde_json::from_str(legacy_json).unwrap();
        match parsed {
            EventSourceRef::TrackerProject {
                tracker,
                project,
                account,
            } => {
                assert_eq!(tracker, IssueTracker::Jira);
                assert_eq!(project, "ENG");
                assert_eq!(account, None);
            }
            EventSourceRef::Repo { .. } => panic!("expected TrackerProject"),
        }
    }

    #[test]
    fn tracker_project_repo_splits_repo_backed_trackers() {
        let repo = tracker_project_repo(IssueTracker::Github, "acme/api").unwrap();
        assert_eq!(repo.platform, Platform::Github);
        assert_eq!(repo.owner, "acme");
        assert_eq!(repo.repo, "api");

        let repo = tracker_project_repo(IssueTracker::Gitlab, "group/subgroup/proj").unwrap();
        assert_eq!(repo.platform, Platform::Gitlab);
        assert_eq!(repo.owner, "group");
        assert_eq!(repo.repo, "subgroup/proj");
    }

    #[test]
    fn tracker_project_repo_returns_none_for_non_repo_backed_or_unsplittable() {
        assert!(tracker_project_repo(IssueTracker::Linear, "team-123").is_none());
        assert!(tracker_project_repo(IssueTracker::Jira, "ENG").is_none());
        // No `/` to split on, even for a repo-backed tracker.
        assert!(tracker_project_repo(IssueTracker::Github, "no-slash").is_none());
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
