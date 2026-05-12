use crate::cmd::autoflow as legacy;
use crate::cmd::workflow::{
    default_execution_worker_context, upsert_worker_record, ExecutionWorkerContext,
};
use crate::output::LineStreamPrinter;
use crate::output::workflow_printer::tool_summary;
use crate::paths;
use anyhow::Context;
use rupu_auth::CredentialResolver;
use rupu_orchestrator::RunStore;
use rupu_runtime::{
    AutoflowCycleEvent, AutoflowCycleEventKind, AutoflowCycleMode, AutoflowCycleRecord,
    AutoflowHistoryStore, WakeEnqueueRequest, WakeEntity, WakeEntityKind, WakeEvent, WakeSource,
    WakeStore,
};
use rupu_transcript::{Event as TranscriptEvent, JsonlReader};
use rupu_workspace::{
    AutoflowClaimRecord, AutoflowClaimStore, ClaimStatus, RepoRegistryStore, WorkerKind,
};
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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

#[derive(Debug, Clone)]
pub struct TickOutcome {
    pub report: TickReport,
    pub cycle: AutoflowCycleRecord,
}

#[derive(Clone, Default)]
pub struct TickOptions {
    pub repo_filter: Option<String>,
    pub worker: Option<ExecutionWorkerContext>,
    pub shared_printer: Option<Arc<Mutex<LineStreamPrinter>>>,
}

#[derive(Clone)]
pub struct ServeOptions {
    pub repo_filter: Option<String>,
    pub worker_name: Option<String>,
    pub idle_sleep: std::time::Duration,
    pub max_cycles: Option<usize>,
    pub shared_printer: Option<Arc<Mutex<LineStreamPrinter>>>,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            repo_filter: None,
            worker_name: None,
            idle_sleep: std::time::Duration::from_secs(10),
            max_cycles: None,
            shared_printer: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServeReport {
    pub cycles: usize,
    pub total: TickReport,
}

#[derive(Debug, Clone)]
pub(crate) struct LiveCycleRecorder {
    history_store: AutoflowHistoryStore,
    seed: AutoflowCycleRecord,
    events: Arc<Mutex<Vec<AutoflowCycleEvent>>>,
    appended: Arc<Mutex<BTreeSet<String>>>,
}

impl LiveCycleRecorder {
    fn new(history_store: AutoflowHistoryStore, seed: AutoflowCycleRecord) -> Self {
        Self {
            history_store,
            seed,
            events: Arc::new(Mutex::new(Vec::new())),
            appended: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    pub(crate) fn record_event(&self, event: AutoflowCycleEvent) -> anyhow::Result<()> {
        let key = cycle_event_key(&event);
        {
            let appended = self
                .appended
                .lock()
                .map_err(|_| anyhow::anyhow!("live cycle recorder poisoned"))?;
            if appended.contains(&key) {
                return Ok(());
            }
        }
        let mut events = self
            .events
            .lock()
            .map_err(|_| anyhow::anyhow!("live cycle recorder poisoned"))?;
        events.push(event.clone());
        dedupe_cycle_events(&mut events);
        drop(events);
        self.history_store
            .append_cycle_event(&self.seed, event, chrono::Utc::now())?;
        self.appended
            .lock()
            .map_err(|_| anyhow::anyhow!("live cycle recorder poisoned"))?
            .insert(key);
        Ok(())
    }

    fn snapshot_events(&self) -> Vec<AutoflowCycleEvent> {
        self.events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }

    fn append_missing_events(&self, cycle: &AutoflowCycleRecord) -> anyhow::Result<()> {
        let mut appended = self
            .appended
            .lock()
            .map_err(|_| anyhow::anyhow!("live cycle recorder poisoned"))?;
        for event in &cycle.events {
            let key = cycle_event_key(event);
            if appended.contains(&key) {
                continue;
            }
            self.history_store
                .append_cycle_event(cycle, event.clone(), chrono::Utc::now())?;
            appended.insert(key);
        }
        Ok(())
    }
}

pub(crate) async fn tick_with_resolver(
    resolver: Arc<dyn CredentialResolver>,
) -> anyhow::Result<TickReport> {
    Ok(tick_with_options(resolver, TickOptions::default())
        .await?
        .report)
}

pub(crate) async fn tick_with_options(
    resolver: Arc<dyn CredentialResolver>,
    options: TickOptions,
) -> anyhow::Result<TickOutcome> {
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

    let serve_mode = matches!(cycle_mode(&options), AutoflowCycleMode::Serve);
    let live_cycle_recorder = serve_mode.then(|| {
        Arc::new(LiveCycleRecorder::new(
            history_store.clone(),
            cycle_record.clone(),
        ))
    });
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
                            let issue = legacy::fetch_issue(
                                &resolved.cfg,
                                resolver.as_ref(),
                                &legacy::parse_issue_ref_text(&issue_ref_text)?,
                            )
                            .await?;
                            if legacy::workflow_declares_autoflow_for_repo(
                                &global,
                                &repo_store,
                                &current.repo_ref,
                                &dispatch.workflow,
                            )? {
                                resolved = legacy::resolve_autoflow_workflow_for_repo(
                                    &global,
                                    &repo_store,
                                    &current.repo_ref,
                                    &dispatch.workflow,
                                )?;
                                legacy::execute_autoflow_cycle(
                                    &global,
                                    &claim_store,
                                    &resolved,
                                    &issue,
                                    &issue_ref_text,
                                    None,
                                    serve_mode,
                                    dispatch.inputs,
                                    current.contenders.clone(),
                                    options.worker.clone(),
                                    options.shared_printer.clone(),
                                    live_cycle_recorder.clone(),
                                )
                                .await?;
                            } else {
                                legacy::execute_pending_dispatch_workflow(
                                    &global,
                                    &repo_store,
                                    &claim_store,
                                    &resolved,
                                    &mut current,
                                    &issue,
                                    &issue_ref_text,
                                    &dispatch.workflow,
                                    dispatch.inputs,
                                    serve_mode,
                                    options.worker.clone(),
                                    options.shared_printer.clone(),
                                    live_cycle_recorder.clone(),
                                )
                                .await?;
                            }
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
                                serve_mode,
                                BTreeMap::new(),
                                current.contenders.clone(),
                                options.worker.clone(),
                                options.shared_printer.clone(),
                                live_cycle_recorder.clone(),
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
                    serve_mode,
                    BTreeMap::new(),
                    legacy::active_or_fallback_contenders(
                        &contenders,
                        Some(&winner.resolved),
                        &winner.resolved.workflow.name,
                    ),
                    options.worker.clone(),
                    options.shared_printer.clone(),
                    live_cycle_recorder.clone(),
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
                &global,
            );

            match issue_result {
                Ok(true) => report.ran_cycles += 1,
                Ok(false) => report.skipped_cycles += 1,
                Err(error) => {
                    report.failed_cycles += 1;
                    if serve_mode {
                        tracing::debug!(
                            issue_ref = %issue_ref_text,
                            repo_ref = repo_hint.as_deref().unwrap_or("-"),
                            workflow = workflow_hint.as_deref().unwrap_or("-"),
                            %error,
                            "autoflow tick failed for issue"
                        );
                    } else {
                        tracing::warn!(
                            issue_ref = %issue_ref_text,
                            repo_ref = repo_hint.as_deref().unwrap_or("-"),
                            workflow = workflow_hint.as_deref().unwrap_or("-"),
                            %error,
                            "autoflow tick failed for issue"
                        );
                    }
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
    if let Some(recorder) = live_cycle_recorder.as_ref() {
        cycle_record.events.extend(recorder.snapshot_events());
        dedupe_cycle_events(&mut cycle_record.events);
        if let Err(error) = recorder.append_missing_events(&cycle_record) {
            tracing::warn!(
                %error,
                cycle_id = %cycle_record.cycle_id,
                "failed to append autoflow event history"
            );
        }
    } else {
        for event in &cycle_record.events {
            if let Err(error) =
                history_store.append_cycle_event(&cycle_record, event.clone(), chrono::Utc::now())
            {
                tracing::warn!(
                    %error,
                    cycle_id = %cycle_record.cycle_id,
                    "failed to append autoflow event history"
                );
            }
        }
    }
    if let Err(error) = history_store.save(&cycle_record) {
        tracing::warn!(%error, cycle_id = %cycle_record.cycle_id, "failed to persist autoflow cycle history");
    }

    result.map(|report| TickOutcome {
        report,
        cycle: cycle_record,
    })
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn serve_with_resolver(
    resolver: Arc<dyn CredentialResolver>,
    options: ServeOptions,
) -> anyhow::Result<ServeReport> {
    serve_with_resolver_and_hook(resolver, options, |_, _, _| Ok(())).await
}

pub(crate) async fn serve_with_resolver_and_hook<F>(
    resolver: Arc<dyn CredentialResolver>,
    options: ServeOptions,
    mut on_cycle: F,
) -> anyhow::Result<ServeReport>
where
    F: FnMut(&ServeReport, &TickReport, &AutoflowCycleRecord) -> anyhow::Result<()>,
{
    serve_with_resolver_and_hooks(
        resolver,
        options,
        || Ok(()),
        move |report, tick, cycle| on_cycle(report, tick, cycle),
    )
    .await
}

pub(crate) async fn serve_with_resolver_and_hooks<F, G>(
    resolver: Arc<dyn CredentialResolver>,
    options: ServeOptions,
    mut on_cycle_start: G,
    mut on_cycle: F,
) -> anyhow::Result<ServeReport>
where
    F: FnMut(&ServeReport, &TickReport, &AutoflowCycleRecord) -> anyhow::Result<()>,
    G: FnMut() -> anyhow::Result<()>,
{
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
        on_cycle_start()?;
        let tick = tick_with_options(
            Arc::clone(&resolver),
            TickOptions {
                repo_filter: options.repo_filter.clone(),
                worker: Some(worker.execution_worker()),
                shared_printer: options.shared_printer.clone(),
            },
        )
        .await?;
        let tick_report = tick.report;
        let cycle_record = tick.cycle;
        report.cycles += 1;
        accumulate_tick_report(&mut report.total, &tick_report);
        on_cycle(&report, &tick_report, &cycle_record)?;

        if options.max_cycles.is_some_and(|max| cycle_index >= max) {
            break;
        }

        let sleep_for = if tick_report.ran_cycles > 0 || tick_report.failed_cycles > 0 {
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
    global: &Path,
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
        append_run_action_events(events, global, issue_ref_text, after);
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

fn append_run_action_events(
    events: &mut Vec<AutoflowCycleEvent>,
    global: &Path,
    issue_ref_text: &str,
    claim: &AutoflowClaimRecord,
) {
    let Some(run_id) = claim.last_run_id.as_deref() else {
        return;
    };
    let run_store = RunStore::new(global.join("runs"));
    let Ok(rows) = run_store.read_step_results(run_id) else {
        return;
    };
    let mut seen_paths = BTreeSet::new();
    for row in rows {
        append_transcript_action_events(
            events,
            issue_ref_text,
            claim,
            run_id,
            &row.transcript_path,
            &mut seen_paths,
        );
        for item in row.items {
            append_transcript_action_events(
                events,
                issue_ref_text,
                claim,
                run_id,
                &item.transcript_path,
                &mut seen_paths,
            );
        }
    }
    if let Some(pr_url) = claim.pr_url.as_deref() {
        let pr_already_recorded = events.iter().any(|event| {
            event.kind == AutoflowCycleEventKind::PullRequestOpened
                && event.run_id.as_deref() == Some(run_id)
        });
        if !pr_already_recorded {
            events.push(AutoflowCycleEvent {
                kind: AutoflowCycleEventKind::PullRequestOpened,
                issue_ref: Some(issue_ref_text.to_string()),
                issue_display_ref: claim.issue_display_ref.clone(),
                repo_ref: Some(claim.repo_ref.clone()),
                source_ref: claim.source_ref.clone(),
                workflow: Some(claim.workflow.clone()),
                run_id: Some(run_id.to_string()),
                status: Some("open".into()),
                detail: Some(pr_url.to_string()),
                ..Default::default()
            });
        }
    }
}

fn append_transcript_action_events(
    events: &mut Vec<AutoflowCycleEvent>,
    issue_ref_text: &str,
    claim: &AutoflowClaimRecord,
    run_id: &str,
    transcript_path: &Path,
    seen_paths: &mut BTreeSet<PathBuf>,
) {
    if transcript_path.as_os_str().is_empty() || !transcript_path.exists() {
        return;
    }
    if !seen_paths.insert(transcript_path.to_path_buf()) {
        return;
    }
    let Ok(iter) = JsonlReader::iter(transcript_path) else {
        return;
    };
    let mut pending = BTreeMap::<String, (String, JsonValue)>::new();
    for event in iter.flatten() {
        match event {
            TranscriptEvent::ToolCall {
                call_id,
                tool,
                input,
                ..
            } if is_promoted_autoflow_tool(&tool) => {
                pending.insert(call_id, (tool, input));
            }
            TranscriptEvent::ToolResult { call_id, error, .. } => {
                let Some((tool, input)) = pending.remove(&call_id) else {
                    continue;
                };
                if error.is_some() {
                    continue;
                }
                if let Some(action_event) =
                    promoted_autoflow_tool_event(&tool, &input, claim, run_id, issue_ref_text)
                {
                    events.push(action_event);
                }
            }
            _ => {}
        }
    }
}

fn is_promoted_autoflow_tool(tool: &str) -> bool {
    matches!(
        tool,
        "issues.comment" | "issues.update_state" | "scm.prs.create"
    )
}

pub(crate) fn promoted_autoflow_tool_event(
    tool: &str,
    input: &JsonValue,
    claim: &AutoflowClaimRecord,
    run_id: &str,
    issue_ref_text: &str,
) -> Option<AutoflowCycleEvent> {
    let issue_ref = issue_target_ref(input, claim).unwrap_or_else(|| issue_ref_text.to_string());
    let issue_display_ref = if issue_ref == issue_ref_text {
        claim.issue_display_ref.clone()
    } else {
        issue_target_display(input)
    };
    let mut event = AutoflowCycleEvent {
        issue_ref: Some(issue_ref),
        issue_display_ref,
        repo_ref: Some(claim.repo_ref.clone()),
        source_ref: claim.source_ref.clone(),
        workflow: Some(claim.workflow.clone()),
        run_id: Some(run_id.to_string()),
        ..Default::default()
    };
    match tool {
        "issues.comment" => {
            event.kind = AutoflowCycleEventKind::IssueCommented;
            event.detail = comment_preview(input)
                .or_else(|| Some(tool_summary(tool, input)))
                .filter(|detail| !detail.is_empty());
            Some(event)
        }
        "issues.update_state" => {
            let state = input.get("state").and_then(|value| value.as_str())?;
            event.kind = AutoflowCycleEventKind::IssueStateChanged;
            event.status = Some(state.to_string());
            event.detail = issue_state_detail(state);
            Some(event)
        }
        "scm.prs.create" => {
            event.kind = AutoflowCycleEventKind::PullRequestOpened;
            event.status = Some(
                if input
                    .get("draft")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
                {
                    "draft".into()
                } else {
                    "open".into()
                },
            );
            event.detail = pull_request_detail(input)
                .or_else(|| Some(tool_summary(tool, input)))
                .filter(|detail| !detail.is_empty());
            Some(event)
        }
        _ => None,
    }
}

fn issue_target_ref(input: &JsonValue, claim: &AutoflowClaimRecord) -> Option<String> {
    let project = input.get("project").and_then(|value| value.as_str())?;
    let number = input.get("number").and_then(|value| value.as_u64())?;
    let tracker = input
        .get("tracker")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .or_else(|| {
            claim
                .issue_ref
                .split_once(':')
                .map(|(tracker, _)| tracker.to_string())
        })?;
    Some(format!("{tracker}:{project}/issues/{number}"))
}

fn issue_target_display(input: &JsonValue) -> Option<String> {
    let project = input.get("project").and_then(|value| value.as_str())?;
    let number = input.get("number").and_then(|value| value.as_u64())?;
    Some(format!("{project}#{number}"))
}

fn comment_preview(input: &JsonValue) -> Option<String> {
    let body = input.get("body").and_then(|value| value.as_str())?;
    let first_line = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if first_line.is_empty() {
        return None;
    }
    Some(format!("comment: {}", truncate_detail(first_line, 96)))
}

fn issue_state_detail(state: &str) -> Option<String> {
    match state {
        "closed" | "open" => None,
        other if !other.trim().is_empty() => Some(format!("state {}", truncate_detail(other, 48))),
        _ => None,
    }
}

fn pull_request_detail(input: &JsonValue) -> Option<String> {
    let title = input
        .get("title")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let base = input
        .get("base")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let draft = input
        .get("draft")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let mut parts = Vec::new();
    if !title.trim().is_empty() {
        parts.push(truncate_detail(title.trim(), 72));
    }
    if !base.trim().is_empty() {
        parts.push(format!("→ {}", truncate_detail(base.trim(), 24)));
    }
    if draft {
        parts.push("draft".into());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("  ·  "))
    }
}

fn truncate_detail(value: &str, max_chars: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max_chars {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

fn dedupe_cycle_events(events: &mut Vec<AutoflowCycleEvent>) {
    let mut seen = BTreeSet::new();
    events.retain(|event| {
        let key = cycle_event_key(event);
        seen.insert(key)
    });
}

fn cycle_event_key(event: &AutoflowCycleEvent) -> String {
    serde_json::to_string(event).unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rupu_orchestrator::{RunRecord, RunStatus, StepKind, StepResultRecord};
    use rupu_transcript::{Event, JsonlWriter, RunMode, RunStatus as TranscriptRunStatus};

    #[test]
    fn append_issue_history_events_promotes_tool_actions_from_run_transcripts() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        std::fs::create_dir_all(global.join("runs")).unwrap();
        std::fs::create_dir_all(global.join("transcripts")).unwrap();

        let run_store = RunStore::new(global.join("runs"));
        run_store
            .create(
                RunRecord {
                    id: "run_123".into(),
                    workflow_name: "storefront-feature-delivery".into(),
                    status: RunStatus::Completed,
                    inputs: BTreeMap::new(),
                    event: None,
                    workspace_id: "ws_1".into(),
                    workspace_path: tmp.path().join("repo"),
                    transcript_dir: global.join("transcripts"),
                    started_at: Utc::now(),
                    finished_at: Some(Utc::now()),
                    error_message: None,
                    awaiting_step_id: None,
                    approval_prompt: None,
                    awaiting_since: None,
                    expires_at: None,
                    issue_ref: Some("github:Section9Labs/rupu-sandbox-gh/issues/10".into()),
                    issue: None,
                    parent_run_id: None,
                    backend_id: None,
                    worker_id: None,
                    artifact_manifest_path: None,
                    source_wake_id: None,
                    active_step_id: None,
                    active_step_kind: None,
                    active_step_agent: None,
                    active_step_transcript_path: None,
                },
                "name: demo\nsteps: []\n",
            )
            .unwrap();

        let transcript_path = global.join("transcripts/run_123_step.jsonl");
        let mut writer = JsonlWriter::create(&transcript_path).unwrap();
        writer
            .write(&Event::RunStart {
                run_id: "step_run_1".into(),
                workspace_id: "ws_1".into(),
                agent: "builder".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
                started_at: Utc::now(),
                mode: RunMode::Bypass,
            })
            .unwrap();
        writer
            .write(&Event::ToolCall {
                call_id: "call_comment".into(),
                tool: "issues.comment".into(),
                input: serde_json::json!({
                    "project": "Section9Labs/rupu-sandbox-gh",
                    "number": 10,
                    "body": "Started implementation for the cart drawer.\n\nMore detail follows."
                }),
            })
            .unwrap();
        writer
            .write(&Event::ToolResult {
                call_id: "call_comment".into(),
                output: "ok".into(),
                error: None,
                duration_ms: 12,
            })
            .unwrap();
        writer
            .write(&Event::ToolCall {
                call_id: "call_close".into(),
                tool: "issues.update_state".into(),
                input: serde_json::json!({
                    "project": "Section9Labs/rupu-sandbox-gh",
                    "number": 10,
                    "state": "closed"
                }),
            })
            .unwrap();
        writer
            .write(&Event::ToolResult {
                call_id: "call_close".into(),
                output: "ok".into(),
                error: None,
                duration_ms: 5,
            })
            .unwrap();
        writer
            .write(&Event::ToolCall {
                call_id: "call_pr".into(),
                tool: "scm.prs.create".into(),
                input: serde_json::json!({
                    "owner": "Section9Labs",
                    "repo": "rupu-sandbox-gh",
                    "title": "Add cart drawer and checkout summary",
                    "base": "main",
                    "draft": true
                }),
            })
            .unwrap();
        writer
            .write(&Event::ToolResult {
                call_id: "call_pr".into(),
                output: "https://github.com/Section9Labs/rupu-sandbox-gh/pull/10".into(),
                error: None,
                duration_ms: 20,
            })
            .unwrap();
        writer
            .write(&Event::RunComplete {
                run_id: "step_run_1".into(),
                status: TranscriptRunStatus::Ok,
                total_tokens: 100,
                duration_ms: 1000,
                error: None,
            })
            .unwrap();
        writer.flush().unwrap();

        run_store
            .append_step_result(
                "run_123",
                &StepResultRecord {
                    step_id: "implement".into(),
                    run_id: "step_run_1".into(),
                    transcript_path,
                    output: "{\"status\":\"complete\"}".into(),
                    success: true,
                    skipped: false,
                    rendered_prompt: "implement issue 10".into(),
                    kind: StepKind::Linear,
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: Utc::now(),
                },
            )
            .unwrap();

        let before = AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu-sandbox-gh/issues/10".into(),
            repo_ref: "github:Section9Labs/rupu-sandbox-gh".into(),
            source_ref: Some("github:Section9Labs/rupu-sandbox-gh".into()),
            issue_display_ref: Some("10".into()),
            issue_title: Some("cart drawer".into()),
            issue_url: None,
            issue_state_name: Some("open".into()),
            issue_tracker: Some("github".into()),
            workflow: "storefront-feature-delivery".into(),
            status: ClaimStatus::Claimed,
            worktree_path: None,
            branch: Some("storefront/issue-10".into()),
            last_run_id: None,
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![],
            updated_at: Utc::now().to_rfc3339(),
        };
        let after = AutoflowClaimRecord {
            last_run_id: Some("run_123".into()),
            pr_url: Some("https://github.com/Section9Labs/rupu-sandbox-gh/pull/10".into()),
            status: ClaimStatus::Complete,
            ..before.clone()
        };

        let mut events = Vec::new();
        append_issue_history_events(
            &mut events,
            &Ok(true),
            &after.issue_ref,
            Some(&before),
            Some(&after),
            Some(&after.repo_ref),
            Some(&after.workflow),
            &global,
        );

        assert!(events.iter().any(|event| {
            event.kind == AutoflowCycleEventKind::RunLaunched
                && event.run_id.as_deref() == Some("run_123")
        }));
        assert!(events.iter().any(|event| {
            event.kind == AutoflowCycleEventKind::IssueCommented
                && event.detail.as_deref()
                    == Some("comment: Started implementation for the cart drawer.")
        }));
        assert!(events.iter().any(|event| {
            event.kind == AutoflowCycleEventKind::IssueStateChanged
                && event.status.as_deref() == Some("closed")
        }));
        assert!(events.iter().any(|event| {
            event.kind == AutoflowCycleEventKind::PullRequestOpened
                && event.status.as_deref() == Some("draft")
                && event
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains("Add cart drawer"))
        }));
    }
}
