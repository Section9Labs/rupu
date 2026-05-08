//! `rupu autoflow ...` — manual/autonomous workflow entrypoints.

use crate::cmd::completers::workflow_names;
use crate::cmd::issues::canonical_issue_ref;
use crate::cmd::workflow::{
    locate_workflow_in, run_with_explicit_context, ExplicitWorkflowRunContext,
};
use crate::paths;
use anyhow::{anyhow, bail, Context};
use clap::Subcommand;
use clap_complete::ArgValueCompleter;
use comfy_table::Cell;
use jsonschema::JSONSchema;
use rupu_auth::{CredentialResolver, KeychainResolver};
use rupu_config::{AutoflowCheckout, Config};
use rupu_orchestrator::templates::{render_step_prompt, RenderMode, StepContext};
use rupu_orchestrator::{
    AutoflowWorkspaceStrategy, ContractFormat, RunStatus, RunStore, StepResultRecord, Workflow,
    WorkflowOutputContract,
};
use rupu_scm::{Issue, IssueFilter, IssueRef, IssueState, IssueTracker};
use rupu_workspace::{
    ensure_issue_worktree, issue_dir_name, AutoflowClaimRecord, AutoflowClaimStore, ClaimStatus,
    PendingDispatch, RepoRegistryStore,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::sync::Arc;
use tracing::warn;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List autoflow-enabled workflows.
    List,
    /// Show one autoflow workflow and its resolved metadata.
    Show {
        #[arg(add = ArgValueCompleter::new(workflow_names))]
        name: String,
    },
    /// Execute one autonomous cycle for one issue target.
    Run {
        #[arg(add = ArgValueCompleter::new(workflow_names))]
        name: String,
        /// Issue target in full run-target form:
        /// `github:owner/repo/issues/42` or `gitlab:group/project/issues/9`.
        target: String,
        /// Override permission mode (`ask` | `bypass` | `readonly`).
        #[arg(long)]
        mode: Option<String>,
    },
    /// Reconcile every discovered autoflow once.
    Tick,
    /// Summarize persisted autoflow claim state.
    Status,
    /// Inspect persisted autoflow claims.
    Claims,
    /// Force-release one claim.
    Release { r#ref: String },
}

#[derive(Debug, Clone)]
struct ResolvedAutoflowWorkflow {
    scope: String,
    name: String,
    workflow: Workflow,
    project_root: Option<PathBuf>,
    repo_ref: String,
    preferred_checkout: PathBuf,
    cfg: Config,
}

impl ResolvedAutoflowWorkflow {
    fn autoflow(&self) -> anyhow::Result<&rupu_orchestrator::Autoflow> {
        self.workflow
            .autoflow
            .as_ref()
            .filter(|autoflow| autoflow.enabled)
            .ok_or_else(|| anyhow!("workflow `{}` is not autoflow-enabled", self.workflow.name))
    }
}

#[derive(Debug, Clone)]
struct IssueMatch {
    resolved: ResolvedAutoflowWorkflow,
    issue: Issue,
    issue_ref_text: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct AutoflowOutcomeDoc {
    status: AutoflowOutcomeStatus,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    dispatch: Option<DispatchDoc>,
    #[serde(default)]
    retry_after: Option<String>,
    #[serde(default)]
    pr_url: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    artifacts: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum AutoflowOutcomeStatus {
    Continue,
    AwaitHuman,
    AwaitExternal,
    Retry,
    Blocked,
    Complete,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct DispatchDoc {
    workflow: String,
    target: String,
    #[serde(default)]
    inputs: BTreeMap<String, String>,
}

pub async fn handle(action: Action) -> ExitCode {
    let resolver: Arc<dyn CredentialResolver> = Arc::new(KeychainResolver::new());
    let result = handle_with_resolver(action, resolver).await;
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

async fn handle_with_resolver(
    action: Action,
    resolver: Arc<dyn CredentialResolver>,
) -> anyhow::Result<()> {
    match action {
        Action::List => list().await,
        Action::Show { name } => show(&name).await,
        Action::Run { name, target, mode } => run(&name, &target, mode.as_deref(), resolver).await,
        Action::Tick => tick_with_resolver(resolver).await,
        Action::Status => status().await,
        Action::Claims => claims().await,
        Action::Release { r#ref } => release(&r#ref).await,
    }
}

async fn list() -> anyhow::Result<()> {
    let entries = visible_autoflows()?;
    println!("{:<28} {:<8} {:<8} PRIORITY", "NAME", "SCOPE", "ENTITY");
    for (name, scope, workflow) in entries {
        let autoflow = workflow.autoflow.as_ref().expect("filtered to autoflows");
        println!(
            "{:<28} {:<8} {:<8} {}",
            name,
            scope,
            match autoflow.entity {
                rupu_orchestrator::AutoflowEntity::Issue => "issue",
            },
            autoflow.priority
        );
    }
    Ok(())
}

async fn show(name: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let path = locate_workflow_in(&global, project_root.as_deref(), name)?;
    let body = std::fs::read_to_string(&path)?;
    let workflow = Workflow::parse(&body)?;
    let autoflow = workflow
        .autoflow
        .as_ref()
        .filter(|a| a.enabled)
        .ok_or_else(|| anyhow!("workflow `{name}` does not declare `autoflow.enabled = true`"))?;

    println!("path: {}", path.display());
    println!("priority: {}", autoflow.priority);
    println!(
        "entity: {}",
        match autoflow.entity {
            rupu_orchestrator::AutoflowEntity::Issue => "issue",
        }
    );
    println!(
        "workspace: {}",
        autoflow
            .workspace
            .as_ref()
            .map(|w| match w.strategy {
                AutoflowWorkspaceStrategy::Worktree => "worktree",
                AutoflowWorkspaceStrategy::InPlace => "in_place",
            })
            .unwrap_or("worktree")
    );
    if let Some(outcome) = &autoflow.outcome {
        println!("outcome output: {}", outcome.output);
    }
    if !autoflow.selector.states.is_empty() {
        let states = autoflow
            .selector
            .states
            .iter()
            .map(|s| match s {
                rupu_orchestrator::AutoflowIssueState::Open => "open",
                rupu_orchestrator::AutoflowIssueState::Closed => "closed",
            })
            .collect::<Vec<_>>()
            .join(",");
        println!("selector states: {states}");
    }
    if !autoflow.selector.labels_all.is_empty() {
        println!(
            "selector labels_all: {}",
            autoflow.selector.labels_all.join(",")
        );
    }
    println!("---");
    print!("{body}");
    Ok(())
}

async fn run(
    name: &str,
    target: &str,
    mode: Option<&str>,
    resolver: Arc<dyn CredentialResolver>,
) -> anyhow::Result<()> {
    let issue_ref = parse_full_issue_target(target)?;
    let issue_ref_text = canonical_issue_ref(target)?;
    let repo_ref = issue_repo_ref(&issue_ref);
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let repo_store = RepoRegistryStore {
        root: paths::repos_dir(&global),
    };
    let resolved = resolve_autoflow_workflow_for_repo(&global, &repo_store, &repo_ref, name)?;
    let issue = fetch_issue(&resolved.cfg, resolver.as_ref(), &issue_ref).await?;
    let claim_store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    execute_autoflow_cycle(
        &global,
        &claim_store,
        &resolved,
        &issue,
        &issue_ref_text,
        mode,
        true,
        BTreeMap::new(),
    )
    .await
}

async fn tick_with_resolver(resolver: Arc<dyn CredentialResolver>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let repo_store = RepoRegistryStore {
        root: paths::repos_dir(&global),
    };
    let claim_store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    let discovered = discover_tick_autoflows(&global, &repo_store)?;
    if discovered.is_empty() {
        println!("(no autoflows)");
        return Ok(());
    }

    let tick_started_at = chrono::Utc::now();
    let winners = choose_winning_matches(
        collect_issue_matches(&discovered, resolver.as_ref())
            .await
            .context("discover autoflow issue matches")?,
    );
    let mut claims_by_issue: BTreeMap<String, AutoflowClaimRecord> = claim_store
        .list()?
        .into_iter()
        .map(|claim| (claim.issue_ref.clone(), claim))
        .collect();
    let mut issue_keys: BTreeSet<String> = winners.keys().cloned().collect();
    issue_keys.extend(claims_by_issue.keys().cloned());

    let mut active_claim_counts: BTreeMap<String, usize> = BTreeMap::new();
    for claim in claims_by_issue.values() {
        if claim.status != ClaimStatus::Released {
            *active_claim_counts
                .entry(claim.repo_ref.clone())
                .or_insert(0) += 1;
        }
    }

    let mut ran = 0usize;
    let mut skipped = 0usize;

    for issue_ref_text in issue_keys {
        let winner = winners.get(&issue_ref_text).cloned();
        let mut claim = claims_by_issue.remove(&issue_ref_text);

        if let Some(mut current) = claim.take() {
            let active_lock = claim_store.read_active_lock(&issue_ref_text)?;
            let claim_expired = claim_lease_expired(&current)?;
            let owner_resolution = resolve_autoflow_workflow_for_repo(
                &global,
                &repo_store,
                &current.repo_ref,
                &current.workflow,
            );

            if owner_resolution.is_err() && (!claim_expired || active_lock.is_some()) {
                skipped += 1;
                continue;
            }
            if claim_expired && active_lock.is_none() && owner_resolution.is_err() {
                current.status = ClaimStatus::Released;
                current.updated_at = chrono::Utc::now().to_rfc3339();
                claim_store.save(&current)?;
                if winner.is_none() {
                    skipped += 1;
                    continue;
                }
            } else {
                let mut resolved = owner_resolution?;
                reconcile_claim_from_last_run(&global, &resolved, &mut current)?;

                if current.status == ClaimStatus::Released {
                    claim_store.save(&current)?;
                } else if current.status == ClaimStatus::Complete
                    || current.status == ClaimStatus::Blocked
                {
                    claim_store.save(&current)?;
                    skipped += 1;
                    continue;
                } else if let Some(dispatch) = current.pending_dispatch.clone() {
                    if !updated_before_tick(&current, tick_started_at)? {
                        claim_store.save(&current)?;
                        skipped += 1;
                        continue;
                    }
                    resolved = resolve_autoflow_workflow_for_repo(
                        &global,
                        &repo_store,
                        &current.repo_ref,
                        &dispatch.workflow,
                    )?;
                    let issue = fetch_issue(
                        &resolved.cfg,
                        resolver.as_ref(),
                        &parse_issue_ref_text(&issue_ref_text)?,
                    )
                    .await?;
                    execute_autoflow_cycle(
                        &global,
                        &claim_store,
                        &resolved,
                        &issue,
                        &issue_ref_text,
                        None,
                        false,
                        dispatch.inputs,
                    )
                    .await?;
                    ran += 1;
                    continue;
                } else if should_run_claim(&current, &resolved, &claim_store, tick_started_at)? {
                    let issue = fetch_issue(
                        &resolved.cfg,
                        resolver.as_ref(),
                        &parse_issue_ref_text(&issue_ref_text)?,
                    )
                    .await?;
                    execute_autoflow_cycle(
                        &global,
                        &claim_store,
                        &resolved,
                        &issue,
                        &issue_ref_text,
                        None,
                        false,
                        BTreeMap::new(),
                    )
                    .await?;
                    ran += 1;
                    continue;
                } else {
                    claim_store.save(&current)?;
                    skipped += 1;
                    continue;
                }
            }
        }

        let Some(winner) = winner else {
            skipped += 1;
            continue;
        };
        let max_active = winner.resolved.cfg.autoflow.max_active.unwrap_or(u32::MAX) as usize;
        let active = active_claim_counts
            .get(&winner.resolved.repo_ref)
            .copied()
            .unwrap_or_default();
        if active >= max_active {
            skipped += 1;
            continue;
        }
        execute_autoflow_cycle(
            &global,
            &claim_store,
            &winner.resolved,
            &winner.issue,
            &winner.issue_ref_text,
            None,
            false,
            BTreeMap::new(),
        )
        .await?;
        *active_claim_counts
            .entry(winner.resolved.repo_ref.clone())
            .or_insert(0) += 1;
        ran += 1;
    }

    println!(
        "autoflow tick: {} workflow(s), {} cycle(s) ran, {} skipped",
        discovered.len(),
        ran,
        skipped
    );
    Ok(())
}

async fn status() -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for claim in store.list()? {
        *counts.entry(status_name(claim.status)).or_insert(0) += 1;
    }
    if counts.is_empty() {
        println!("(no autoflow claims)");
        return Ok(());
    }
    println!("{:<16} COUNT", "STATUS");
    for (status, count) in counts {
        println!("{status:<16} {count}");
    }
    Ok(())
}

async fn claims() -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    let claims = store.list()?;
    if claims.is_empty() {
        println!("(no autoflow claims)");
        return Ok(());
    }

    let mut table = crate::output::tables::new_table();
    table.set_header(vec!["Issue", "Workflow", "Status", "Run", "Workspace"]);
    for claim in claims {
        table.add_row(vec![
            Cell::new(claim.issue_ref),
            Cell::new(claim.workflow),
            Cell::new(status_name(claim.status)),
            Cell::new(claim.last_run_id.unwrap_or_else(|| "-".into())),
            Cell::new(claim.worktree_path.unwrap_or_else(|| "-".into())),
        ]);
    }
    println!("{table}");
    Ok(())
}

async fn release(r#ref: &str) -> anyhow::Result<()> {
    let issue_ref = canonical_issue_ref(r#ref)?;
    let global = paths::global_dir()?;
    let store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    if store.delete(&issue_ref)? {
        println!("released {issue_ref}");
    } else {
        println!("{issue_ref} was not claimed");
    }
    Ok(())
}

fn visible_autoflows() -> anyhow::Result<Vec<(String, String, Workflow)>> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let mut by_name: BTreeMap<String, (String, PathBuf)> = BTreeMap::new();
    push_workflow_paths(&global.join("workflows"), "global", &mut by_name);
    if let Some(project_root) = &project_root {
        push_workflow_paths(
            &project_root.join(".rupu/workflows"),
            "project",
            &mut by_name,
        );
    }
    let mut out = Vec::new();
    for (name, (scope, path)) in by_name {
        let body = std::fs::read_to_string(&path)?;
        let workflow = Workflow::parse(&body)?;
        if workflow
            .autoflow
            .as_ref()
            .map(|a| a.enabled)
            .unwrap_or(false)
        {
            out.push((name, scope, workflow));
        }
    }
    Ok(out)
}

fn discover_tick_autoflows(
    global: &Path,
    repo_store: &RepoRegistryStore,
) -> anyhow::Result<Vec<ResolvedAutoflowWorkflow>> {
    let mut out = Vec::new();
    let global_cfg = resolve_config(global, None)?;
    if global_cfg.autoflow.enabled.unwrap_or(true) {
        if let Some(repo_ref) = global_cfg.autoflow.repo.clone() {
            if let Some(tracked) = repo_store.load(&repo_ref)? {
                let preferred_checkout = PathBuf::from(&tracked.preferred_path);
                let project_root = paths::project_root_for(&preferred_checkout)?
                    .or_else(|| Some(preferred_checkout.clone()));
                push_resolved_autoflow_paths(
                    &global.join("workflows"),
                    "global",
                    global,
                    project_root,
                    repo_ref,
                    preferred_checkout,
                    global_cfg.clone(),
                    &mut out,
                )?;
            } else {
                warn!(
                    repo_ref,
                    "skipping global autoflows because repo is not tracked"
                );
            }
        }
    }

    for tracked in repo_store.list()? {
        let preferred_checkout = PathBuf::from(&tracked.preferred_path);
        if !preferred_checkout.exists() {
            warn!(path = %preferred_checkout.display(), repo_ref = %tracked.repo_ref, "skipping tracked repo because preferred checkout is missing");
            continue;
        }
        let project_root = paths::project_root_for(&preferred_checkout)?.or_else(|| {
            preferred_checkout
                .join(".rupu")
                .is_dir()
                .then_some(preferred_checkout.clone())
        });
        let Some(project_root) = project_root else {
            continue;
        };
        let cfg = resolve_config(global, Some(&project_root))?;
        if cfg.autoflow.enabled == Some(false) {
            continue;
        }
        push_resolved_autoflow_paths(
            &project_root.join(".rupu/workflows"),
            "project",
            global,
            Some(project_root),
            tracked.repo_ref.clone(),
            preferred_checkout,
            cfg,
            &mut out,
        )?;
    }

    out.sort_by(|left, right| {
        left.repo_ref
            .cmp(&right.repo_ref)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.scope.cmp(&right.scope))
    });
    Ok(out)
}

async fn collect_issue_matches(
    discovered: &[ResolvedAutoflowWorkflow],
    resolver: &dyn CredentialResolver,
) -> anyhow::Result<Vec<IssueMatch>> {
    let mut out = Vec::new();
    for resolved in discovered {
        let autoflow = resolved.autoflow()?;
        let (tracker, project) = parse_repo_ref(&resolved.repo_ref)?;
        let registry = Arc::new(rupu_scm::Registry::discover(resolver, &resolved.cfg).await);
        let Some(connector) = registry.issues(tracker) else {
            warn!(repo_ref = %resolved.repo_ref, workflow = %resolved.name, "skipping autoflow because no issue connector is configured");
            continue;
        };

        let filter = build_issue_filter(autoflow);
        let mut issues = match connector.list_issues(&project, filter).await {
            Ok(issues) => issues,
            Err(err) => {
                warn!(repo_ref = %resolved.repo_ref, workflow = %resolved.name, error = %err, "skipping autoflow because issue listing failed");
                continue;
            }
        };
        issues.retain(|issue| selector_matches(autoflow, issue));
        issues.sort_by_key(|issue| issue.r.number);
        if let Some(limit) = autoflow.selector.limit {
            issues.truncate(limit as usize);
        }
        for issue in issues {
            out.push(IssueMatch {
                issue_ref_text: format_issue_ref(&issue.r),
                resolved: resolved.clone(),
                issue,
            });
        }
    }
    Ok(out)
}

fn choose_winning_matches(matches: Vec<IssueMatch>) -> BTreeMap<String, IssueMatch> {
    let mut grouped: BTreeMap<String, Vec<IssueMatch>> = BTreeMap::new();
    for item in matches {
        grouped
            .entry(item.issue_ref_text.clone())
            .or_default()
            .push(item);
    }

    let mut winners = BTreeMap::new();
    for (issue_ref_text, mut items) in grouped {
        items.sort_by(|left, right| {
            right
                .resolved
                .autoflow()
                .expect("autoflow")
                .priority
                .cmp(&left.resolved.autoflow().expect("autoflow").priority)
                .then_with(|| {
                    left.resolved
                        .workflow
                        .name
                        .cmp(&right.resolved.workflow.name)
                })
        });
        if let Some(winner) = items.into_iter().next() {
            winners.insert(issue_ref_text, winner);
        }
    }
    winners
}

async fn execute_autoflow_cycle(
    global: &Path,
    claim_store: &AutoflowClaimStore,
    resolved: &ResolvedAutoflowWorkflow,
    issue: &Issue,
    issue_ref_text: &str,
    mode_override: Option<&str>,
    attach_ui: bool,
    inputs: BTreeMap<String, String>,
) -> anyhow::Result<()> {
    let autoflow = resolved.autoflow()?;
    let issue_payload = issue_payload(issue)?;
    let workspace_strategy = resolve_workspace_strategy(&resolved.cfg.autoflow, autoflow);
    let branch = resolve_branch_name(
        autoflow
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.branch.as_deref()),
        &issue_payload,
        issue_ref_text,
    )?;
    let workspace_path = match workspace_strategy {
        AutoflowWorkspaceStrategy::Worktree => {
            let root = resolve_worktree_root(global, &resolved.cfg.autoflow)?;
            ensure_issue_worktree(
                &resolved.preferred_checkout,
                &root,
                &resolved.repo_ref,
                issue_ref_text,
                &branch,
                Some("HEAD"),
            )?
            .path
        }
        AutoflowWorkspaceStrategy::InPlace => resolved.preferred_checkout.clone(),
    };
    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, &workspace_path)?;

    let owner = format!("{}:pid-{}", whoami::username(), std::process::id());
    let lease_expires_at = match autoflow
        .claim
        .as_ref()
        .and_then(|claim| claim.ttl.as_deref())
    {
        Some(ttl) => Some((chrono::Utc::now() + parse_duration(ttl)?).to_rfc3339()),
        None => None,
    };
    let _lock =
        claim_store.try_acquire_active_lock(issue_ref_text, &owner, lease_expires_at.as_deref())?;

    let mut claim = claim_store
        .load(issue_ref_text)?
        .unwrap_or(AutoflowClaimRecord {
            issue_ref: issue_ref_text.to_string(),
            repo_ref: resolved.repo_ref.clone(),
            workflow: resolved.workflow.name.clone(),
            status: ClaimStatus::Claimed,
            worktree_path: None,
            branch: None,
            last_run_id: None,
            last_error: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            updated_at: chrono::Utc::now().to_rfc3339(),
        });
    claim.workflow = resolved.workflow.name.clone();
    claim.status = ClaimStatus::Running;
    claim.worktree_path = Some(workspace_path.display().to_string());
    claim.branch = Some(branch.clone());
    claim.claim_owner = Some(owner);
    claim.lease_expires_at = lease_expires_at;
    claim.pending_dispatch = None;
    claim.updated_at = chrono::Utc::now().to_rfc3339();
    claim_store.save(&claim)?;

    let run_result = run_with_explicit_context(
        &resolved.name,
        ExplicitWorkflowRunContext {
            project_root: resolved.project_root.clone(),
            workspace_path,
            workspace_id: ws.id,
            inputs: inputs.into_iter().collect(),
            mode: mode_override
                .map(ToOwned::to_owned)
                .or_else(|| resolved.cfg.autoflow.permission_mode.clone())
                .unwrap_or_else(|| "bypass".to_string()),
            event: None,
            issue: Some(issue_payload),
            issue_ref: Some(issue_ref_text.to_string()),
            system_prompt_suffix: Some(crate::run_target::format_run_target_for_prompt(
                &crate::run_target::RunTarget::Issue {
                    tracker: issue.r.tracker,
                    project: issue.r.project.clone(),
                    number: issue.r.number,
                },
            )),
            attach_ui,
            use_canvas: false,
            run_id_override: None,
            strict_templates: resolved.cfg.autoflow.strict_templates.unwrap_or(true),
        },
    )
    .await;

    match run_result {
        Ok(summary) => {
            claim.last_run_id = Some(summary.run_id.clone());
            if summary.awaiting_step_id.is_some() {
                claim.status = ClaimStatus::AwaitHuman;
                claim.last_error = None;
                claim.updated_at = chrono::Utc::now().to_rfc3339();
                claim_store.save(&claim)?;
                return Ok(());
            }
            if let Err(err) =
                apply_terminal_run_to_claim(global, resolved, &summary.run_id, &mut claim)
            {
                claim.status = ClaimStatus::Blocked;
                claim.last_error = Some(err.to_string());
                claim.updated_at = chrono::Utc::now().to_rfc3339();
                claim_store.save(&claim)?;
                return Err(err);
            }
            claim_store.save(&claim)?;
            Ok(())
        }
        Err(err) => {
            claim.status = ClaimStatus::Blocked;
            claim.last_error = Some(err.to_string());
            claim.updated_at = chrono::Utc::now().to_rfc3339();
            claim_store.save(&claim)?;
            Err(err)
        }
    }
}

fn reconcile_claim_from_last_run(
    global: &Path,
    resolved: &ResolvedAutoflowWorkflow,
    claim: &mut AutoflowClaimRecord,
) -> anyhow::Result<()> {
    if !matches!(claim.status, ClaimStatus::AwaitHuman | ClaimStatus::Running) {
        return Ok(());
    }
    let Some(run_id) = claim.last_run_id.clone() else {
        return Ok(());
    };
    let run_store = RunStore::new(global.join("runs"));
    let run = match run_store.load(&run_id) {
        Ok(run) => run,
        Err(_) => return Ok(()),
    };
    match run.status {
        RunStatus::AwaitingApproval => {
            claim.status = ClaimStatus::AwaitHuman;
        }
        RunStatus::Completed => {
            apply_terminal_run_to_claim(global, resolved, &run_id, claim)?;
        }
        RunStatus::Failed | RunStatus::Rejected => {
            claim.status = ClaimStatus::Blocked;
            claim.last_error = run
                .error_message
                .clone()
                .or_else(|| Some(run.status.as_str().to_string()));
            claim.updated_at = chrono::Utc::now().to_rfc3339();
        }
        RunStatus::Pending | RunStatus::Running => {}
    }
    Ok(())
}

fn apply_terminal_run_to_claim(
    global: &Path,
    resolved: &ResolvedAutoflowWorkflow,
    run_id: &str,
    claim: &mut AutoflowClaimRecord,
) -> anyhow::Result<()> {
    let run_store = RunStore::new(global.join("runs"));
    let run = run_store.load(run_id)?;
    match run.status {
        RunStatus::Completed => {}
        RunStatus::AwaitingApproval => {
            claim.status = ClaimStatus::AwaitHuman;
            claim.updated_at = chrono::Utc::now().to_rfc3339();
            return Ok(());
        }
        RunStatus::Failed | RunStatus::Rejected => {
            claim.status = ClaimStatus::Blocked;
            claim.last_error = run
                .error_message
                .clone()
                .or_else(|| Some(run.status.as_str().to_string()));
            claim.updated_at = chrono::Utc::now().to_rfc3339();
            return Ok(());
        }
        RunStatus::Pending | RunStatus::Running => {
            claim.status = ClaimStatus::Running;
            claim.updated_at = chrono::Utc::now().to_rfc3339();
            return Ok(());
        }
    }

    let (contract, output_record) = autoflow_output_record(&run_store, resolved, run_id)?;
    let raw_output = match contract.format {
        ContractFormat::Json => serde_json::from_str::<serde_json::Value>(&output_record.output)
            .with_context(|| format!("parse JSON outcome from step `{}`", contract.from_step))?,
        ContractFormat::Yaml => {
            let yaml_value: serde_yaml::Value = serde_yaml::from_str(&output_record.output)
                .with_context(|| {
                    format!("parse YAML outcome from step `{}`", contract.from_step)
                })?;
            serde_json::to_value(yaml_value)?
        }
    };
    validate_output_contract(
        global,
        resolved.project_root.as_deref(),
        &contract,
        &raw_output,
    )?;
    let outcome: AutoflowOutcomeDoc = serde_json::from_value(raw_output)?;
    claim.last_error = None;
    claim.next_retry_at = None;
    claim.pending_dispatch = None;

    if let Some(dispatch) = outcome.dispatch {
        let target = canonical_issue_ref(&dispatch.target)?;
        if target != claim.issue_ref {
            bail!(
                "autoflow dispatch target `{}` does not match claimed issue `{}`",
                target,
                claim.issue_ref
            );
        }
        claim.pending_dispatch = Some(PendingDispatch {
            workflow: dispatch.workflow,
            target,
            inputs: dispatch.inputs,
        });
    }

    if let Some(reason) = outcome.reason {
        claim.last_error = Some(reason);
    }
    let _ = outcome.summary;
    let _ = outcome.pr_url;
    let _ = outcome.artifacts;

    claim.status = match outcome.status {
        AutoflowOutcomeStatus::Continue => ClaimStatus::Claimed,
        AutoflowOutcomeStatus::AwaitHuman => ClaimStatus::AwaitHuman,
        AutoflowOutcomeStatus::AwaitExternal => ClaimStatus::AwaitExternal,
        AutoflowOutcomeStatus::Retry => {
            let retry_after = outcome
                .retry_after
                .as_deref()
                .ok_or_else(|| anyhow!("autoflow retry outcome must include `retry_after`"))?;
            claim.next_retry_at = Some(resolve_retry_at(retry_after)?);
            ClaimStatus::RetryBackoff
        }
        AutoflowOutcomeStatus::Blocked => ClaimStatus::Blocked,
        AutoflowOutcomeStatus::Complete => ClaimStatus::Complete,
    };
    claim.updated_at = chrono::Utc::now().to_rfc3339();
    Ok(())
}

fn autoflow_output_record(
    run_store: &RunStore,
    resolved: &ResolvedAutoflowWorkflow,
    run_id: &str,
) -> anyhow::Result<(WorkflowOutputContract, StepResultRecord)> {
    let autoflow = resolved.autoflow()?;
    let outcome_ref = autoflow.outcome.as_ref().ok_or_else(|| {
        anyhow!(
            "autoflow `{}` does not declare `autoflow.outcome.output`",
            resolved.name
        )
    })?;
    let contract = resolved
        .workflow
        .contracts
        .outputs
        .get(&outcome_ref.output)
        .ok_or_else(|| {
            anyhow!(
                "workflow `{}` missing output contract `{}`",
                resolved.name,
                outcome_ref.output
            )
        })?;
    let step_result = run_store
        .read_step_results(run_id)?
        .into_iter()
        .rev()
        .find(|record| record.step_id == contract.from_step)
        .ok_or_else(|| {
            anyhow!(
                "run `{run_id}` did not persist output for step `{}`",
                contract.from_step
            )
        })?;
    Ok((contract.clone(), step_result))
}

fn validate_output_contract(
    global: &Path,
    project_root: Option<&Path>,
    contract: &WorkflowOutputContract,
    output: &serde_json::Value,
) -> anyhow::Result<()> {
    let schema_path = resolve_contract_path(global, project_root, &contract.schema)?;
    let schema_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&schema_path)?)
            .with_context(|| format!("parse contract schema {}", schema_path.display()))?;
    let compiled = JSONSchema::compile(&schema_json)
        .map_err(|err| anyhow!("compile {}: {err}", schema_path.display()))?;
    if let Err(errors) = compiled.validate(output) {
        let messages = errors
            .take(5)
            .map(|err| err.to_string())
            .collect::<Vec<_>>();
        bail!(
            "output failed schema `{}` validation: {}",
            contract.schema,
            messages.join("; ")
        );
    }
    Ok(())
}

fn resolve_contract_path(
    global: &Path,
    project_root: Option<&Path>,
    schema: &str,
) -> anyhow::Result<PathBuf> {
    if let Some(project_root) = project_root {
        let path = project_root
            .join(".rupu/contracts")
            .join(format!("{schema}.json"));
        if path.is_file() {
            return Ok(path);
        }
    }
    let global_path = global.join("contracts").join(format!("{schema}.json"));
    if global_path.is_file() {
        return Ok(global_path);
    }
    bail!("contract schema not found: {schema}")
}

fn resolve_retry_at(value: &str) -> anyhow::Result<String> {
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(value) {
        return Ok(parsed.with_timezone(&chrono::Utc).to_rfc3339());
    }
    Ok((chrono::Utc::now() + parse_duration(value)?).to_rfc3339())
}

fn should_run_claim(
    claim: &AutoflowClaimRecord,
    resolved: &ResolvedAutoflowWorkflow,
    claim_store: &AutoflowClaimStore,
    tick_started_at: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<bool> {
    if claim_store.read_active_lock(&claim.issue_ref)?.is_some() {
        return Ok(false);
    }
    match claim.status {
        ClaimStatus::Eligible
        | ClaimStatus::Claimed
        | ClaimStatus::Running
        | ClaimStatus::AwaitExternal => due_by_reconcile_interval(claim, resolved),
        ClaimStatus::RetryBackoff => due_by_retry_backoff(claim),
        ClaimStatus::AwaitHuman => Ok(false),
        ClaimStatus::Blocked | ClaimStatus::Complete | ClaimStatus::Released => Ok(false),
    }
    .map(|due| {
        if claim.pending_dispatch.is_some() {
            due && updated_before_tick(claim, tick_started_at).unwrap_or(false)
        } else {
            due
        }
    })
}

fn due_by_reconcile_interval(
    claim: &AutoflowClaimRecord,
    resolved: &ResolvedAutoflowWorkflow,
) -> anyhow::Result<bool> {
    let Some(interval) = resolved.autoflow()?.reconcile_every.as_deref() else {
        return Ok(false);
    };
    let last = chrono::DateTime::parse_from_rfc3339(&claim.updated_at)
        .with_context(|| format!("parse claim updated_at for `{}`", claim.issue_ref))?
        .with_timezone(&chrono::Utc);
    Ok(last + parse_duration(interval)? <= chrono::Utc::now())
}

fn due_by_retry_backoff(claim: &AutoflowClaimRecord) -> anyhow::Result<bool> {
    let Some(next_retry_at) = claim.next_retry_at.as_deref() else {
        return Ok(true);
    };
    let retry_at = chrono::DateTime::parse_from_rfc3339(next_retry_at)
        .with_context(|| format!("parse next_retry_at for `{}`", claim.issue_ref))?
        .with_timezone(&chrono::Utc);
    Ok(retry_at <= chrono::Utc::now())
}

fn updated_before_tick(
    claim: &AutoflowClaimRecord,
    tick_started_at: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<bool> {
    let updated = chrono::DateTime::parse_from_rfc3339(&claim.updated_at)
        .with_context(|| format!("parse updated_at for `{}`", claim.issue_ref))?
        .with_timezone(&chrono::Utc);
    Ok(updated < tick_started_at)
}

fn claim_lease_expired(claim: &AutoflowClaimRecord) -> anyhow::Result<bool> {
    let Some(lease_expires_at) = claim.lease_expires_at.as_deref() else {
        return Ok(false);
    };
    let lease = chrono::DateTime::parse_from_rfc3339(lease_expires_at)
        .with_context(|| format!("parse lease expiry for `{}`", claim.issue_ref))?
        .with_timezone(&chrono::Utc);
    Ok(lease <= chrono::Utc::now())
}

fn push_workflow_paths(dir: &Path, scope: &str, into: &mut BTreeMap<String, (String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|s| s.to_str());
        if !matches!(ext, Some("yaml" | "yml")) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        into.insert(stem.to_string(), (scope.to_string(), path));
    }
}

fn push_resolved_autoflow_paths(
    dir: &Path,
    scope: &str,
    global: &Path,
    project_root: Option<PathBuf>,
    repo_ref: String,
    preferred_checkout: PathBuf,
    cfg: Config,
    into: &mut Vec<ResolvedAutoflowWorkflow>,
) -> anyhow::Result<()> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in rd {
        let entry = entry?;
        let path = entry.path();
        let ext = path.extension().and_then(|s| s.to_str());
        if !matches!(ext, Some("yaml" | "yml")) {
            continue;
        }
        let resolved = resolve_autoflow_from_path(
            global,
            path,
            scope.to_string(),
            project_root.clone(),
            repo_ref.clone(),
            preferred_checkout.clone(),
            cfg.clone(),
        )?;
        into.push(resolved);
    }
    Ok(())
}

fn resolve_autoflow_workflow_for_repo(
    global: &Path,
    repo_store: &RepoRegistryStore,
    repo_ref: &str,
    name: &str,
) -> anyhow::Result<ResolvedAutoflowWorkflow> {
    let tracked = repo_store
        .load(repo_ref)?
        .ok_or_else(|| anyhow!("repo `{repo_ref}` is not tracked"))?;
    let preferred_checkout = PathBuf::from(&tracked.preferred_path);
    let project_root =
        paths::project_root_for(&preferred_checkout)?.or_else(|| Some(preferred_checkout.clone()));
    let cfg = resolve_config(global, project_root.as_deref())?;
    let workflow_path = locate_workflow_in(global, project_root.as_deref(), name)?;
    resolve_autoflow_from_path(
        global,
        workflow_path,
        "project".into(),
        project_root,
        repo_ref.to_string(),
        preferred_checkout,
        cfg,
    )
}

fn resolve_autoflow_from_path(
    _global: &Path,
    workflow_path: PathBuf,
    scope: String,
    project_root: Option<PathBuf>,
    repo_ref: String,
    preferred_checkout: PathBuf,
    cfg: Config,
) -> anyhow::Result<ResolvedAutoflowWorkflow> {
    let body = std::fs::read_to_string(&workflow_path)?;
    let workflow = Workflow::parse(&body)?;
    let enabled = workflow
        .autoflow
        .as_ref()
        .map(|autoflow| autoflow.enabled)
        .unwrap_or(false);
    if !enabled {
        bail!(
            "workflow `{}` does not declare `autoflow.enabled = true`",
            workflow.name
        );
    }
    Ok(ResolvedAutoflowWorkflow {
        scope,
        name: workflow.name.clone(),
        workflow,
        project_root,
        repo_ref,
        preferred_checkout,
        cfg,
    })
}

fn issue_payload(issue: &Issue) -> anyhow::Result<serde_json::Value> {
    let mut value = serde_json::to_value(issue)?;
    if let Some(obj) = value.as_object_mut() {
        obj.entry("number")
            .or_insert_with(|| serde_json::json!(issue.r.number));
        obj.entry("project")
            .or_insert_with(|| serde_json::json!(issue.r.project));
        obj.entry("tracker")
            .or_insert_with(|| serde_json::json!(issue.r.tracker.to_string()));
    }
    Ok(value)
}

fn resolve_config(global: &Path, project_root: Option<&Path>) -> anyhow::Result<Config> {
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.map(|root| root.join(".rupu/config.toml"));
    Ok(rupu_config::layer_files(
        Some(&global_cfg_path),
        project_cfg_path.as_deref(),
    )?)
}

fn build_issue_filter(autoflow: &rupu_orchestrator::Autoflow) -> IssueFilter {
    let state = match autoflow.selector.states.as_slice() {
        [rupu_orchestrator::AutoflowIssueState::Open] => Some(IssueState::Open),
        [rupu_orchestrator::AutoflowIssueState::Closed] => Some(IssueState::Closed),
        _ => None,
    };
    IssueFilter {
        state,
        labels: autoflow.selector.labels_all.clone(),
        author: None,
        limit: autoflow.selector.limit,
    }
}

fn selector_matches(autoflow: &rupu_orchestrator::Autoflow, issue: &Issue) -> bool {
    if !autoflow.selector.states.is_empty() {
        let state_matches = autoflow.selector.states.iter().any(|state| {
            matches!(
                (state, issue.state),
                (
                    rupu_orchestrator::AutoflowIssueState::Open,
                    IssueState::Open
                ) | (
                    rupu_orchestrator::AutoflowIssueState::Closed,
                    IssueState::Closed
                )
            )
        });
        if !state_matches {
            return false;
        }
    }
    autoflow
        .selector
        .labels_all
        .iter()
        .all(|label| issue.labels.iter().any(|existing| existing == label))
}

async fn fetch_issue(
    cfg: &Config,
    resolver: &dyn CredentialResolver,
    issue_ref: &IssueRef,
) -> anyhow::Result<Issue> {
    let registry = Arc::new(rupu_scm::Registry::discover(resolver, cfg).await);
    let connector = registry.issues(issue_ref.tracker).ok_or_else(|| {
        anyhow!(
            "no {} credential — run `rupu auth login --provider {}`",
            issue_ref.tracker,
            issue_ref.tracker
        )
    })?;
    connector
        .get_issue(issue_ref)
        .await
        .map_err(anyhow::Error::from)
}

fn parse_full_issue_target(target: &str) -> anyhow::Result<IssueRef> {
    match crate::run_target::parse_run_target(target) {
        Ok(crate::run_target::RunTarget::Issue {
            tracker,
            project,
            number,
        }) => Ok(IssueRef {
            tracker,
            project,
            number,
        }),
        _ => bail!(
            "autoflow run requires an issue target in `<platform>:<owner>/<repo>/issues/<N>` form"
        ),
    }
}

fn parse_issue_ref_text(value: &str) -> anyhow::Result<IssueRef> {
    let (tracker, rest) = value
        .split_once(':')
        .ok_or_else(|| anyhow!("invalid issue ref `{value}`"))?;
    let (project, number) = rest
        .rsplit_once("/issues/")
        .ok_or_else(|| anyhow!("invalid issue ref `{value}`"))?;
    Ok(IssueRef {
        tracker: IssueTracker::from_str(tracker).map_err(|err| anyhow!(err))?,
        project: project.to_string(),
        number: number.parse()?,
    })
}

fn parse_repo_ref(repo_ref: &str) -> anyhow::Result<(IssueTracker, String)> {
    let (tracker, project) = repo_ref
        .split_once(':')
        .ok_or_else(|| anyhow!("invalid repo ref `{repo_ref}`"))?;
    Ok((
        IssueTracker::from_str(tracker).map_err(|err| anyhow!(err))?,
        project.to_string(),
    ))
}

fn format_issue_ref(issue_ref: &IssueRef) -> String {
    format!(
        "{}:{}/issues/{}",
        issue_ref.tracker, issue_ref.project, issue_ref.number
    )
}

fn issue_repo_ref(issue_ref: &IssueRef) -> String {
    format!("{}:{}", issue_ref.tracker, issue_ref.project)
}

fn resolve_workspace_strategy(
    cfg: &rupu_config::AutoflowConfig,
    autoflow: &rupu_orchestrator::Autoflow,
) -> AutoflowWorkspaceStrategy {
    if let Some(workspace) = &autoflow.workspace {
        return workspace.strategy;
    }
    match cfg.checkout.unwrap_or(AutoflowCheckout::Worktree) {
        AutoflowCheckout::Worktree => AutoflowWorkspaceStrategy::Worktree,
        AutoflowCheckout::InPlace => AutoflowWorkspaceStrategy::InPlace,
    }
}

fn resolve_worktree_root(
    global: &Path,
    cfg: &rupu_config::AutoflowConfig,
) -> anyhow::Result<PathBuf> {
    match cfg.worktree_root.as_deref() {
        Some(path) => expand_user_path(path),
        None => Ok(paths::autoflow_worktrees_dir(global)),
    }
}

fn resolve_branch_name(
    template: Option<&str>,
    issue_payload: &serde_json::Value,
    issue_ref: &str,
) -> anyhow::Result<String> {
    if let Some(template) = template {
        let ctx = StepContext::new().with_issue(issue_payload.clone());
        let rendered = render_step_prompt(template, &ctx, RenderMode::Strict)?;
        let branch = rendered.trim();
        if !branch.is_empty() {
            return Ok(branch.to_string());
        }
    }
    Ok(format!("rupu/{}", issue_dir_name(issue_ref)))
}

fn expand_user_path(input: &str) -> anyhow::Result<PathBuf> {
    if input == "~" {
        let home = dirs::home_dir().ok_or_else(|| anyhow!("could not locate home directory"))?;
        return Ok(home);
    }
    if let Some(rest) = input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
    {
        let home = dirs::home_dir().ok_or_else(|| anyhow!("could not locate home directory"))?;
        return Ok(home.join(rest));
    }
    Ok(PathBuf::from(input))
}

fn parse_duration(value: &str) -> anyhow::Result<chrono::Duration> {
    let trimmed = value.trim();
    let unit = trimmed
        .chars()
        .last()
        .ok_or_else(|| anyhow!("invalid duration `{value}`"))?;
    let amount: i64 = trimmed[..trimmed.len().saturating_sub(1)]
        .parse()
        .map_err(|e| anyhow!("invalid duration `{value}`: {e}"))?;
    let duration = match unit {
        's' => chrono::Duration::seconds(amount),
        'm' => chrono::Duration::minutes(amount),
        'h' => chrono::Duration::hours(amount),
        'd' => chrono::Duration::days(amount),
        _ => bail!("invalid duration `{value}`"),
    };
    Ok(duration)
}

fn status_name(status: ClaimStatus) -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::GET;
    use httpmock::MockServer;
    use rupu_auth::in_memory::InMemoryResolver;
    use rupu_auth::StoredCredential;
    use rupu_orchestrator::{RunRecord, StepKind, StepResultRecord};
    use rupu_providers::AuthMode;
    use tokio::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    const COMPLETE_SCRIPT: &str = r#"
[
  {
    "AssistantText": {
      "text": "{\"status\":\"complete\",\"summary\":\"done\"}",
      "stop": "end_turn"
    }
  }
]
"#;

    fn init_git_repo(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        assert!(std::process::Command::new("git")
            .arg("init")
            .arg("-b")
            .arg("main")
            .arg(path)
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.email", "test@example.com"])
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.name", "Test User"])
            .status()
            .unwrap()
            .success());
        std::fs::write(path.join("README.md"), "hello\n").unwrap();
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["add", "README.md"])
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["commit", "-m", "init"])
            .status()
            .unwrap()
            .success());
    }

    fn write_autoflow_project(
        project: &Path,
        base_url: &str,
        workflow_name: &str,
        workflow_yaml: &str,
    ) {
        std::fs::create_dir_all(project.join(".rupu/agents")).unwrap();
        std::fs::create_dir_all(project.join(".rupu/workflows")).unwrap();
        std::fs::create_dir_all(project.join(".rupu/contracts")).unwrap();
        std::fs::write(
            project.join(".rupu/agents/echo.md"),
            "---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nyou echo.\n",
        )
        .unwrap();
        std::fs::write(
            project.join(format!(".rupu/workflows/{workflow_name}.yaml")),
            workflow_yaml,
        )
        .unwrap();
        std::fs::write(
            project.join(".rupu/contracts/autoflow_outcome_v1.json"),
            r#"{
  "type": "object",
  "required": ["status"],
  "properties": {
    "status": {
      "type": "string",
      "enum": ["continue", "await_human", "await_external", "retry", "blocked", "complete"]
    },
    "summary": { "type": "string" },
    "retry_after": { "type": "string" },
    "dispatch": {
      "type": "object",
      "required": ["workflow", "target", "inputs"],
      "properties": {
        "workflow": { "type": "string" },
        "target": { "type": "string" },
        "inputs": {
          "type": "object",
          "additionalProperties": { "type": "string" }
        }
      }
    }
  }
}"#,
        )
        .unwrap();
        std::fs::write(
            project.join(".rupu/config.toml"),
            format!(
                r#"[autoflow]
enabled = true
permission_mode = "bypass"
strict_templates = true

[scm.github]
base_url = "{base_url}"
"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn parse_duration_accepts_supported_units() {
        assert_eq!(
            parse_duration("10m").unwrap(),
            chrono::Duration::minutes(10)
        );
        assert_eq!(parse_duration("2h").unwrap(), chrono::Duration::hours(2));
    }

    #[test]
    fn issue_target_parser_rejects_repo_targets() {
        assert!(parse_full_issue_target("github:Section9Labs/rupu").is_err());
    }

    #[test]
    fn winners_prefer_priority_then_workflow_name() {
        let workflow = |name: &str, priority: i32| {
            Workflow::parse(&format!(
                r#"name: {name}
autoflow:
  enabled: true
  priority: {priority}
steps:
  - id: a
    agent: echo
    actions: []
    prompt: hi
"#
            ))
            .unwrap()
        };
        let resolved = |name: &str, priority: i32| ResolvedAutoflowWorkflow {
            scope: "project".into(),
            name: name.into(),
            workflow: workflow(name, priority),
            project_root: None,
            repo_ref: "github:Section9Labs/rupu".into(),
            preferred_checkout: PathBuf::from("/tmp/repo"),
            cfg: Config::default(),
        };
        let issue = Issue {
            r: IssueRef {
                tracker: IssueTracker::Github,
                project: "Section9Labs/rupu".into(),
                number: 42,
            },
            title: "x".into(),
            body: String::new(),
            state: IssueState::Open,
            labels: vec!["autoflow".into()],
            label_colors: BTreeMap::new(),
            author: "matt".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let winners = choose_winning_matches(vec![
            IssueMatch {
                resolved: resolved("beta", 50),
                issue_ref_text: "github:Section9Labs/rupu/issues/42".into(),
                issue: issue.clone(),
            },
            IssueMatch {
                resolved: resolved("alpha", 50),
                issue_ref_text: "github:Section9Labs/rupu/issues/42".into(),
                issue: issue.clone(),
            },
            IssueMatch {
                resolved: resolved("gamma", 100),
                issue_ref_text: "github:Section9Labs/rupu/issues/42".into(),
                issue,
            },
        ]);
        assert_eq!(
            winners
                .get("github:Section9Labs/rupu/issues/42")
                .unwrap()
                .resolved
                .workflow
                .name,
            "gamma"
        );
    }

    #[test]
    fn terminal_outcome_persists_dispatch_for_next_tick() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);
        write_autoflow_project(
            &project,
            "http://localhost.invalid",
            "controller",
            r#"name: controller
autoflow:
  enabled: true
  priority: 100
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
        );

        let resolved = resolve_autoflow_from_path(
            &global,
            project.join(".rupu/workflows/controller.yaml"),
            "project".into(),
            Some(project.clone()),
            "github:Section9Labs/rupu".into(),
            project.clone(),
            resolve_config(&global, Some(&project)).unwrap(),
        )
        .unwrap();
        let store = RunStore::new(global.join("runs"));
        std::fs::create_dir_all(global.join("runs")).unwrap();
        let run = RunRecord {
            id: "run_dispatch".into(),
            workflow_name: "controller".into(),
            status: RunStatus::Completed,
            inputs: BTreeMap::new(),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: project.clone(),
            transcript_dir: global.join("transcripts"),
            started_at: chrono::Utc::now(),
            finished_at: Some(chrono::Utc::now()),
            error_message: None,
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
            issue: None,
        };
        store.create(run, "name: controller\nsteps: []\n").unwrap();
        store
            .append_step_result(
                "run_dispatch",
                &StepResultRecord {
                    step_id: "decide".into(),
                    run_id: "step_1".into(),
                    transcript_path: global.join("transcripts/step.jsonl"),
                    output: r#"{"status":"continue","dispatch":{"workflow":"phase-delivery-cycle","target":"github:Section9Labs/rupu/issues/42","inputs":{"phase":"phase-1"}}}"#.into(),
                    success: true,
                    skipped: false,
                    rendered_prompt: "hi".into(),
                    kind: StepKind::Linear,
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                },
            )
            .unwrap();

        let mut claim = AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu/issues/42".into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            workflow: "controller".into(),
            status: ClaimStatus::Running,
            worktree_path: None,
            branch: None,
            last_run_id: Some("run_dispatch".into()),
            last_error: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        apply_terminal_run_to_claim(&global, &resolved, "run_dispatch", &mut claim).unwrap();
        assert_eq!(claim.status, ClaimStatus::Claimed);
        let dispatch = claim.pending_dispatch.expect("dispatch");
        assert_eq!(dispatch.workflow, "phase-delivery-cycle");
        assert_eq!(
            dispatch.inputs.get("phase").map(String::as_str),
            Some("phase-1")
        );
    }

    #[tokio::test]
    async fn tick_discovers_tracked_repo_and_runs_autoflow_cycle() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        let server = MockServer::start();
        let body = std::fs::read_to_string(
            "/Users/matt/Code/Oracle/rupu/.worktrees/feat-autoflow-phase-4/crates/rupu-scm/tests/fixtures/github/issues_list_happy.json",
        )
        .unwrap()
        .replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body(body);
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bug"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.number }}"
"#,
        );

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", COMPLETE_SCRIPT);

        tick_with_resolver(resolver).await.unwrap();

        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        let claim = claim_store
            .load("github:Section9Labs/rupu/issues/123")
            .unwrap()
            .unwrap();
        assert_eq!(claim.status, ClaimStatus::Complete);
        assert!(claim.last_run_id.is_some());
        assert!(claim
            .worktree_path
            .as_deref()
            .unwrap()
            .contains("issue-123"));
    }
}
