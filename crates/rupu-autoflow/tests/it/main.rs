//! rupu-autoflow's integration tests: the reports, repair and the run
//! precheck over a real `RUPU_HOME` in a temp dir.

mod claims;
mod reports;

use rupu_workspace::{AutoflowClaimRecord, ClaimStatus};

/// A claim another cycle owns: its lease runs another hour.
pub(crate) fn owned_claim(issue_ref: &str, status: ClaimStatus) -> AutoflowClaimRecord {
    AutoflowClaimRecord {
        issue_ref: issue_ref.into(),
        repo_ref: "github:Section9Labs/rupu".into(),
        source_ref: None,
        issue_display_ref: None,
        issue_title: None,
        issue_url: None,
        issue_state_name: None,
        issue_tracker: None,
        workflow: "controller".into(),
        status,
        worktree_path: None,
        branch: None,
        last_run_id: None,
        last_error: None,
        last_summary: None,
        pr_url: None,
        artifacts: None,
        artifact_manifest_path: None,
        next_retry_at: None,
        claim_owner: None,
        lease_expires_at: Some((chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339()),
        pending_dispatch: None,
        contenders: vec![],
        updated_at: chrono::Utc::now().to_rfc3339(),
    }
}
