//! `rupu autoflow ...` — manual/autonomous workflow entrypoints.

use crate::cmd::completers::workflow_names;
use crate::cmd::issues::canonical_issue_ref;
use crate::cmd::workflow::{
    locate_workflow_in, run_with_explicit_context, ExplicitWorkflowRunContext,
};
use crate::paths;
use clap::Subcommand;
use clap_complete::ArgValueCompleter;
use comfy_table::Cell;
use rupu_config::AutoflowCheckout;
use rupu_orchestrator::templates::{render_step_prompt, RenderMode, StepContext};
use rupu_orchestrator::{AutoflowWorkspaceStrategy, Workflow};
use rupu_scm::IssueRef;
use rupu_workspace::{
    ensure_issue_worktree, issue_dir_name, AutoflowClaimRecord, AutoflowClaimStore, ClaimStatus,
    RepoRegistryStore,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

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
    /// Summarize persisted autoflow claim state.
    Status,
    /// Inspect persisted autoflow claims.
    Claims,
    /// Force-release one claim.
    Release { r#ref: String },
}

pub async fn handle(action: Action) -> ExitCode {
    let result = match action {
        Action::List => list().await,
        Action::Show { name } => show(&name).await,
        Action::Run { name, target, mode } => run(&name, &target, mode.as_deref()).await,
        Action::Status => status().await,
        Action::Claims => claims().await,
        Action::Release { r#ref } => release(&r#ref).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
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
        .ok_or_else(|| {
            anyhow::anyhow!("workflow `{name}` does not declare `autoflow.enabled = true`")
        })?;

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

async fn run(name: &str, target: &str, mode: Option<&str>) -> anyhow::Result<()> {
    let issue_ref = parse_full_issue_target(target)?;
    let issue_ref_text = canonical_issue_ref(target)?;
    let repo_ref = issue_repo_ref(&issue_ref);

    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let repo_store = RepoRegistryStore {
        root: paths::repos_dir(&global),
    };
    let tracked = repo_store.load(&repo_ref)?.ok_or_else(|| {
        anyhow::anyhow!(
            "repo `{repo_ref}` is not tracked; run `rupu repos attach {repo_ref} <path>`"
        )
    })?;
    let preferred_checkout = PathBuf::from(&tracked.preferred_path);

    let project_root =
        paths::project_root_for(&preferred_checkout)?.or_else(|| Some(preferred_checkout.clone()));
    let path = locate_workflow_in(&global, project_root.as_deref(), name)?;
    let body = std::fs::read_to_string(&path)?;
    let workflow = Workflow::parse(&body)?;
    let autoflow = workflow
        .autoflow
        .as_ref()
        .filter(|a| a.enabled)
        .ok_or_else(|| {
            anyhow::anyhow!("workflow `{name}` does not declare `autoflow.enabled = true`")
        })?;

    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())?;

    let resolver = Arc::new(rupu_auth::KeychainResolver::new());
    let registry = Arc::new(rupu_scm::Registry::discover(resolver.as_ref(), &cfg).await);
    let issue_connector = registry.issues(issue_ref.tracker).ok_or_else(|| {
        anyhow::anyhow!(
            "no {} credential — run `rupu auth login --provider {}`",
            issue_ref.tracker,
            issue_ref.tracker
        )
    })?;
    let issue = issue_connector.get_issue(&issue_ref).await?;
    let issue_payload = serde_json::to_value(&issue)?;

    let workspace_strategy = resolve_workspace_strategy(&cfg.autoflow, autoflow);
    let branch = resolve_branch_name(
        autoflow
            .workspace
            .as_ref()
            .and_then(|w| w.branch.as_deref()),
        &issue_payload,
        &issue_ref_text,
    )?;
    let workspace_path = match workspace_strategy {
        AutoflowWorkspaceStrategy::Worktree => {
            let root = resolve_worktree_root(&global, &cfg.autoflow)?;
            ensure_issue_worktree(
                &preferred_checkout,
                &root,
                &repo_ref,
                &issue_ref_text,
                &branch,
                tracked.default_branch.as_deref().or(Some("HEAD")),
            )?
            .path
        }
        AutoflowWorkspaceStrategy::InPlace => preferred_checkout.clone(),
    };
    let runtime_project_root =
        paths::project_root_for(&workspace_path)?.or_else(|| Some(workspace_path.clone()));

    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, &workspace_path)?;

    let claim_store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    let owner = format!("{}:pid-{}", whoami::username(), std::process::id());
    let lease_expires_at = match autoflow
        .claim
        .as_ref()
        .and_then(|claim| claim.ttl.as_deref())
    {
        Some(ttl) => Some((chrono::Utc::now() + parse_duration(ttl)?).to_rfc3339()),
        None => None,
    };
    let _lock = claim_store.try_acquire_active_lock(
        &issue_ref_text,
        &owner,
        lease_expires_at.as_deref(),
    )?;

    let mut claim = claim_store
        .load(&issue_ref_text)?
        .unwrap_or(AutoflowClaimRecord {
            issue_ref: issue_ref_text.clone(),
            repo_ref: repo_ref.clone(),
            workflow: workflow.name.clone(),
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
    claim.workflow = workflow.name.clone();
    claim.status = ClaimStatus::Running;
    claim.worktree_path = Some(workspace_path.display().to_string());
    claim.branch = Some(branch.clone());
    claim.claim_owner = Some(owner.clone());
    claim.lease_expires_at = lease_expires_at.clone();
    claim.updated_at = chrono::Utc::now().to_rfc3339();
    claim_store.save(&claim)?;

    let run_result = run_with_explicit_context(
        name,
        ExplicitWorkflowRunContext {
            project_root: runtime_project_root,
            workspace_path,
            workspace_id: ws.id,
            inputs: Vec::new(),
            mode: mode
                .map(ToOwned::to_owned)
                .or_else(|| cfg.autoflow.permission_mode.clone())
                .unwrap_or_else(|| "bypass".to_string()),
            event: None,
            issue: Some(issue_payload.clone()),
            issue_ref: Some(issue_ref_text.clone()),
            system_prompt_suffix: Some(crate::run_target::format_run_target_for_prompt(
                &crate::run_target::RunTarget::Issue {
                    tracker: issue_ref.tracker,
                    project: issue_ref.project.clone(),
                    number: issue_ref.number,
                },
            )),
            attach_ui: true,
            use_canvas: false,
            run_id_override: None,
            strict_templates: cfg.autoflow.strict_templates.unwrap_or(true),
        },
    )
    .await;

    match run_result {
        Ok(summary) => {
            claim.status = if summary.awaiting_step_id.is_some() {
                ClaimStatus::AwaitHuman
            } else {
                ClaimStatus::Claimed
            };
            claim.last_run_id = Some(summary.run_id);
            claim.last_error = None;
            claim.updated_at = chrono::Utc::now().to_rfc3339();
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
        _ => anyhow::bail!(
            "autoflow run requires an issue target in `<platform>:<owner>/<repo>/issues/<N>` form"
        ),
    }
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
        let home =
            dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not locate home directory"))?;
        return Ok(home);
    }
    if let Some(rest) = input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
    {
        let home =
            dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not locate home directory"))?;
        return Ok(home.join(rest));
    }
    Ok(PathBuf::from(input))
}

fn parse_duration(value: &str) -> anyhow::Result<chrono::Duration> {
    let trimmed = value.trim();
    let unit = trimmed
        .chars()
        .last()
        .ok_or_else(|| anyhow::anyhow!("invalid duration `{value}`"))?;
    let amount: i64 = trimmed[..trimmed.len().saturating_sub(1)]
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid duration `{value}`: {e}"))?;
    let duration = match unit {
        's' => chrono::Duration::seconds(amount),
        'm' => chrono::Duration::minutes(amount),
        'h' => chrono::Duration::hours(amount),
        'd' => chrono::Duration::days(amount),
        _ => anyhow::bail!("invalid duration `{value}`"),
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
}
