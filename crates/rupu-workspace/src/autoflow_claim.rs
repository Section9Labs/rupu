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
pub struct AutoflowClaimRecord {
    pub issue_ref: String,
    pub repo_ref: String,
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
    pub next_retry_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub claim_owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub lease_expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pending_dispatch: Option<PendingDispatch>,
    pub updated_at: String,
}
