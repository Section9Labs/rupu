//! Operator-safe approval projection. Never serialize the private store record
//! directly into lists, events, errors or logs.
use super::store::{Completion, DecisionReceipt, Effect, Record, RevokeReceipt, State};
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Serialize)]
pub struct ApprovalView {
    pub schema_version: u32,
    pub approval_id: String,
    pub cp_instance_id: String,
    pub broker_epoch: String,
    pub worker_generation: u64,
    pub host_id: &'static str,
    pub session_id: String,
    pub root_turn_run_id: String,
    pub run_id: String,
    pub turn_index: u64,
    pub tool_call_id: String,
    pub call_sequence: u64,
    pub workspace_id: String,
    pub workspace_path: String,
    pub parent_run_id: Option<String>,
    pub root_run_id: String,
    pub tool: String,
    pub called_as: String,
    pub effect: Effect,
    pub intent_fingerprint: String,
    pub input_fingerprint: String,
    pub review_version: u32,
    pub review_fingerprint: String,
    pub review_complete: bool,
    /// Full exact arguments only in the explicitly requested detail response.
    /// Known credential-bearing field names are refused before publication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<Value>,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub revision: u64,
    pub state: State,
    pub updated_at_ms: u64,
    pub decision: Option<DecisionReceipt>,
    pub revoke: Option<RevokeReceipt>,
    pub dispatch_id: Option<String>,
    pub dispatched_at_ms: Option<u64>,
    pub completion: Option<Completion>,
    pub reason: Option<String>,
    /// Advisory current state only. Mutation still checks live authority and CAS.
    pub can_decide: bool,
    pub can_revoke: bool,
}

impl ApprovalView {
    pub fn from_record(record: &Record, detail: bool, live: bool, now_ms: u64) -> Self {
        let i = &record.intent;
        let unexpired = now_ms < i.expires_at_ms;
        Self {
            schema_version: 1,
            approval_id: i.approval_id.clone(),
            cp_instance_id: i.cp_instance_id.clone(),
            broker_epoch: i.broker_epoch.clone(),
            worker_generation: i.worker_generation,
            host_id: "local",
            session_id: i.session_id.clone(),
            root_turn_run_id: i.root_turn_run_id.clone(),
            run_id: i.run_id.clone(),
            turn_index: i.turn_index,
            tool_call_id: i.tool_call_id.clone(),
            call_sequence: i.call_sequence,
            workspace_id: i.workspace_id.clone(),
            workspace_path: i.workspace_path.clone(),
            parent_run_id: i.parent_run_id.clone(),
            root_run_id: i.root_run_id.clone(),
            tool: i.tool.clone(),
            called_as: i.called_as.clone(),
            effect: i.effect,
            intent_fingerprint: record.intent_fingerprint.clone(),
            input_fingerprint: record.input_fingerprint.clone(),
            review_version: 1,
            review_fingerprint: record.review_fingerprint.clone(),
            review_complete: true,
            review: detail.then(|| i.review.clone()),
            created_at_ms: i.created_at_ms,
            expires_at_ms: i.expires_at_ms,
            revision: record.revision,
            state: record.state,
            updated_at_ms: record.updated_at_ms,
            decision: record.decision.clone(),
            revoke: record.revoke.clone(),
            dispatch_id: record.dispatch_id.clone(),
            dispatched_at_ms: record.dispatched_at_ms,
            completion: record.completion.clone(),
            reason: record.reason.clone(),
            can_decide: live && unexpired && record.state == State::Pending,
            can_revoke: live && unexpired && record.state.is_waiting(),
        }
    }
}
