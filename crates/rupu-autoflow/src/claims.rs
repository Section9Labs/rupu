//! Claim views and checks: status names, the next-action summary, lease
//! expiry, whether a manual cycle may take a claim, and removing a claim's
//! worktree.

use crate::Refusal;
use anyhow::{anyhow, Context as _};
use rupu_workspace::{
    remove_issue_worktree, AutoflowClaimRecord, AutoflowClaimStore, AutoflowContender, ClaimStatus,
    RepoRegistryStore,
};
use std::path::PathBuf;

pub fn status_name(status: ClaimStatus) -> &'static str {
    match status {
        ClaimStatus::Eligible => "eligible",
        ClaimStatus::Claimed => "claimed",
        ClaimStatus::Running => "running",
        ClaimStatus::AwaitHuman => "await_human",
        ClaimStatus::AwaitExternal => "await_external",
        ClaimStatus::RetryBackoff => "retry_backoff",
        ClaimStatus::Blocked => "blocked",
        ClaimStatus::Complete => "complete",
        ClaimStatus::Released => "released",
    }
}

pub fn selected_priority(claim: &AutoflowClaimRecord) -> Option<i32> {
    claim
        .contenders
        .iter()
        .find(|contender| contender.selected)
        .map(|contender| contender.priority)
}

pub fn next_action_summary(claim: &AutoflowClaimRecord) -> String {
    if let Some(dispatch) = &claim.pending_dispatch {
        return format!("dispatch {}", dispatch.workflow);
    }
    if let Some(next_retry_at) = &claim.next_retry_at {
        return format!("retry {next_retry_at}");
    }
    match claim.status {
        ClaimStatus::AwaitHuman => "human approval".into(),
        ClaimStatus::AwaitExternal => "external change".into(),
        ClaimStatus::Running => claim
            .last_run_id
            .clone()
            .unwrap_or_else(|| "running".into()),
        _ => claim.last_run_id.clone().unwrap_or_else(|| "-".into()),
    }
}

pub fn format_contenders(contenders: &[AutoflowContender]) -> String {
    if contenders.is_empty() {
        return "-".into();
    }
    contenders
        .iter()
        .map(|contender| {
            let selected = if contender.selected { "*" } else { "" };
            format!("{selected}{}[{}]", contender.workflow, contender.priority)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn claim_lease_expired(claim: &AutoflowClaimRecord) -> anyhow::Result<bool> {
    let Some(lease_expires_at) = claim.lease_expires_at.as_deref() else {
        return Ok(false);
    };
    let lease = chrono::DateTime::parse_from_rfc3339(lease_expires_at)
        .with_context(|| format!("parse lease expiry for `{}`", claim.issue_ref))?
        .with_timezone(&chrono::Utc);
    Ok(lease <= chrono::Utc::now())
}

pub fn filter_claims_by_repo(
    claims: Vec<AutoflowClaimRecord>,
    repo_filter: Option<&str>,
) -> Vec<AutoflowClaimRecord> {
    match repo_filter {
        Some(repo_ref) => claims
            .into_iter()
            .filter(|claim| claim.repo_ref == repo_ref)
            .collect(),
        None => claims,
    }
}

pub fn cleanup_claim_artifacts(
    repo_store: &RepoRegistryStore,
    claim: &AutoflowClaimRecord,
) -> anyhow::Result<()> {
    let Some(worktree_path) = claim.worktree_path.as_deref() else {
        return Ok(());
    };
    let worktree_path = PathBuf::from(worktree_path);
    if !worktree_path.exists() {
        return Ok(());
    }
    let tracked = repo_store
        .load(&claim.repo_ref)?
        .ok_or_else(|| anyhow!("repo `{}` is not tracked", claim.repo_ref))?;
    let preferred_checkout = PathBuf::from(&tracked.preferred_path);
    let preferred_canonical = preferred_checkout
        .canonicalize()
        .unwrap_or_else(|_| preferred_checkout.clone());
    let worktree_canonical = worktree_path
        .canonicalize()
        .unwrap_or_else(|_| worktree_path.clone());
    if worktree_canonical == preferred_canonical {
        return Ok(());
    }
    remove_issue_worktree(&preferred_checkout, &worktree_path)
        .with_context(|| format!("remove worktree {}", worktree_path.display()))?;
    Ok(())
}

/// Whether `rupu autoflow run` may start a cycle on `issue_ref_text`: no
/// claim, or a finished one, or one whose lease expired. A blocked claim, an
/// active cycle or a live lease is a [`Refusal::Conflict`].
pub fn ensure_manual_run_can_take_claim(
    claim_store: &AutoflowClaimStore,
    issue_ref_text: &str,
) -> anyhow::Result<()> {
    let Some(claim) = claim_store.load(issue_ref_text)? else {
        return Ok(());
    };
    if matches!(claim.status, ClaimStatus::Complete | ClaimStatus::Released) {
        return Ok(());
    }
    if claim.status == ClaimStatus::Blocked {
        return Err(Refusal::Conflict(format!(
            "issue `{issue_ref_text}` has a blocked autoflow claim for workflow `{}`; release it first with `rupu autoflow release {issue_ref_text}`",
            claim.workflow
        ))
        .into());
    }
    if claim_store.read_active_lock(issue_ref_text)?.is_some() {
        return Err(Refusal::Conflict(format!(
            "issue `{issue_ref_text}` already has an active autoflow cycle for workflow `{}`",
            claim.workflow
        ))
        .into());
    }
    if !claim_lease_expired(&claim)? {
        return Err(Refusal::Conflict(format!(
            "issue `{issue_ref_text}` already has an owned autoflow claim for workflow `{}` with status `{}`; wait for it, run `rupu autoflow tick`, or release it first",
            claim.workflow,
            status_name(claim.status)
        ))
        .into());
    }
    Ok(())
}
