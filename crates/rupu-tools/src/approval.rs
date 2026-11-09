//! Exact, asynchronous per-call approval boundary. This is a runtime port,
//! not a transport, credential source, tool grant or reusable authorization.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalOwner {
    pub session_id: String,
    pub root_turn_run_id: String,
    pub run_id: String,
    pub parent_run_id: Option<String>,
    pub root_run_id: String,
    pub workspace_id: String,
    pub workspace_path: String,
    pub settings_revision: u64,
    pub orchestration_revision: u64,
    pub host_mcp_registry_fingerprint: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalEffect {
    Write,
    External,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRemote {
    pub server_id: String,
    pub tool: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallApprovalRequest {
    pub owner: ApprovalOwner,
    pub turn_index: u64,
    pub tool_call_id: String,
    pub call_sequence: u64,
    pub tool: String,
    pub called_as: String,
    pub effect: ApprovalEffect,
    pub descriptor_fingerprint: String,
    pub grant_fingerprint: String,
    pub remote: Option<ApprovalRemote>,
    pub input: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalCompletion {
    pub success: bool,
    /// Operator-safe category only; never raw tool output or credentials.
    pub error: Option<String>,
    pub result_fingerprint: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("tool approval unavailable: {0}")]
pub struct ApprovalError(pub String);

/// A safe broker-authored transcript event. The receiver must still verify
/// its owner against the call before recording it. No input payload is needed.
pub type ApprovalObserver<'a> = dyn FnMut(Value) -> Result<(), ApprovalError> + Send + 'a;

/// Noncloneable dispatch authority. Dropping an uncompleted permit must make
/// its transport report uncertainty; it must never make the call retryable.
#[async_trait]
pub trait ApprovalPermit: Send {
    fn approval_id(&self) -> &str;
    fn intent_fingerprint(&self) -> &str;
    async fn complete(self: Box<Self>, outcome: ApprovalCompletion) -> Result<(), ApprovalError>;
}

pub enum ApprovalOutcome {
    Denied { reason: String },
    Permitted(Box<dyn ApprovalPermit>),
}

#[async_trait]
pub trait CallAuthorizer: Send + Sync {
    /// Frozen root-turn binding supplied by the session worker, not the model.
    fn root_owner(&self) -> ApprovalOwner;
    /// A live private-broker round trip before Ask assembly/provider work.
    async fn check_owner(&self, owner: &ApprovalOwner) -> Result<(), ApprovalError>;
    /// Wait without blocking an executor thread. On future cancellation the
    /// implementation must revoke the channel, including pending requests.
    async fn authorize(
        &self,
        request: CallApprovalRequest,
        observer: &mut ApprovalObserver<'_>,
    ) -> Result<ApprovalOutcome, ApprovalError>;
}
