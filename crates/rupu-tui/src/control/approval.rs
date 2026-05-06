//! Inline approval handlers for the TUI (D-in). Wraps
//! `RunStore::approve` / `RunStore::reject` calls so the App can
//! resolve a focused gate without re-implementing orchestration.

use rupu_orchestrator::{ApprovalDecision, ApprovalError, RunStore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Approved { step_id: String },
    Rejected { step_id: String, reason: String },
    Error(String),
}

pub fn approve_focused(
    store: &RunStore,
    run_id: &str,
    approver: &str,
) -> Result<ApprovalOutcome, ApprovalError> {
    let now = chrono::Utc::now();
    match store.approve(run_id, approver, now)? {
        ApprovalDecision::Approved { step_id, .. } => Ok(ApprovalOutcome::Approved { step_id }),
        other => Ok(ApprovalOutcome::Error(format!("unexpected decision: {other:?}"))),
    }
}

pub fn reject_focused(
    store: &RunStore,
    run_id: &str,
    approver: &str,
    reason: &str,
) -> Result<ApprovalOutcome, ApprovalError> {
    let now = chrono::Utc::now();
    match store.reject(run_id, approver, reason, now)? {
        ApprovalDecision::Rejected { step_id, reason, .. } => {
            Ok(ApprovalOutcome::Rejected { step_id, reason })
        }
        other => Ok(ApprovalOutcome::Error(format!("unexpected decision: {other:?}"))),
    }
}
