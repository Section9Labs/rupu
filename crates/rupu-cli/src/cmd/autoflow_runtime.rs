use crate::cmd::autoflow as legacy;
use crate::paths;
use anyhow::Context;
use rupu_auth::CredentialResolver;
use rupu_workspace::{AutoflowClaimRecord, AutoflowClaimStore, ClaimStatus, RepoRegistryStore};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickReport {
    pub workflow_count: usize,
    pub polled_event_count: usize,
    pub webhook_event_count: usize,
    pub ran_cycles: usize,
    pub skipped_cycles: usize,
    pub failed_cycles: usize,
    pub cleaned_claims: usize,
}

pub(crate) async fn tick_with_resolver(
    resolver: Arc<dyn CredentialResolver>,
) -> anyhow::Result<TickReport> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let repo_store = RepoRegistryStore {
        root: paths::repos_dir(&global),
    };
    let claim_store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    let cleaned =
        legacy::cleanup_terminal_claims(&global, &repo_store, &claim_store, chrono::Utc::now())?;
    let discovered = legacy::discover_tick_autoflows(&global, &repo_store)?;
    if discovered.is_empty() {
        return Ok(TickReport {
            cleaned_claims: cleaned,
            ..TickReport::default()
        });
    }
    let wake_hints = legacy::collect_wake_hints(&global, &discovered, resolver.as_ref())
        .await
        .context("collect autoflow wake hints")?;

    let tick_started_at = chrono::Utc::now();
    let matches = legacy::collect_issue_matches(&discovered, resolver.as_ref())
        .await
        .context("discover autoflow issue matches")?;
    let contenders_by_issue = legacy::summarize_issue_contenders(&matches);
    let winners = legacy::choose_winning_matches(matches);
    let mut claims_by_issue: BTreeMap<String, AutoflowClaimRecord> = claim_store
        .list()?
        .into_iter()
        .map(|claim| (claim.issue_ref.clone(), claim))
        .collect();
    let mut issue_keys: BTreeSet<String> = winners.keys().cloned().collect();
    issue_keys.extend(claims_by_issue.keys().cloned());

    let mut active_claim_counts: BTreeMap<String, usize> = BTreeMap::new();
    for claim in claims_by_issue.values() {
        if legacy::claim_counts_toward_max_active(claim.status) {
            *active_claim_counts
                .entry(claim.repo_ref.clone())
                .or_insert(0) += 1;
        }
    }

    let mut report = TickReport {
        workflow_count: discovered.len(),
        polled_event_count: wake_hints.total_polled_events,
        webhook_event_count: wake_hints.total_webhook_events,
        cleaned_claims: cleaned,
        ..TickReport::default()
    };

    for issue_ref_text in issue_keys {
        let winner = winners.get(&issue_ref_text).cloned();
        let claim = claims_by_issue.remove(&issue_ref_text);
        let contenders = contenders_by_issue
            .get(&issue_ref_text)
            .cloned()
            .unwrap_or_default();
        let repo_hint = claim
            .as_ref()
            .map(|current| current.repo_ref.clone())
            .or_else(|| {
                winner
                    .as_ref()
                    .map(|matched| matched.resolved.repo_ref.clone())
            });
        let workflow_hint = claim
            .as_ref()
            .map(|current| current.workflow.clone())
            .or_else(|| {
                winner
                    .as_ref()
                    .map(|matched| matched.resolved.workflow.name.clone())
            });

        let issue_result: anyhow::Result<bool> = async {
            if let Some(mut current) = claim {
                let previous_status = current.status;
                let active_lock = claim_store.read_active_lock(&issue_ref_text)?;
                let claim_expired = legacy::claim_lease_expired(&current)?;
                let owner_resolution = legacy::resolve_autoflow_workflow_for_repo(
                    &global,
                    &repo_store,
                    &current.repo_ref,
                    &current.workflow,
                );

                if owner_resolution.is_err() && (!claim_expired || active_lock.is_some()) {
                    return Ok(false);
                }
                if claim_expired && active_lock.is_none() && owner_resolution.is_err() {
                    current.status = ClaimStatus::Released;
                    current.updated_at = chrono::Utc::now().to_rfc3339();
                    claim_store.save(&current)?;
                    legacy::adjust_active_claim_count(
                        &mut active_claim_counts,
                        &current.repo_ref,
                        Some(previous_status),
                        Some(current.status),
                    );
                    if winner.is_none() {
                        return Ok(false);
                    }
                } else {
                    let mut resolved = owner_resolution?;
                    current.contenders = legacy::active_or_fallback_contenders(
                        &contenders,
                        Some(&resolved),
                        &current.workflow,
                    );
                    legacy::reconcile_claim_from_last_run(&global, &resolved, &mut current)?;

                    if legacy::claim_should_yield_to_winner(
                        &current,
                        winner.as_ref(),
                        active_lock.is_some(),
                    ) {
                        if let Some(winner) = winner.as_ref() {
                            current.contenders = legacy::active_or_fallback_contenders(
                                &contenders,
                                Some(&winner.resolved),
                                &winner.resolved.workflow.name,
                            );
                        }
                        current.status = ClaimStatus::Released;
                        current.pending_dispatch = None;
                        current.updated_at = chrono::Utc::now().to_rfc3339();
                        claim_store.save(&current)?;
                        legacy::adjust_active_claim_count(
                            &mut active_claim_counts,
                            &current.repo_ref,
                            Some(previous_status),
                            Some(current.status),
                        );
                    } else if current.status == ClaimStatus::Released {
                        claim_store.save(&current)?;
                        legacy::adjust_active_claim_count(
                            &mut active_claim_counts,
                            &current.repo_ref,
                            Some(previous_status),
                            Some(current.status),
                        );
                    } else if current.status == ClaimStatus::Complete
                        || current.status == ClaimStatus::Blocked
                    {
                        claim_store.save(&current)?;
                        legacy::adjust_active_claim_count(
                            &mut active_claim_counts,
                            &current.repo_ref,
                            Some(previous_status),
                            Some(current.status),
                        );
                        return Ok(false);
                    } else if let Some(dispatch) = current.pending_dispatch.clone() {
                        if !legacy::updated_before_tick(&current, tick_started_at)? {
                            claim_store.save(&current)?;
                            legacy::adjust_active_claim_count(
                                &mut active_claim_counts,
                                &current.repo_ref,
                                Some(previous_status),
                                Some(current.status),
                            );
                            return Ok(false);
                        }
                        resolved = legacy::resolve_autoflow_workflow_for_repo(
                            &global,
                            &repo_store,
                            &current.repo_ref,
                            &dispatch.workflow,
                        )?;
                        let issue = legacy::fetch_issue(
                            &resolved.cfg,
                            resolver.as_ref(),
                            &legacy::parse_issue_ref_text(&issue_ref_text)?,
                        )
                        .await?;
                        legacy::execute_autoflow_cycle(
                            &global,
                            &claim_store,
                            &resolved,
                            &issue,
                            &issue_ref_text,
                            None,
                            false,
                            dispatch.inputs,
                            current.contenders.clone(),
                        )
                        .await?;
                        legacy::adjust_active_claim_count(
                            &mut active_claim_counts,
                            &current.repo_ref,
                            Some(previous_status),
                            Some(load_claim_status(&claim_store, &issue_ref_text)?),
                        );
                        return Ok(true);
                    } else if legacy::should_run_claim(
                        &current,
                        &resolved,
                        &claim_store,
                        tick_started_at,
                        &wake_hints.events_for(&issue_ref_text, &current.repo_ref),
                    )? {
                        let issue = legacy::fetch_issue(
                            &resolved.cfg,
                            resolver.as_ref(),
                            &legacy::parse_issue_ref_text(&issue_ref_text)?,
                        )
                        .await?;
                        legacy::execute_autoflow_cycle(
                            &global,
                            &claim_store,
                            &resolved,
                            &issue,
                            &issue_ref_text,
                            None,
                            false,
                            BTreeMap::new(),
                            current.contenders.clone(),
                        )
                        .await?;
                        legacy::adjust_active_claim_count(
                            &mut active_claim_counts,
                            &current.repo_ref,
                            Some(previous_status),
                            Some(load_claim_status(&claim_store, &issue_ref_text)?),
                        );
                        return Ok(true);
                    } else {
                        claim_store.save(&current)?;
                        legacy::adjust_active_claim_count(
                            &mut active_claim_counts,
                            &current.repo_ref,
                            Some(previous_status),
                            Some(current.status),
                        );
                        return Ok(false);
                    }
                }
            }

            let Some(winner) = winner else {
                return Ok(false);
            };
            let max_active = winner.resolved.cfg.autoflow.max_active.unwrap_or(u32::MAX) as usize;
            let active = active_claim_counts
                .get(&winner.resolved.repo_ref)
                .copied()
                .unwrap_or_default();
            if active >= max_active {
                return Ok(false);
            }
            legacy::execute_autoflow_cycle(
                &global,
                &claim_store,
                &winner.resolved,
                &winner.issue,
                &winner.issue_ref_text,
                None,
                false,
                BTreeMap::new(),
                legacy::active_or_fallback_contenders(
                    &contenders,
                    Some(&winner.resolved),
                    &winner.resolved.workflow.name,
                ),
            )
            .await?;
            legacy::adjust_active_claim_count(
                &mut active_claim_counts,
                &winner.resolved.repo_ref,
                None,
                Some(load_claim_status(&claim_store, &winner.issue_ref_text)?),
            );
            Ok(true)
        }
        .await;

        match issue_result {
            Ok(true) => report.ran_cycles += 1,
            Ok(false) => report.skipped_cycles += 1,
            Err(error) => {
                report.failed_cycles += 1;
                tracing::warn!(
                    issue_ref = %issue_ref_text,
                    repo_ref = repo_hint.as_deref().unwrap_or("-"),
                    workflow = workflow_hint.as_deref().unwrap_or("-"),
                    %error,
                    "autoflow tick failed for issue"
                );
                resync_active_claim_counts(&claim_store, &mut active_claim_counts)?;
            }
        }
    }

    Ok(report)
}

fn load_claim_status(
    claim_store: &AutoflowClaimStore,
    issue_ref: &str,
) -> anyhow::Result<ClaimStatus> {
    Ok(claim_store
        .load(issue_ref)?
        .ok_or_else(|| anyhow::anyhow!("claim `{issue_ref}` disappeared during tick"))?
        .status)
}

fn resync_active_claim_counts(
    claim_store: &AutoflowClaimStore,
    active_claim_counts: &mut BTreeMap<String, usize>,
) -> anyhow::Result<()> {
    active_claim_counts.clear();
    for claim in claim_store.list()? {
        if legacy::claim_counts_toward_max_active(claim.status) {
            *active_claim_counts.entry(claim.repo_ref).or_insert(0) += 1;
        }
    }
    Ok(())
}
