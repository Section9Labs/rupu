//! The reports `rupu autoflow show | explain | doctor` print with
//! `--format json` (and the control plane serves), `rupu autoflow repair`,
//! and the checks `rupu autoflow run` makes before it starts a cycle.

use crate::claims::{
    cleanup_claim_artifacts, ensure_manual_run_can_take_claim, filter_claims_by_repo,
    format_contenders, next_action_summary, selected_priority, status_name,
};
use crate::format::format_inputs;
use crate::history::{filter_monitor_events, history_rows_from_events, AutoflowHistoryRow};
use crate::paths;
use crate::refs::{
    canonical_issue_ref, format_issue_ref, normalize_repo_filter, parse_full_issue_target,
    parse_issue_ref_text,
};
use crate::run_summary::{
    format_run_diff_line, format_run_metrics_line, format_run_models_line, format_run_usage_line,
    load_run_summary,
};
use crate::visible::{
    issue_ref_for_autoflow, resolve_autoflow_permission_mode, resolve_autoflow_workflow_for_issue,
    resolve_repo_binding_for_issue, visible_autoflow_matches, visible_autoflow_matches_for_issue,
    VisibleAutoflowWorkflow,
};
use crate::wakes::{
    describe_wake_source, enqueue_issue_wake, filter_wakes_by_repo, invalid_wake_payload_detail,
    wake_row_from_record, wakes_for_issue, AutoflowWakeRow,
};
use crate::Refusal;
use anyhow::anyhow;
use rupu_config::PricingConfig;
use rupu_orchestrator::{AutoflowWorkspaceStrategy, RunStatus, RunStore};
use rupu_runtime::{AutoflowHistoryStore, WakeSource, WakeStore};
use rupu_workspace::autoflow_claim_store::issue_key;
use rupu_workspace::{
    ensure_issue_worktree, issue_dir_name, AutoflowClaimStore, ClaimStatus, RepoRegistryStore,
};
use serde::Serialize;
use std::path::Path;

/// The `item` of `rupu autoflow show`'s report.
#[derive(Debug, Clone, Serialize)]
pub struct AutoflowShowItem {
    pub name: String,
    pub scope: String,
    pub path: String,
    pub repo: Option<String>,
    pub project_root: Option<String>,
    pub preferred_checkout: Option<String>,
    pub description: Option<String>,
    pub entity: String,
    pub source: String,
    pub priority: i32,
    pub workspace: String,
    pub workspace_branch: Option<String>,
    pub reconcile_every: Option<String>,
    pub claim_ttl: Option<String>,
    pub outcome_output: Option<String>,
    pub wake_on: Vec<String>,
    pub labels_all: Vec<String>,
    pub labels_any: Vec<String>,
    pub labels_none: Vec<String>,
    pub selector_states: Vec<String>,
    pub selector_limit: Option<u32>,
    pub body: String,
}

/// `rupu autoflow show`'s JSON report.
#[derive(Debug, Clone, Serialize)]
pub struct AutoflowShowReport {
    pub kind: &'static str,
    pub version: u8,
    pub item: AutoflowShowItem,
}

/// The `autoflow_show` report `rupu autoflow show --format json` prints and
/// `GET /api/autoflows/:name` serves.
pub fn build_autoflow_show_report(
    entry: &VisibleAutoflowWorkflow,
    body: String,
) -> anyhow::Result<AutoflowShowReport> {
    let autoflow = entry.autoflow()?;
    let selector_states = autoflow
        .selector
        .states
        .iter()
        .map(|state| match state {
            rupu_orchestrator::AutoflowIssueState::Open => "open".to_string(),
            rupu_orchestrator::AutoflowIssueState::Closed => "closed".to_string(),
        })
        .collect::<Vec<_>>();
    let item = AutoflowShowItem {
        name: entry.name.clone(),
        scope: entry.scope.clone(),
        path: entry.workflow_path.display().to_string(),
        repo: entry.repo_ref.clone(),
        project_root: entry
            .project_root
            .as_ref()
            .map(|path| path.display().to_string()),
        preferred_checkout: entry
            .preferred_checkout
            .as_ref()
            .map(|path| path.display().to_string()),
        description: entry.workflow.description.clone(),
        entity: match autoflow.entity {
            rupu_orchestrator::AutoflowEntity::Issue => "issue".to_string(),
            rupu_orchestrator::AutoflowEntity::PullRequest => "pull_request".to_string(),
        },
        source: autoflow
            .source
            .clone()
            .or_else(|| entry.repo_ref.clone())
            .unwrap_or_else(|| "-".to_string()),
        priority: autoflow.priority,
        workspace: autoflow
            .workspace
            .as_ref()
            .map(|workspace| match workspace.strategy {
                AutoflowWorkspaceStrategy::Worktree => "worktree".to_string(),
                AutoflowWorkspaceStrategy::InPlace => "in_place".to_string(),
            })
            .unwrap_or_else(|| "worktree".to_string()),
        workspace_branch: autoflow
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.branch.clone()),
        reconcile_every: autoflow.reconcile_every.clone(),
        claim_ttl: autoflow.claim.as_ref().and_then(|claim| claim.ttl.clone()),
        outcome_output: autoflow
            .outcome
            .as_ref()
            .map(|outcome| outcome.output.clone()),
        wake_on: autoflow.wake_on.clone(),
        labels_all: autoflow.selector.labels_all.clone(),
        labels_any: autoflow.selector.labels_any.clone(),
        labels_none: autoflow.selector.labels_none.clone(),
        selector_states,
        selector_limit: autoflow.selector.limit,
        body,
    };
    Ok(AutoflowShowReport {
        kind: "autoflow_show",
        version: 1,
        item,
    })
}

/// `rupu autoflow show <name>`'s report: the one autoflow named `name`
/// that `pwd` sees ([`show_entry`]) with its YAML body. The control plane's
/// `GET /api/autoflows/:name` serves it.
pub fn show_report(
    global: &Path,
    pwd: &Path,
    name: &str,
    repo: Option<&str>,
) -> anyhow::Result<AutoflowShowReport> {
    let entry = show_entry(global, pwd, name, repo)?;
    let body = std::fs::read_to_string(&entry.workflow_path)?;
    build_autoflow_show_report(&entry, body)
}

/// The one autoflow named `name` that `pwd` sees: none is
/// [`Refusal::NotFound`], several (pass `--repo`) [`Refusal::Conflict`].
pub fn show_entry(
    global: &Path,
    pwd: &Path,
    name: &str,
    repo: Option<&str>,
) -> anyhow::Result<VisibleAutoflowWorkflow> {
    let matches = visible_autoflow_matches(global, pwd, name, repo)?;
    let entry = match matches.as_slice() {
        [] => {
            return Err(Refusal::NotFound(match repo {
                Some(repo) => format!("no autoflow named `{name}` is visible for repo `{repo}`"),
                None => format!("no autoflow named `{name}` is visible"),
            })
            .into())
        }
        [entry] => entry,
        many => {
            let mut options = many
                .iter()
                .map(|entry| {
                    format!(
                        "- {} ({}, {})",
                        entry.repo_ref.as_deref().unwrap_or("-"),
                        entry.scope,
                        entry.workflow_path.display()
                    )
                })
                .collect::<Vec<_>>();
            options.sort();
            return Err(Refusal::Conflict(format!(
                "multiple autoflows named `{name}` are visible:\n{}\npass `--repo <platform>:<owner>/<repo>` to disambiguate",
                options.join("\n")
            ))
            .into());
        }
    };
    Ok(entry.clone())
}

#[derive(Debug, Clone, Serialize)]
pub struct AutoflowExplainLock {
    pub owner: String,
    pub acquired_at: String,
    pub lease_expires: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AutoflowExplainPendingDispatch {
    pub workflow: String,
    pub target: String,
    pub inputs: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AutoflowExplainRun {
    pub run_id: String,
    pub status: String,
    pub backend: String,
    pub worker: String,
    pub source_wake: Option<String>,
    pub approval_step: Option<String>,
    pub approval_expires: Option<String>,
    pub execution: Option<String>,
    pub models: Option<String>,
    pub usage: Option<String>,
    pub changes: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub workspace: Option<String>,
    pub merge_target: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AutoflowExplainClaim {
    pub issue_display: Option<String>,
    pub issue_title: Option<String>,
    pub issue_tracker: Option<String>,
    pub issue_state: Option<String>,
    pub source: Option<String>,
    pub issue_url: Option<String>,
    pub workflow: String,
    pub status: String,
    pub priority: Option<String>,
    pub contenders: String,
    pub workspace: String,
    pub branch: String,
    pub pr: String,
    pub claim_owner: String,
    pub lease_expires: String,
    pub active_lock: Option<AutoflowExplainLock>,
    pub last_run: Option<AutoflowExplainRun>,
    pub next_action: String,
    pub pending_dispatch: Option<AutoflowExplainPendingDispatch>,
    pub next_retry: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AutoflowExplainItem {
    pub issue: String,
    pub repo: String,
    pub status: String,
    pub source: Option<String>,
    pub candidate_workflows: Vec<String>,
    pub claim: Option<AutoflowExplainClaim>,
    pub queued_wakes: Vec<AutoflowWakeRow>,
    pub recent_processed_wake: Option<AutoflowWakeRow>,
    pub recent_cycle_events: Vec<AutoflowHistoryRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AutoflowExplainReport {
    pub kind: &'static str,
    pub version: u8,
    pub item: AutoflowExplainItem,
}

/// `rupu autoflow explain <ref>`'s report: the issue's claim (with its
/// last run, priced at `pricing`), queued and recent wakes and recent cycle
/// events, or, unclaimed, the autoflows `pwd` sees for it. `issue_ref` is
/// the full form (the CLI resolves a bare number against its checkout
/// first); anything else is a [`Refusal::Invalid`]. The control plane's
/// `GET /api/autoflows/explain` serves it.
pub fn explain_report(
    global: &Path,
    pwd: &Path,
    issue_ref: &str,
    repo: Option<&str>,
    pricing: &PricingConfig,
) -> anyhow::Result<AutoflowExplainReport> {
    let issue_ref =
        canonical_issue_ref(issue_ref).map_err(|e| Refusal::Invalid(format!("{e:#}")))?;
    let claim_store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(global),
    };
    let wake_store = WakeStore::new(paths::autoflow_wakes_dir(global));
    let run_store = RunStore::new(global.join("runs"));
    let history_store = AutoflowHistoryStore::new(paths::autoflow_history_dir(global));
    let claim = claim_store.load(&issue_ref)?;
    let issue_ref_parsed = parse_issue_ref_text(&issue_ref)?;
    let source_matches = if claim.is_none() {
        visible_autoflow_matches_for_issue(global, pwd, &issue_ref_parsed, repo)?
    } else {
        Vec::new()
    };
    let repo_ref = match claim.as_ref() {
        Some(claim) => claim.repo_ref.clone(),
        None => resolve_repo_binding_for_issue(global, pwd, &issue_ref_parsed, repo)?,
    };
    let queued = wakes_for_issue(
        filter_wakes_by_repo(wake_store.list_queued()?, Some(&repo_ref)),
        &issue_ref,
    );
    let mut processed = wakes_for_issue(
        filter_wakes_by_repo(wake_store.list_processed()?, Some(&repo_ref)),
        &issue_ref,
    );
    processed.sort_by(|left, right| right.received_at.cmp(&left.received_at));
    let recent_history = history_rows_from_events(
        &filter_monitor_events(
            history_store.list_recent_events(400)?,
            Some(&repo_ref),
            None,
        ),
        Some(&issue_ref),
        None,
        None,
    )
    .into_iter()
    .take(5)
    .collect::<Vec<_>>();
    let queued_wakes = queued
        .iter()
        .map(|wake| wake_row_from_record(wake, "queued"))
        .collect::<Vec<_>>();
    let recent_processed_wake = processed
        .first()
        .map(|wake| wake_row_from_record(wake, "processed"));
    let candidate_workflows = source_matches
        .iter()
        .map(|entry| entry.name.clone())
        .collect::<Vec<_>>();

    let claim_report = if let Some(claim) = claim {
        let active_lock =
            claim_store
                .read_active_lock(&issue_ref)?
                .map(|lock| AutoflowExplainLock {
                    owner: lock.owner,
                    acquired_at: lock.acquired_at,
                    lease_expires: lock.lease_expires_at,
                });
        let last_run = match claim.last_run_id.as_deref() {
            Some(run_id) => match run_store.load(run_id) {
                Ok(run) => {
                    let summary = load_run_summary(&run_store, run_id, pricing);
                    Some(AutoflowExplainRun {
                        run_id: run_id.to_string(),
                        status: run.status.as_str().to_string(),
                        backend: run.backend_id.unwrap_or_else(|| "-".into()),
                        worker: run.worker_id.unwrap_or_else(|| "-".into()),
                        source_wake: run.source_wake_id.as_deref().map(|source_wake_id| {
                            describe_wake_source(global, &wake_store, source_wake_id)
                        }),
                        approval_step: (run.status == RunStatus::AwaitingApproval)
                            .then(|| run.awaiting_step_id.unwrap_or_else(|| "-".into())),
                        approval_expires: (run.status == RunStatus::AwaitingApproval).then(|| {
                            run.expires_at
                                .map(|value| value.to_rfc3339())
                                .unwrap_or_else(|| "-".into())
                        }),
                        execution: summary.as_ref().map(format_run_metrics_line),
                        models: summary.as_ref().map(format_run_models_line),
                        usage: summary.as_ref().map(format_run_usage_line),
                        changes: summary.as_ref().map(format_run_diff_line),
                        started_at: summary
                            .as_ref()
                            .map(|summary| summary.started_at.to_rfc3339()),
                        finished_at: summary.as_ref().map(|summary| {
                            summary
                                .finished_at
                                .as_ref()
                                .map(|value| value.to_rfc3339())
                                .unwrap_or_else(|| "-".into())
                        }),
                        workspace: summary
                            .as_ref()
                            .map(|summary| summary.workspace.as_deref().unwrap_or("-").to_string()),
                        merge_target: summary.as_ref().map(|summary| {
                            summary
                                .diff
                                .as_ref()
                                .and_then(|diff| diff.merge_target.as_deref())
                                .unwrap_or("-")
                                .to_string()
                        }),
                    })
                }
                Err(error) => Some(AutoflowExplainRun {
                    run_id: run_id.to_string(),
                    status: format!("missing ({error})"),
                    backend: "-".into(),
                    worker: "-".into(),
                    source_wake: None,
                    approval_step: None,
                    approval_expires: None,
                    execution: None,
                    models: None,
                    usage: None,
                    changes: None,
                    started_at: None,
                    finished_at: None,
                    workspace: None,
                    merge_target: None,
                }),
            },
            None => None,
        };
        let priority = selected_priority(&claim).map(|value| value.to_string());
        let contenders = format_contenders(&claim.contenders);
        let next_action = next_action_summary(&claim);
        Some(AutoflowExplainClaim {
            issue_display: claim.issue_display_ref,
            issue_title: claim.issue_title,
            issue_tracker: claim.issue_tracker,
            issue_state: claim.issue_state_name,
            source: claim.source_ref,
            issue_url: claim.issue_url,
            workflow: claim.workflow,
            status: status_name(claim.status).to_string(),
            priority,
            contenders,
            workspace: claim.worktree_path.unwrap_or_else(|| "-".into()),
            branch: claim.branch.unwrap_or_else(|| "-".into()),
            pr: claim.pr_url.unwrap_or_else(|| "-".into()),
            claim_owner: claim.claim_owner.unwrap_or_else(|| "-".into()),
            lease_expires: claim.lease_expires_at.unwrap_or_else(|| "-".into()),
            active_lock,
            last_run,
            next_action,
            pending_dispatch: claim.pending_dispatch.map(|dispatch| {
                AutoflowExplainPendingDispatch {
                    workflow: dispatch.workflow,
                    target: dispatch.target,
                    inputs: format_inputs(&dispatch.inputs),
                }
            }),
            next_retry: claim.next_retry_at,
            summary: claim.last_summary,
        })
    } else {
        None
    };

    let report = AutoflowExplainReport {
        kind: "autoflow_explain",
        version: 1,
        item: AutoflowExplainItem {
            issue: issue_ref,
            repo: repo_ref,
            status: claim_report
                .as_ref()
                .map(|claim| claim.status.clone())
                .unwrap_or_else(|| "unclaimed".into()),
            source: claim_report
                .as_ref()
                .and_then(|claim| claim.source.clone())
                .or_else(|| {
                    source_matches
                        .first()
                        .and_then(|entry| entry.autoflow().ok())
                        .and_then(|autoflow| autoflow.source.clone())
                }),
            candidate_workflows,
            claim: claim_report,
            queued_wakes,
            recent_processed_wake,
            recent_cycle_events: recent_history,
        },
    };
    Ok(report)
}

#[derive(Debug, Clone, Serialize)]
pub struct AutoflowDoctorRow {
    pub scope: String,
    pub problem: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AutoflowDoctorReport {
    pub kind: &'static str,
    pub version: u8,
    pub ok: bool,
    pub rows: Vec<AutoflowDoctorRow>,
}

#[derive(Debug)]
pub struct DoctorFinding {
    pub scope: String,
    pub kind: &'static str,
    pub detail: String,
}

impl DoctorFinding {
    pub fn new(scope: impl Into<String>, kind: &'static str, detail: impl Into<String>) -> Self {
        Self {
            scope: scope.into(),
            kind,
            detail: detail.into(),
        }
    }
}

/// `rupu autoflow doctor`'s report: consistency problems across claims,
/// wakes, worktrees and runs (`ok` when there are none). The control
/// plane's `GET /api/autoflows/doctor` serves it.
pub fn doctor_report(global: &Path, repo: Option<&str>) -> anyhow::Result<AutoflowDoctorReport> {
    let global = global.to_path_buf();
    let repo_filter = normalize_repo_filter(repo)?;
    let repo_store = RepoRegistryStore {
        root: paths::repos_dir(&global),
    };
    let claim_store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    let wake_store = WakeStore::new(paths::autoflow_wakes_dir(&global));
    let run_store = RunStore::new(global.join("runs"));
    let mut findings = Vec::new();
    let claims = filter_claims_by_repo(claim_store.list()?, repo_filter.as_deref());
    let queued_wakes = filter_wakes_by_repo(wake_store.list_queued()?, repo_filter.as_deref());

    for claim in &claims {
        if repo_store.load(&claim.repo_ref)?.is_none() {
            findings.push(DoctorFinding::new(
                &claim.issue_ref,
                "missing_repo_binding",
                format!("repo `{}` is not tracked", claim.repo_ref),
            ));
        }
        if !matches!(claim.status, ClaimStatus::Complete | ClaimStatus::Released)
            && claim
                .worktree_path
                .as_deref()
                .is_some_and(|path| !Path::new(path).exists())
        {
            findings.push(DoctorFinding::new(
                &claim.issue_ref,
                "missing_worktree",
                format!(
                    "workspace path `{}` does not exist",
                    claim.worktree_path.as_deref().unwrap_or("-")
                ),
            ));
        }
        if let Some(manifest_path) = claim.artifact_manifest_path.as_deref() {
            if !Path::new(manifest_path).exists() {
                findings.push(DoctorFinding::new(
                    &claim.issue_ref,
                    "artifact_missing",
                    format!("artifact manifest `{manifest_path}` does not exist"),
                ));
            }
        }
        if let Some(run_id) = claim.last_run_id.as_deref() {
            match run_store.load(run_id) {
                Ok(run) => {
                    if run.issue_ref.as_deref() != Some(claim.issue_ref.as_str()) {
                        findings.push(DoctorFinding::new(
                            &claim.issue_ref,
                            "claim_run_mismatch",
                            format!(
                                "run `{run_id}` is bound to `{}`",
                                run.issue_ref.as_deref().unwrap_or("-")
                            ),
                        ));
                    }
                    if run.workflow_name != claim.workflow && claim.pending_dispatch.is_none() {
                        findings.push(DoctorFinding::new(
                            &claim.issue_ref,
                            "claim_run_mismatch",
                            format!(
                                "claim workflow `{}` disagrees with run workflow `{}`",
                                claim.workflow, run.workflow_name
                            ),
                        ));
                    }
                }
                Err(error) => findings.push(DoctorFinding::new(
                    &claim.issue_ref,
                    "missing_run",
                    format!("last run `{run_id}` could not be loaded: {error}"),
                )),
            }
        }
        let lock_path = claim_store
            .root
            .join(issue_key(&claim.issue_ref))
            .join(".lock");
        if lock_path.exists() {
            match claim_store.read_active_lock(&claim.issue_ref) {
                Ok(Some(lock)) if claim.status != ClaimStatus::Running => {
                    findings.push(DoctorFinding::new(
                        &claim.issue_ref,
                        "stale_lock",
                        format!(
                            "active lock owned by `{}` remains while claim status is `{}`",
                            lock.owner,
                            status_name(claim.status)
                        ),
                    ))
                }
                Err(error) => findings.push(DoctorFinding::new(
                    &claim.issue_ref,
                    "stale_lock",
                    format!("active lock is unreadable: {error}"),
                )),
                _ => {}
            }
        }
    }

    for entry in std::fs::read_dir(&claim_store.root)
        .into_iter()
        .flatten()
        .flatten()
    {
        let issue_dir = entry.path();
        let lock_path = issue_dir.join(".lock");
        let claim_path = issue_dir.join("claim.toml");
        if lock_path.is_file() && !claim_path.is_file() {
            findings.push(DoctorFinding::new(
                issue_dir
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("unknown"),
                "stale_lock",
                format!(
                    "orphan active lock `{}` has no claim.toml",
                    lock_path.display()
                ),
            ));
        }
    }

    for wake in &queued_wakes {
        if let Some(problem) = invalid_wake_payload_detail(&wake_store, wake) {
            findings.push(DoctorFinding::new(
                &wake.entity.ref_text,
                "invalid_wake_payload",
                format!("wake `{}` {problem}", wake.wake_id),
            ));
        }
    }

    let rows = findings
        .into_iter()
        .map(|finding| AutoflowDoctorRow {
            scope: finding.scope,
            problem: finding.kind.to_string(),
            detail: finding.detail,
        })
        .collect::<Vec<_>>();

    Ok(AutoflowDoctorReport {
        kind: "autoflow_doctor",
        version: 1,
        ok: rows.is_empty(),
        rows,
    })
}

/// What [`repair_claim`] did, or with `dry_run` would do: one line per
/// repair.
#[derive(Debug, Clone, Serialize)]
pub struct RepairOutcome {
    pub issue_ref: String,
    pub dry_run: bool,
    pub repairs: Vec<String>,
}

/// `rupu autoflow repair <ref> [--release | --requeue]`: delete the claim's
/// invalid queued wakes, rebuild its missing worktree (unless releasing),
/// then release the claim or queue a repair wake. `dry_run` checks the same
/// things and reports each repair as `would …`, changing nothing — the
/// control plane's `POST /api/autoflows/claims/repair` defaults to it.
pub fn repair_claim(
    global: &Path,
    r#ref: &str,
    release: bool,
    requeue_requested: bool,
    dry_run: bool,
) -> anyhow::Result<RepairOutcome> {
    if release && requeue_requested {
        return Err(Refusal::Invalid(
            "repair accepts at most one of `--release` or `--requeue`".into(),
        )
        .into());
    }

    let issue_ref = canonical_issue_ref(r#ref).map_err(|e| Refusal::Invalid(format!("{e:#}")))?;
    let repo_store = RepoRegistryStore {
        root: paths::repos_dir(global),
    };
    let claim_store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(global),
    };
    let wake_store = WakeStore::new(paths::autoflow_wakes_dir(global));
    let Some(mut claim) = claim_store.load(&issue_ref)? else {
        return Err(Refusal::NotFound(format!("no autoflow claim for `{issue_ref}`")).into());
    };
    let would = |text: String| {
        if dry_run {
            format!("would {text}")
        } else {
            text
        }
    };

    let mut repairs = Vec::new();
    let queued = wakes_for_issue(
        filter_wakes_by_repo(wake_store.list_queued()?, Some(&claim.repo_ref)),
        &issue_ref,
    );
    for wake in queued {
        if invalid_wake_payload_detail(&wake_store, &wake).is_some() {
            if !dry_run {
                wake_store.delete(&wake.wake_id)?;
            }
            repairs.push(would(format!(
                "{} invalid queued wake {}",
                if dry_run { "delete" } else { "deleted" },
                wake.wake_id
            )));
        }
    }

    if !release
        && claim
            .worktree_path
            .as_deref()
            .zip(claim.branch.as_deref())
            .is_some_and(|(path, _)| !Path::new(path).exists())
    {
        let tracked = repo_store
            .load(&claim.repo_ref)?
            .ok_or_else(|| anyhow!("repo `{}` is not tracked", claim.repo_ref))?;
        let branch = claim
            .branch
            .clone()
            .unwrap_or_else(|| format!("rupu/{}", issue_dir_name(&claim.issue_ref)));
        if dry_run {
            repairs.push(would(format!(
                "rebuild worktree {} on branch {branch}",
                claim.worktree_path.as_deref().unwrap_or("-")
            )));
        } else {
            let worktree = ensure_issue_worktree(
                Path::new(&tracked.preferred_path),
                &paths::autoflow_worktrees_dir(global),
                &claim.repo_ref,
                &claim.issue_ref,
                &branch,
                Some("HEAD"),
            )?;
            claim.worktree_path = Some(worktree.path.display().to_string());
            claim.branch = Some(branch);
            claim.updated_at = chrono::Utc::now().to_rfc3339();
            claim_store.save(&claim)?;
            repairs.push(format!(
                "rebuilt worktree {}",
                claim.worktree_path.as_deref().unwrap_or("-")
            ));
        }
    }

    if release {
        if !dry_run {
            cleanup_claim_artifacts(&repo_store, &claim)?;
            claim_store.delete(&issue_ref)?;
        }
        repairs.push(would(format!(
            "{} claim {issue_ref}",
            if dry_run { "release" } else { "released" }
        )));
    } else if requeue_requested {
        if dry_run {
            repairs.push(would("queue a repair wake".to_string()));
        } else {
            let wake = enqueue_issue_wake(
                &wake_store,
                WakeSource::Repair,
                &claim.repo_ref,
                &issue_ref,
                "autoflow.repair.requeue",
                chrono::Utc::now(),
            )?;
            repairs.push(format!("queued repair wake {}", wake.wake_id));
        }
    }

    Ok(RepairOutcome {
        issue_ref,
        dry_run,
        repairs,
    })
}

/// What `rupu autoflow run` checks before it fetches the issue, for the
/// control plane to answer before it starts the cycle detached: the target
/// is an issue ([`Refusal::Invalid`]), the autoflow resolves for it from
/// `pwd` ([`Refusal::NotFound`] / [`Refusal::Conflict`] when ambiguous),
/// the mode is one autoflows take, and no other cycle holds the issue's
/// claim ([`Refusal::Conflict`]).
pub fn run_precheck(
    global: &Path,
    pwd: &Path,
    name: &str,
    target: &str,
    repo: Option<&str>,
    mode: Option<&str>,
) -> anyhow::Result<()> {
    let issue_ref =
        parse_full_issue_target(target).map_err(|e| Refusal::Invalid(format!("{e:#}")))?;
    let resolved = resolve_autoflow_workflow_for_issue(global, pwd, name, &issue_ref, repo)?;
    resolve_autoflow_permission_mode(mode, resolved.cfg.autoflow.permission_mode.as_deref())
        .map_err(|e| Refusal::Invalid(format!("{e:#}")))?;
    let claim_store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(global),
    };
    let fetch_ref = issue_ref_for_autoflow(&issue_ref, &resolved)?;
    ensure_manual_run_can_take_claim(&claim_store, &format_issue_ref(&fetch_ref))
}
