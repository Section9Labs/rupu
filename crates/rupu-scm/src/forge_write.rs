//! Reviewed operator forge writes. These DTOs carry no credentials or requests.
//! Provider preflight returns a private, consuming single-dispatch object.
use crate::{IssueTracker, RepoRef};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForgeAction {
    #[serde(rename = "workflow.dispatch")]
    WorkflowDispatch,
    #[serde(rename = "branch.create")]
    BranchCreate,
    #[serde(rename = "review.comment")]
    ReviewComment,
    #[serde(rename = "review.create")]
    ReviewCreate,
    #[serde(rename = "issue.comment")]
    IssueComment,
    #[serde(rename = "issue.create")]
    IssueCreate,
    #[serde(rename = "issue.set_state")]
    IssueSetState,
}
impl ForgeAction {
    pub fn is_review(self) -> bool {
        matches!(self, Self::ReviewComment | Self::ReviewCreate)
    }
    pub fn is_repository(self) -> bool {
        self.is_review() || matches!(self, Self::BranchCreate | Self::WorkflowDispatch)
    }
    pub fn needs_existing_number(self) -> bool {
        matches!(
            self,
            Self::ReviewComment | Self::IssueComment | Self::IssueSetState
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeTarget {
    pub provider: IssueTracker,
    pub account: String,
    pub api_origin: String,
    pub project_ref: String,
    #[serde(default)]
    pub number: Option<String>,
    #[serde(default)]
    pub ws_id: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_kind: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub workflow_inputs: BTreeMap<String, String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub head: Option<String>,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub draft: Option<bool>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub choice_id: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeRequest {
    pub action: ForgeAction,
    pub target: ForgeTarget,
    pub expected: BTreeMap<String, String>,
    pub input: ForgeInput,
}
/// Supplied by the owning CP's registered-checkout resolver, never deserialized
/// from a caller's JSON. All repository actions require this exact owner.
#[derive(Clone, Debug)]
pub struct ForgeOwner {
    pub ws_id: String,
    pub canonical_root: PathBuf,
    pub repo: RepoRef,
    pub origin_host: String,
    pub origin_url_identity: String,
    pub checkout_identity: String,
    pub branch: String,
    pub head: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedWrite {
    pub request: ForgeRequest,
    pub authority: Value,
    pub observed: Value,
    pub preconditions: Value,
    pub limitations: Vec<String>,
    /// Nonsecret adapter/credential generation, never a token-derived digest.
    pub generation: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct ForgeCapability {
    pub provider: IssueTracker,
    pub account: String,
    pub api_origin: Option<String>,
    pub actions: Vec<ForgeAction>,
    pub unavailable_reason: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ForgeChoice {
    pub id: String,
    pub name: String,
    pub kind: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct ForgeError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<ForgeChoice>>,
}
impl ForgeError {
    pub fn fixed(code: &'static str, message: &'static str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            choices: None,
        }
    }
    pub fn unsupported() -> Self {
        Self::fixed(
            "unsupported",
            "This configured connector cannot safely prepare this operator action",
        )
    }
    pub fn invalid() -> Self {
        Self::fixed(
            "invalid_intent",
            "The exact action, target, expectations and input are required",
        )
    }
    pub fn changed() -> Self {
        Self::fixed(
            "selection_changed",
            "The reviewed target, state or owning account changed; prepare a new intent",
        )
    }
    pub fn upstream() -> Self {
        Self::fixed(
            "upstream_unavailable",
            "The forge did not return a usable bounded response",
        )
    }
    pub fn choice(choices: Vec<ForgeChoice>) -> Self {
        Self {
            code: "choice_required".into(),
            message: "Choose the exact provider action before preparing this intent".into(),
            choices: Some(choices),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchOutcome {
    Accepted,
    Rejected,
    Unknown,
}
#[derive(Clone, Debug, Serialize)]
pub struct DispatchResult {
    pub outcome: DispatchOutcome,
    pub reason: String,
    pub result: Value,
}
impl DispatchResult {
    pub fn unknown() -> Self {
        Self {
            outcome: DispatchOutcome::Unknown,
            reason:
                "No trustworthy provider acknowledgement was obtained; do not retry automatically"
                    .into(),
            result: Value::Null,
        }
    }
}
#[async_trait::async_trait]
pub trait PreparedMutation: Send {
    /// Final local generation guard; no provider request is made.
    fn is_current(&self) -> bool {
        true
    }
    /// Must be invoked only after the owning CP durably records dispatching.
    /// The consumed object holds already selected credentials privately.
    async fn dispatch(self: Box<Self>) -> DispatchResult;
}

mod provider;
mod transport;
pub use transport::ForgeSession;

pub use provider::validate_request;

pub(crate) use transport::http;
