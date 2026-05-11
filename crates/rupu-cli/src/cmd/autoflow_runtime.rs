use crate::cmd::autoflow as legacy;
use crate::cmd::workflow::{
    default_execution_worker_context, upsert_worker_record, ExecutionWorkerContext,
};
use crate::paths;
use anyhow::Context;
use rupu_auth::CredentialResolver;
use rupu_runtime::{
    AutoflowCycleEvent, AutoflowCycleEventKind, AutoflowCycleMode, AutoflowCycleRecord,
    AutoflowHistoryStore, WakeEnqueueRequest, WakeEntity, WakeEntityKind, WakeEvent, WakeSource,
    WakeStore,
};
use rupu_workspace::{
    AutoflowClaimRecord, AutoflowClaimStore, ClaimStatus, RepoRegistryStore, WorkerKind,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
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

#[derive(Debug, Clone, Default)]
pub struct TickOptions {
    pub repo_filter: Option<String>,
    pub worker: Option<ExecutionWorkerContext>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeOptions {
    pub repo_filter: Option<String>,
    pub worker_name: Option<String>,
    pub idle_sleep: std::time::Duration,
    pub max_cycles: Option<usize>,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            repo_filter: None,
            worker_name: None,
            idle_sleep: std::time::Duration::from_secs(10),
            max_cycles: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServeReport {
    pub cycles: usize,
    pub total: TickReport,
}

pub(crate) async fn tick_with_resolver(
    resolver: Arc<dyn CredentialResolver>,
) -> anyhow::Result<TickReport> {
    tick_with_options(resolver, TickOptions::default()).await
}

pub(crate) async fn tick_with_options(
    resolver: Arc<dyn CredentialResolver>,
    options: TickOptions,
) -> anyhow::Result<TickReport> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let tick_started_at = chrono::Utc::now();
    let history_store = AutoflowHistoryStore::new(paths::autoflow_history_dir(&global));
    let mut cycle_record = AutoflowCycleRecord::new(cycle_mode(&options), tick_started_at);
    cycle_record.repo_filter = options.repo_filter.clone();
    if let Some(worker) = options.worker.as_ref() {
        cycle_record.worker_id = Some(worker.worker_id.clone());
        cycle_record.worker_name = Some(worker.name.clone());
    }

    let result: anyhow::Result<TickReport> = async {
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        let wake_store = WakeStore::new(paths::autoflow_wakes_dir(&global));
        let cleaned = legacy::cleanup_terminal_claims(
            &global,
            &repo_store,
            &claim_store,
            chrono::Utc::now(),
            options.repo_filter.as_deref(),
        )?;
        if cleaned > 0 {
            cycle_record.events.push(AutoflowCycleEvent {
                kind: AutoflowCycleEventKind::CleanupPerformed,
                detail: Some(format!("cleaned {cleaned} terminal claim(s)")),
                ..Default::default()
            });
        }
        let mut discovered = legacy::discover_tick_autoflows(&global, &repo_store)?;
        if let Some(repo_filter) = options.repo_filter.as_deref() {
            discovered.retain(|resolved| resolved.repo_ref == repo_filter);
        }

        let mut report = TickReport {
            workflow_count: discovered.len(),
            cleaned_claims: cleaned,
            ..TickReport::default()
        };
        if discovered.is_empty() {
            return Ok(report);
        }

        let wake_hints = legacy::collect_wake_hints(&global, &discovered, resolver.as_ref())
            .await
            .context("collect autoflow wake hints")?;
        report.polled_event_count = wake_hints.total_polled_events;
        report.webhook_event_count = wake_hints.total_webhook_events;

        let matches = legacy::collect_issue_matches(&discovered, resolver.as_ref())
            .await
            .context("discover autoflow issue matches")?;
        let contenders_by_issue = legacy::summarize_issue_contenders(&matches);
        let winners = legacy::choose_winning_matches(matches);
        let mut claims_by_issue: BTreeMap<String, AutoflowClaimRecord> = claim_store
            .list()?
            .into_iter()
            .filter(|claim| repo_filter_matches(options.repo_filter.as_deref(), &claim.repo_ref))
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
            let before_claim = claim.clone();

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
                                options.worker.clone(),
                            )
                            .await?;
                            enqueue_follow_up_wake(&global, &claim_store, &issue_ref_text)?;
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
                                options.worker.clone(),
                            )
                            .await?;
                            enqueue_follow_up_wake(&global, &claim_store, &issue_ref_text)?;
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
                let max_active =
                    winner.resolved.cfg.autoflow.max_active.unwrap_or(u32::MAX) as usize;
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
                    options.worker.clone(),
                )
                .await?;
                enqueue_follow_up_wake(&global, &claim_store, &winner.issue_ref_text)?;
                legacy::adjust_active_claim_count(
                    &mut active_claim_counts,
                    &winner.resolved.repo_ref,
                    None,
                    Some(load_claim_status(&claim_store, &winner.issue_ref_text)?),
                );
                Ok(true)
            }
            .await;

            let after_claim = claim_store.load(&issue_ref_text)?;
            append_issue_history_events(
                &mut cycle_record.events,
                &issue_result,
                &issue_ref_text,
                before_claim.as_ref(),
                after_claim.as_ref(),
                repo_hint.as_deref(),
                workflow_hint.as_deref(),
            );

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
                    resync_active_claim_counts(
                        &claim_store,
                        &mut active_claim_counts,
                        options.repo_filter.as_deref(),
                    )?;
                }
            }
        }

        for wake_id in &wake_hints.due_wake_ids {
            if let Ok(wake) = wake_store.load(wake_id) {
                cycle_record.events.push(AutoflowCycleEvent {
                    kind: AutoflowCycleEventKind::WakeConsumed,
                    issue_ref: (wake.entity.kind == WakeEntityKind::Issue)
                        .then(|| wake.entity.ref_text.clone()),
                    repo_ref: Some(wake.repo_ref.clone()),
                    wake_id: Some(wake.wake_id.clone()),
                    wake_event_id: Some(wake.event.id.clone()),
                    detail: Some(format!("{:?}", wake.source).to_lowercase()),
                    ..Default::default()
                });
            }
            if let Err(error) = wake_store.mark_processed(wake_id) {
                tracing::warn!(wake_id, %error, "failed to mark wake processed");
            }
        }

        Ok(report)
    }
    .await;

    match &result {
        Ok(report) => {
            finalize_cycle_record(&mut cycle_record, report, chrono::Utc::now());
        }
        Err(error) => {
            cycle_record.finished_at = chrono::Utc::now().to_rfc3339();
            cycle_record.failed_cycles = 1;
            cycle_record.events.push(AutoflowCycleEvent {
                kind: AutoflowCycleEventKind::CycleFailed,
                detail: Some(error.to_string()),
                ..Default::default()
            });
        }
    }
    if let Err(error) = history_store.save(&cycle_record) {
        tracing::warn!(%error, cycle_id = %cycle_record.cycle_id, "failed to persist autoflow cycle history");
    }

    result
}

pub(crate) async fn serve_with_resolver(
    resolver: Arc<dyn CredentialResolver>,
    options: ServeOptions,
) -> anyhow::Result<ServeReport> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let worker = ServeWorker::acquire(
        &global,
        options.worker_name.as_deref(),
        options.repo_filter.as_deref(),
    )?;
    let mut report = ServeReport::default();
    let mut cycle_index = 0usize;

    loop {
        cycle_index += 1;
        worker.heartbeat(options.repo_filter.as_deref())?;
        let tick = tick_with_options(
            Arc::clone(&resolver),
            TickOptions {
                repo_filter: options.repo_filter.clone(),
                worker: Some(worker.execution_worker()),
            },
        )
        .await?;
        report.cycles += 1;
        accumulate_tick_report(&mut report.total, &tick);

        if options.max_cycles.is_some_and(|max| cycle_index >= max) {
            break;
        }

        let sleep_for = if tick.ran_cycles > 0 || tick.failed_cycles > 0 {
            std::cmp::min(options.idle_sleep, std::time::Duration::from_millis(250))
        } else {
            options.idle_sleep
        };
        if sleep_for.is_zero() {
            continue;
        }

        tokio::select! {
            _ = wait_for_shutdown() => break,
            _ = tokio::time::sleep(sleep_for) => {}
        }
    }

    Ok(report)
}

fn repo_filter_matches(repo_filter: Option<&str>, repo_ref: &str) -> bool {
    repo_filter.is_none_or(|filter| filter == repo_ref)
}

fn accumulate_tick_report(total: &mut TickReport, tick: &TickReport) {
    total.workflow_count = tick.workflow_count;
    total.polled_event_count += tick.polled_event_count;
    total.webhook_event_count += tick.webhook_event_count;
    total.ran_cycles += tick.ran_cycles;
    total.skipped_cycles += tick.skipped_cycles;
    total.failed_cycles += tick.failed_cycles;
    total.cleaned_claims += tick.cleaned_claims;
}

fn cycle_mode(options: &TickOptions) -> AutoflowCycleMode {
    match options.worker.as_ref().map(|worker| worker.kind) {
        Some(WorkerKind::AutoflowServe) => AutoflowCycleMode::Serve,
        _ => AutoflowCycleMode::Tick,
    }
}

fn finalize_cycle_record(
    record: &mut AutoflowCycleRecord,
    report: &TickReport,
    finished_at: chrono::DateTime<chrono::Utc>,
) {
    record.finished_at = finished_at.to_rfc3339();
    record.workflow_count = report.workflow_count;
    record.polled_event_count = report.polled_event_count;
    record.webhook_event_count = report.webhook_event_count;
    record.ran_cycles = report.ran_cycles;
    record.skipped_cycles = report.skipped_cycles;
    record.failed_cycles = report.failed_cycles;
    record.cleaned_claims = report.cleaned_claims;
}

fn append_issue_history_events(
    events: &mut Vec<AutoflowCycleEvent>,
    issue_result: &anyhow::Result<bool>,
    issue_ref_text: &str,
    before: Option<&AutoflowClaimRecord>,
    after: Option<&AutoflowClaimRecord>,
    repo_hint: Option<&str>,
    workflow_hint: Option<&str>,
) {
    if let Err(error) = issue_result {
        events.push(AutoflowCycleEvent {
            kind: AutoflowCycleEventKind::CycleFailed,
            issue_ref: Some(issue_ref_text.to_string()),
            repo_ref: repo_hint.map(ToOwned::to_owned),
            workflow: workflow_hint.map(ToOwned::to_owned),
            detail: Some(error.to_string()),
            ..Default::default()
        });
    }

    let Some(after) = after else {
        return;
    };
    let issue_display_ref = after.issue_display_ref.clone();
    let repo_ref = Some(after.repo_ref.clone());
    let source_ref = after.source_ref.clone();
    let workflow = Some(after.workflow.clone());

    if before.is_none() {
        events.push(AutoflowCycleEvent {
            kind: AutoflowCycleEventKind::ClaimAcquired,
            issue_ref: Some(issue_ref_text.to_string()),
            issue_display_ref: issue_display_ref.clone(),
            repo_ref: repo_ref.clone(),
            source_ref: source_ref.clone(),
            workflow: workflow.clone(),
            status: Some(claim_status_name(after.status).to_string()),
            ..Default::default()
        });
    } else if before.is_some_and(|before| before.workflow != after.workflow) {
        events.push(AutoflowCycleEvent {
            kind: AutoflowCycleEventKind::ClaimTakeover,
            issue_ref: Some(issue_ref_text.to_string()),
            issue_display_ref: issue_display_ref.clone(),
            repo_ref: repo_ref.clone(),
            source_ref: source_ref.clone(),
            workflow: workflow.clone(),
            status: Some(claim_status_name(after.status).to_string()),
            ..Default::default()
        });
    }

    let before_run_id = before.and_then(|claim| claim.last_run_id.as_deref());
    let after_run_id = after.last_run_id.as_deref();
    if after_run_id.is_some() && after_run_id != before_run_id {
        events.push(AutoflowCycleEvent {
            kind: AutoflowCycleEventKind::RunLaunched,
            issue_ref: Some(issue_ref_text.to_string()),
            issue_display_ref: issue_display_ref.clone(),
            repo_ref: repo_ref.clone(),
            source_ref: source_ref.clone(),
            workflow: workflow.clone(),
            run_id: after_run_id.map(ToOwned::to_owned),
            status: Some(claim_status_name(after.status).to_string()),
            ..Default::default()
        });
    }

    if before.and_then(|claim| claim.pending_dispatch.as_ref()) != after.pending_dispatch.as_ref()
        && after.pending_dispatch.is_some()
    {
        events.push(AutoflowCycleEvent {
            kind: AutoflowCycleEventKind::DispatchQueued,
            issue_ref: Some(issue_ref_text.to_string()),
            issue_display_ref: issue_display_ref.clone(),
            repo_ref: repo_ref.clone(),
            source_ref: source_ref.clone(),
            workflow: workflow.clone(),
            detail: after
                .pending_dispatch
                .as_ref()
                .map(|dispatch| format!("{} target={}", dispatch.workflow, dispatch.target)),
            ..Default::default()
        });
    }

    if before.and_then(|claim| claim.next_retry_at.as_deref()) != after.next_retry_at.as_deref()
        && after.next_retry_at.is_some()
    {
        events.push(AutoflowCycleEvent {
            kind: AutoflowCycleEventKind::RetryScheduled,
            issue_ref: Some(issue_ref_text.to_string()),
            issue_display_ref: issue_display_ref.clone(),
            repo_ref: repo_ref.clone(),
            source_ref: source_ref.clone(),
            workflow: workflow.clone(),
            detail: after.next_retry_at.clone(),
            ..Default::default()
        });
    }

    let before_status = before.map(|claim| claim.status);
    if Some(after.status) != before_status {
        let kind = match after.status {
            ClaimStatus::AwaitHuman => Some(AutoflowCycleEventKind::AwaitingHuman),
            ClaimStatus::AwaitExternal => Some(AutoflowCycleEventKind::AwaitingExternal),
            ClaimStatus::Released => Some(AutoflowCycleEventKind::ClaimReleased),
            _ => None,
        };
        if let Some(kind) = kind {
            events.push(AutoflowCycleEvent {
                kind,
                issue_ref: Some(issue_ref_text.to_string()),
                issue_display_ref,
                repo_ref,
                source_ref,
                workflow,
                status: Some(claim_status_name(after.status).to_string()),
                ..Default::default()
            });
        }
    }
}

fn claim_status_name(status: ClaimStatus) -> &'static str {
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

struct ServeWorker {
    global: PathBuf,
    worker: ExecutionWorkerContext,
    lock_path: PathBuf,
}

impl ServeWorker {
    fn acquire(
        global: &Path,
        worker_name: Option<&str>,
        repo_filter: Option<&str>,
    ) -> anyhow::Result<Self> {
        let worker = default_execution_worker_context(WorkerKind::AutoflowServe, worker_name);
        let workers_dir = paths::autoflow_workers_dir(global);
        std::fs::create_dir_all(&workers_dir)?;
        let lock_path = workers_dir.join(format!("{}.serve.lock", worker.worker_id));
        let lock_body = serde_json::json!({
            "worker_id": worker.worker_id,
            "name": worker.name,
            "repo_filter": repo_filter,
            "pid": std::process::id(),
            "started_at": chrono::Utc::now().to_rfc3339(),
        });
        let mut lock = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("acquire serve lock {}", lock_path.display()))?;
        use std::io::Write;
        lock.write_all(serde_json::to_string_pretty(&lock_body)?.as_bytes())?;

        let worker_ref = Self {
            global: global.to_path_buf(),
            worker,
            lock_path,
        };
        worker_ref.heartbeat(repo_filter)?;
        Ok(worker_ref)
    }

    fn execution_worker(&self) -> ExecutionWorkerContext {
        self.worker.clone()
    }

    fn heartbeat(&self, repo_filter: Option<&str>) -> anyhow::Result<()> {
        let _ = upsert_worker_record(
            &self.global,
            &self.worker,
            "local_worktree",
            "bypass",
            repo_filter,
        );
        let _ = upsert_worker_record(
            &self.global,
            &self.worker,
            "local_worktree",
            "readonly",
            repo_filter,
        );
        Ok(())
    }
}

impl Drop for ServeWorker {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

fn enqueue_follow_up_wake(
    global: &Path,
    claim_store: &AutoflowClaimStore,
    issue_ref: &str,
) -> anyhow::Result<()> {
    let Some(claim) = claim_store.load(issue_ref)? else {
        return Ok(());
    };
    let Some(request) = follow_up_wake_request(&claim)? else {
        return Ok(());
    };
    let store = WakeStore::new(paths::autoflow_wakes_dir(global));
    match store.enqueue(request) {
        Ok(_) => {}
        Err(rupu_runtime::WakeStoreError::DuplicateDedupeKey(_)) => {}
        Err(error) => return Err(anyhow::Error::from(error)),
    }
    Ok(())
}

fn follow_up_wake_request(
    claim: &AutoflowClaimRecord,
) -> anyhow::Result<Option<WakeEnqueueRequest>> {
    let (source, event_id, not_before) = match claim.status {
        ClaimStatus::RetryBackoff => (
            WakeSource::Retry,
            "autoflow.retry.due",
            claim
                .next_retry_at
                .clone()
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
        ),
        ClaimStatus::AwaitHuman => (
            WakeSource::ApprovalResume,
            "autoflow.approval.resume",
            (chrono::Utc::now() + chrono::Duration::seconds(30)).to_rfc3339(),
        ),
        ClaimStatus::Claimed if claim.pending_dispatch.is_some() => (
            WakeSource::AutoflowDispatch,
            "autoflow.dispatch.pending",
            chrono::Utc::now().to_rfc3339(),
        ),
        _ => return Ok(None),
    };
    Ok(Some(WakeEnqueueRequest {
        source,
        repo_ref: claim.repo_ref.clone(),
        entity: WakeEntity {
            kind: WakeEntityKind::Issue,
            ref_text: claim.issue_ref.clone(),
        },
        event: WakeEvent {
            id: event_id.to_string(),
            delivery_id: None,
            dedupe_key: Some(format!(
                "{}:{}:{}",
                event_id, claim.issue_ref, claim.updated_at
            )),
        },
        payload: None,
        received_at: chrono::Utc::now().to_rfc3339(),
        not_before,
    }))
}

async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = terminate.recv() => {},
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
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
    repo_filter: Option<&str>,
) -> anyhow::Result<()> {
    active_claim_counts.clear();
    for claim in claim_store.list()? {
        if !repo_filter_matches(repo_filter, &claim.repo_ref) {
            continue;
        }
        if legacy::claim_counts_toward_max_active(claim.status) {
            *active_claim_counts.entry(claim.repo_ref).or_insert(0) += 1;
        }
    }
    Ok(())
}
