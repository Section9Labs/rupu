//! Persistent autoflow claim record.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    Eligible,
    Claimed,
    Running,
    AwaitHuman,
    AwaitExternal,
    RetryBackoff,
    Blocked,
    Complete,
    Released,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingDispatch {
    pub workflow: String,
    pub target: String,
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoflowContender {
    pub workflow: String,
    pub priority: i32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoflowClaimRecord {
    pub issue_ref: String,
    pub repo_ref: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub issue_display_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub issue_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub issue_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub issue_state_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub issue_tracker: Option<String>,
    pub workflow: String,
    pub status: ClaimStatus,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worktree_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pr_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub artifacts: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub artifact_manifest_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub next_retry_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub claim_owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub lease_expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pending_dispatch: Option<PendingDispatch>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contenders: Vec<AutoflowContender>,
    pub updated_at: String,
}
