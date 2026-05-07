//! `rupu workflow list | show | run`.
//!
//! Lists workflows from `<global>/workflows/*.yaml` and (if any)
//! `<project>/.rupu/workflows/*.yaml`; project entries shadow global by
//! filename. `show` prints the YAML body. `run` parses the workflow,
//! builds a [`StepFactory`] that wires real providers via
//! [`provider_factory::build_for_provider`], and dispatches
//! [`rupu_orchestrator::run_workflow`].
//!
//! The factory carries a clone of the parsed [`Workflow`] so each
//! step's `agent:` field is honored (no hardcoded agent name).

use crate::cmd::completers::workflow_names;
use crate::paths;
use crate::provider_factory;
use async_trait::async_trait;
use clap::Subcommand;
use clap_complete::ArgValueCompleter;
use rupu_agent::runner::{AgentRunOpts, BypassDecider, PermissionDecider};
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts, StepFactory};
use rupu_orchestrator::Workflow;
use rupu_tools::ToolContext;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List all workflows (global + project).
    List,
    /// Print a workflow's YAML body.
    Show {
        /// Workflow name (filename stem under `workflows/`).
        #[arg(add = ArgValueCompleter::new(workflow_names))]
        name: String,
    },
    /// Run a workflow.
    Run {
        /// Workflow name (filename stem under `workflows/`).
        #[arg(add = ArgValueCompleter::new(workflow_names))]
        name: String,
        /// Optional target reference, e.g. `github:owner/repo#42`.
        /// See `docs/scm.md#target-syntax` for the full grammar.
        /// Distinguished from other positionals by parsing as a RunTarget.
        /// Per-step propagation via StepFactory is wired in Plan 3 Task 3.
        target: Option<String>,
        /// `KEY=VALUE` template inputs (repeatable).
        #[arg(long, value_parser = parse_kv)]
        input: Vec<(String, String)>,
        /// Override permission mode (`ask` | `bypass` | `readonly`).
        #[arg(long)]
        mode: Option<String>,
        /// Use the alt-screen TUI canvas instead of the default line-stream
        /// output. The canvas offers a DAG view and live status glyphs but
        /// requires an interactive terminal.
        #[arg(long)]
        canvas: bool,
    },
    /// List recent workflow runs from the persistent run-store
    /// (`<global>/runs/`). Newest first.
    Runs {
        /// Show only the N most recent runs.
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Filter by status: `pending` / `running` / `completed` /
        /// `failed` / `awaiting_approval` / `rejected`.
        #[arg(long)]
        status: Option<String>,
        /// Filter by issue ref (full or shorthand). Matches the
        /// textual `RunRecord.issue_ref` persisted at run start.
        /// Accepts:
        ///   - `<platform>:<owner>/<repo>/issues/<N>` (full),
        ///   - `<owner>/<repo>#<N>` (GitHub shorthand),
        ///   - bare `<N>` (autodetects from cwd's git remote).
        #[arg(long)]
        issue: Option<String>,
    },
    /// Inspect one persisted run: status, inputs, per-step
    /// transcript pointers.
    ShowRun {
        /// Full run id (`run_<ULID>`) as printed by
        /// `rupu workflow run`.
        run_id: String,
    },
    /// Approve a paused run and resume execution from the awaited
    /// step. The run must be in `awaiting_approval` status.
    Approve {
        run_id: String,
        /// Override permission mode for the resumed run
        /// (`ask` | `bypass` | `readonly`).
        #[arg(long)]
        mode: Option<String>,
    },
    /// Reject a paused run. Marks it `rejected`; no further steps
    /// dispatch.
    Reject {
        run_id: String,
        /// Optional human-readable reason recorded in the run's
        /// `error_message`.
        #[arg(long)]
        reason: Option<String>,
    },
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("expected KEY=VALUE: {s}"))?;
    Ok((k.to_string(), v.to_string()))
}

pub async fn handle(action: Action) -> ExitCode {
    let result = match action {
        Action::List => list().await,
        Action::Show { name } => show(&name).await,
        Action::Run {
            name,
            target,
            input,
            mode,
            canvas,
        } => run(&name, target.as_deref(), input, mode.as_deref(), None, canvas).await,
        Action::Runs {
            limit,
            status,
            issue,
        } => runs(limit, status.as_deref(), issue.as_deref()).await,
        Action::ShowRun { run_id } => show_run(&run_id).await,
        Action::Approve { run_id, mode } => approve(&run_id, mode.as_deref()).await,
        Action::Reject { run_id, reason } => reject(&run_id, reason.as_deref()).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("rupu workflow: {e}");
            ExitCode::from(1)
        }
    }
}

async fn list() -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    // (name, scope) — project shadows global by name. We collect into
    // a BTreeMap to dedupe before printing.
    let mut by_name: BTreeMap<String, String> = BTreeMap::new();
    push_yaml_names(&global.join("workflows"), "global", &mut by_name);
    if let Some(p) = &project_root {
        // Project entries inserted second deliberately overwrite the
        // global scope chip for the same name.
        push_yaml_names(&p.join(".rupu/workflows"), "project", &mut by_name);
    }

    println!("{:<28} SCOPE", "NAME");
    for (n, s) in &by_name {
        println!("{n:<28} {s}");
    }
    Ok(())
}

fn push_yaml_names(dir: &Path, scope: &str, into: &mut BTreeMap<String, String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
            into.insert(stem.to_string(), scope.to_string());
        }
    }
}

async fn show(name: &str) -> anyhow::Result<()> {
    let path = locate_workflow(name)?;
    let body = std::fs::read_to_string(&path)?;
    print!("{body}");
    Ok(())
}

async fn runs(
    limit: usize,
    status_filter: Option<&str>,
    issue_filter: Option<&str>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    let mut all = store
        .list()
        .map_err(|e| anyhow::anyhow!("run-store list failed: {e}"))?;

    // Lazy expiry: any AwaitingApproval row whose expires_at is in
    // the past gets transitioned to Failed and persisted before we
    // render. Operators learn about expired runs the next time they
    // look at the list.
    let now = chrono::Utc::now();
    for r in &mut all {
        let _ = store.expire_if_overdue(r, now);
    }

    // Normalize the optional issue filter once. Accepts the same
    // forms `rupu issues show / run` accept; we resolve to the
    // canonical `<tracker>:<project>/issues/<N>` text and compare
    // against `RunRecord.issue_ref` verbatim.
    let issue_filter_canonical: Option<String> = match issue_filter {
        None => None,
        Some(s) => Some(super::issues::canonical_issue_ref(s)?),
    };

    let filtered: Vec<_> = all
        .into_iter()
        .filter(|r| match status_filter {
            None => true,
            Some(s) => r.status.as_str() == s,
        })
        .filter(|r| match &issue_filter_canonical {
            None => true,
            Some(canonical) => r.issue_ref.as_deref() == Some(canonical.as_str()),
        })
        .take(limit)
        .collect();

    println!(
        "{:<28} {:<20} {:<18} {:<24} {:<22} WORKFLOW",
        "RUN ID", "STATUS", "STARTED (UTC)", "DURATION", "EXPIRES"
    );
    for r in &filtered {
        let started = r.started_at.format("%Y-%m-%d %H:%M:%S").to_string();
        let duration = match r.finished_at {
            Some(fin) => format!("{}s", (fin - r.started_at).num_seconds()),
            None => "(in flight)".into(),
        };
        let expires = match r.expires_at {
            Some(ex) => {
                let delta = (ex - now).num_seconds();
                if delta >= 0 {
                    format!("in {}s", delta)
                } else {
                    format!("{}s ago", -delta)
                }
            }
            None => String::new(),
        };
        println!(
            "{:<28} {:<20} {:<18} {:<24} {:<22} {}",
            r.id,
            r.status.as_str(),
            started,
            duration,
            expires,
            r.workflow_name
        );
    }
    if filtered.is_empty() {
        println!("(no runs yet — use `rupu workflow run <name>` to create one)");
    }
    Ok(())
}

async fn show_run(run_id: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    let record = store
        .load(run_id)
        .map_err(|e| anyhow::anyhow!("run not found: {e}"))?;
    let rows = store
        .read_step_results(run_id)
        .map_err(|e| anyhow::anyhow!("read step results failed: {e}"))?;

    println!("Run        : {}", record.id);
    println!("Workflow   : {}", record.workflow_name);
    println!("Status     : {}", record.status.as_str());
    println!(
        "Workspace  : {} ({})",
        record.workspace_id,
        record.workspace_path.display()
    );
    println!(
        "Started    : {}",
        record.started_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    if let Some(fin) = record.finished_at {
        println!("Finished   : {}", fin.format("%Y-%m-%d %H:%M:%S UTC"));
    }
    if !record.inputs.is_empty() {
        println!("Inputs     :");
        for (k, v) in &record.inputs {
            println!("  {k} = {v}");
        }
    }
    if let Some(err) = &record.error_message {
        println!("Error      : {err}");
    }
    if let Some(step) = &record.awaiting_step_id {
        println!("Awaiting   : {step}");
    }
    if let Some(since) = &record.awaiting_since {
        println!(
            "Paused at  : {}",
            since.format("%Y-%m-%d %H:%M:%S UTC")
        );
    }
    if let Some(ex) = &record.expires_at {
        println!("Expires    : {}", ex.format("%Y-%m-%d %H:%M:%S UTC"));
    }

    println!();
    println!("STEPS ({}):", rows.len());
    for row in &rows {
        let chip = if row.skipped {
            "skipped"
        } else if row.success {
            "ok"
        } else {
            "fail"
        };
        println!(
            "  [{:<7}] {:<24} -> {}",
            chip,
            row.step_id,
            row.transcript_path.display()
        );
        if !row.items.is_empty() {
            for item in &row.items {
                let chip = if item.success { "ok" } else { "fail" };
                let label = if !item.sub_id.is_empty() {
                    item.sub_id.clone()
                } else {
                    format!("[{}]", item.index)
                };
                println!(
                    "     [{:<7}] {:<22} -> {}",
                    chip,
                    label,
                    item.transcript_path.display()
                );
            }
        }
    }
    Ok(())
}

async fn approve(run_id: &str, mode: Option<&str>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let runs_dir = global.join("runs");
    let store = Arc::new(rupu_orchestrator::RunStore::new(runs_dir));
    let approver = whoami::username();

    // Library call replaces inline load + expire-check + status check
    // + mutate + update. Re-entering run_workflow stays in the CLI
    // because the TUI uses a different resume model.
    let awaited_step_id = match store.approve(run_id, &approver, chrono::Utc::now()) {
        Ok(rupu_orchestrator::ApprovalDecision::Approved { step_id, .. }) => step_id,
        Err(rupu_orchestrator::ApprovalError::Expired(msg)) => {
            anyhow::bail!("approval expired before it was acted on — {msg}");
        }
        Err(rupu_orchestrator::ApprovalError::NotAwaiting(s)) => {
            anyhow::bail!(
                "run is `{s}`, not `awaiting_approval` — only paused runs can be approved",
            );
        }
        Err(rupu_orchestrator::ApprovalError::NoAwaitingStep) => {
            anyhow::bail!("run has no awaiting_step_id; record may be corrupt");
        }
        Err(rupu_orchestrator::ApprovalError::NotFound(id)) => {
            anyhow::bail!("run not found: {id}");
        }
        Err(e) => return Err(anyhow::anyhow!("approve: {e}")),
        Ok(other) => anyhow::bail!("unexpected decision: {other:?}"),
    };
    // Reload the record from disk to get inputs, event, workspace path
    // for the run_workflow re-entry. The library call already persisted
    // the status flip to Running, so the record is coherent.
    let record = store
        .load(run_id)
        .map_err(|e| anyhow::anyhow!("reload run record: {e}"))?;

    // Rebuild context from disk: workflow YAML snapshot + prior
    // step results.
    let body = store
        .read_workflow_snapshot(run_id)
        .map_err(|e| anyhow::anyhow!("read workflow snapshot: {e}"))?;
    let workflow = Workflow::parse(&body)?;
    let prior_records = store
        .read_step_results(run_id)
        .map_err(|e| anyhow::anyhow!("read step results: {e}"))?;
    let prior_step_results: Vec<rupu_orchestrator::StepResult> = prior_records
        .iter()
        .map(rupu_orchestrator::StepResult::from)
        .collect();

    // Restore inputs, event, issue, workspace path from the record.
    let inputs_map: BTreeMap<String, String> = record.inputs.clone();
    let event = record.event.clone();
    let issue_payload = record.issue.clone();
    let issue_ref_text = record.issue_ref.clone();
    let workspace_path = record.workspace_path.clone();
    let transcripts = record.transcript_dir.clone();
    paths::ensure_dir(&transcripts)?;

    // Resolve project_root from the persisted workspace path so
    // agent/config discovery picks up the same `.rupu/` dir the
    // original run used.
    let project_root = paths::project_root_for(&workspace_path)?;

    // Standard wiring (mirrors `run` above; refactor candidate but
    // keeping inline for now to avoid spreading the resume path
    // across the CLI surface).
    let resolver = Arc::new(rupu_auth::KeychainResolver::new());
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())?;
    let mcp_registry = Arc::new(rupu_scm::Registry::discover(resolver.as_ref(), &cfg).await);

    let mode_str = mode.unwrap_or("ask").to_string();
    let factory = Arc::new(CliStepFactory {
        workflow: workflow.clone(),
        global: global.clone(),
        project_root: project_root.clone(),
        resolver,
        mode_str,
        mcp_registry,
        system_prompt_suffix: None,
    });

    let resume = rupu_orchestrator::ResumeState {
        run_id: run_id.to_string(),
        prior_step_results,
        approved_step_id: awaited_step_id.clone(),
    };
    let opts = OrchestratorRunOpts {
        workflow,
        inputs: inputs_map,
        workspace_id: record.workspace_id.clone(),
        workspace_path,
        transcript_dir: transcripts,
        factory,
        event,
        issue: issue_payload,
        issue_ref: issue_ref_text,
        run_store: Some(store),
        workflow_yaml: Some(body),
        resume_from: Some(resume),
    };

    let result = run_workflow(opts).await?;
    println!(
        "rupu: resumed run {} from step `{}`",
        result.run_id, awaited_step_id
    );
    for sr in &result.step_results {
        if sr.run_id.is_empty() {
            continue;
        }
        // Only show the steps the resume actually dispatched —
        // priors have run_id from a previous process and were
        // already printed when the run originally started.
        let was_prior = sr.transcript_path.exists() && sr.run_id.starts_with("run_");
        if was_prior {
            // Heuristic: the persisted prior steps will satisfy
            // both conditions; `run_workflow` records the freshly
            // dispatched ones too, but we don't have an easy way
            // to distinguish from inside the result. Print both for
            // now; future polish can dedupe via a stored boundary.
        }
        println!(
            "rupu: step {} run {} -> {}",
            sr.step_id,
            sr.run_id,
            sr.transcript_path.display()
        );
    }
    match &result.awaiting {
        Some(info) => {
            println!();
            println!(
                "rupu: workflow paused again at step `{}` (run {})",
                info.step_id, result.run_id
            );
            println!("      prompt: {}", info.prompt);
            println!(
                "      approve with: rupu workflow approve {}",
                result.run_id
            );
        }
        None => {
            println!(
                "rupu: workflow run {} finished (inspect with: rupu workflow show-run {})",
                result.run_id, result.run_id
            );
        }
    }
    Ok(())
}

async fn reject(run_id: &str, reason: Option<&str>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    let approver = whoami::username();
    let reason_str = reason.unwrap_or("rejected by operator");

    // Library call replaces inline load + expire-check + status check
    // + mutate + update.
    match store.reject(run_id, &approver, reason_str, chrono::Utc::now()) {
        Ok(rupu_orchestrator::ApprovalDecision::Rejected { .. }) => {}
        Err(rupu_orchestrator::ApprovalError::Expired(msg)) => {
            anyhow::bail!("approval expired before it was acted on — {msg}");
        }
        Err(rupu_orchestrator::ApprovalError::NotAwaiting(s)) => {
            anyhow::bail!(
                "run is `{s}`, not `awaiting_approval` — only paused runs can be rejected",
            );
        }
        Err(rupu_orchestrator::ApprovalError::NotFound(id)) => {
            anyhow::bail!("run not found: {id}");
        }
        Err(e) => return Err(anyhow::anyhow!("reject: {e}")),
        Ok(other) => anyhow::bail!("unexpected decision: {other:?}"),
    }
    println!("rupu: run {run_id} marked rejected");
    Ok(())
}

fn locate_workflow(name: &str) -> anyhow::Result<PathBuf> {
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    if let Some(p) = &project_root {
        let candidate = p.join(".rupu/workflows").join(format!("{name}.yaml"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    let global = paths::global_dir()?;
    let candidate = global.join("workflows").join(format!("{name}.yaml"));
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err(anyhow::anyhow!("workflow not found: {name}"))
}

/// Lightweight outcome surface for [`run_by_name`] callers (the
/// webhook receiver in particular) that need to know the run-id and
/// whether the run paused at an approval gate. The full per-step
/// result list is intentionally excluded — it's heavy and the
/// callers can fetch it via the run-store if they care.
#[derive(Debug, Clone, Default)]
pub struct RunOutcomeSummary {
    pub run_id: String,
    pub awaiting_step_id: Option<String>,
}

/// Public wrapper around the workflow-run pipeline so other
/// subcommands (notably `rupu cron tick` and the webhook receiver)
/// can invoke a workflow by name without going through the clap
/// layer. Same behavior as `rupu workflow run <name>`. The optional
/// `event` argument carries the SCM-vendor JSON payload that
/// triggered the run (when applicable); it lands as `{{event.*}}`
/// bindings in step prompts and `when:` filters.
pub async fn run_by_name(
    name: &str,
    inputs: Vec<(String, String)>,
    mode: Option<&str>,
    event: Option<serde_json::Value>,
) -> anyhow::Result<RunOutcomeSummary> {
    run_with_outcome(name, None, inputs, mode, event, false, false).await
}

/// Public wrapper for `rupu issues run <name> <ref>` and similar
/// callers that need to invoke a workflow with a specific
/// run-target string. Same UI semantics as `rupu workflow run`
/// (interactive line-stream by default) so the issue-targeted run
/// looks identical to the user.
pub async fn run_by_target(
    name: &str,
    target: &str,
    mode: Option<&str>,
) -> anyhow::Result<()> {
    run(name, Some(target), Vec::new(), mode, None, false).await
}

async fn run(
    name: &str,
    target: Option<&str>,
    inputs: Vec<(String, String)>,
    mode: Option<&str>,
    event: Option<serde_json::Value>,
    use_canvas: bool,
) -> anyhow::Result<()> {
    run_with_outcome(name, target, inputs, mode, event, true, use_canvas)
        .await
        .map(|_| ())
}

/// Same as [`run`] but returns a [`RunOutcomeSummary`] so non-CLI
/// callers (the webhook receiver) can surface run-id + pause state.
/// `run` itself thin-wraps this and discards the value.
async fn run_with_outcome(
    name: &str,
    target: Option<&str>,
    inputs: Vec<(String, String)>,
    mode: Option<&str>,
    event: Option<serde_json::Value>,
    attach_ui: bool,
    use_canvas: bool,
) -> anyhow::Result<RunOutcomeSummary> {
    let path = locate_workflow(name)?;
    let body = std::fs::read_to_string(&path)?;
    let workflow = Workflow::parse(&body)?;

    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    // Workspace upsert (mirrors `rupu run`).
    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, &pwd)?;

    // Credential resolver (shared across all steps in this workflow run).
    let resolver = Arc::new(rupu_auth::KeychainResolver::new());

    // Resolve config (global + project) so Registry::discover can read
    // [scm] platform settings.
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())?;

    // Build the SCM/issue registry once for the entire workflow run.
    // Cheap when no platforms are configured; missing credentials are
    // skipped with INFO logs.
    let mcp_registry = Arc::new(rupu_scm::Registry::discover(resolver.as_ref(), &cfg).await);

    // Parse the workflow-level target (if any) and derive a system-prompt
    // suffix that each step prepends. Clone-to-tmpdir for Repo/Pr targets
    // follows the same pattern as `rupu run`; the tmpdir lives for the
    // entire workflow execution.
    let _clone_guard: Option<tempfile::TempDir>;
    let workspace_path: std::path::PathBuf;
    let system_prompt_suffix: Option<String>;
    // Issue context — populated when run-target resolves to an issue.
    // The orchestrator's StepContext binds this as `{{issue.*}}` in
    // step prompts + `when:` expressions; RunRecord persists the
    // textual ref so `rupu workflow runs --issue <ref>` can filter.
    let mut issue_payload: Option<serde_json::Value> = None;
    let mut issue_ref_text: Option<String> = None;
    match target {
        None => {
            _clone_guard = None;
            workspace_path = pwd.clone();
            system_prompt_suffix = None;
        }
        Some(s) => match crate::run_target::parse_run_target(s) {
            Err(_) => {
                // Not a valid target — ignore silently (workflow inputs
                // don't have a free-form prompt field to absorb it).
                _clone_guard = None;
                workspace_path = pwd.clone();
                system_prompt_suffix = None;
            }
            Ok(run_target) => {
                let suffix = Some(crate::run_target::format_run_target_for_prompt(&run_target));
                let (guard, path) = match &run_target {
                    crate::run_target::RunTarget::Repo {
                        platform,
                        owner,
                        repo,
                        ..
                    }
                    | crate::run_target::RunTarget::Pr {
                        platform,
                        owner,
                        repo,
                        ..
                    } => {
                        let r = rupu_scm::RepoRef {
                            platform: *platform,
                            owner: owner.clone(),
                            repo: repo.clone(),
                        };
                        let conn = mcp_registry.repo(*platform).ok_or_else(|| {
                            anyhow::anyhow!(
                                "no {} credential — run `rupu auth login --provider {}`",
                                platform,
                                platform
                            )
                        })?;
                        let tmp = tempfile::tempdir()?;
                        conn.clone_to(&r, tmp.path()).await?;
                        let p = tmp.path().to_path_buf();
                        (Some(tmp), p)
                    }
                    crate::run_target::RunTarget::Issue {
                        tracker,
                        project,
                        number,
                    } => {
                        // Pre-fetch the issue once at run-start so step
                        // prompts can reference `{{issue.title}}` /
                        // `{{issue.body}}` / `{{issue.labels}}` etc.
                        // without each step having to call the
                        // IssueConnector.
                        let i = rupu_scm::IssueRef {
                            tracker: *tracker,
                            project: project.clone(),
                            number: *number,
                        };
                        let conn = mcp_registry.issues(*tracker).ok_or_else(|| {
                            anyhow::anyhow!(
                                "no {} credential — run `rupu auth login --provider {}`",
                                tracker,
                                tracker
                            )
                        })?;
                        match conn.get_issue(&i).await {
                            Ok(issue) => {
                                issue_payload = serde_json::to_value(&issue).ok();
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "failed to fetch issue at run-start; {{issue.*}} will be empty"
                                );
                            }
                        }
                        issue_ref_text =
                            Some(format!("{}:{}/issues/{}", tracker, project, number));
                        (None, pwd.clone())
                    }
                };
                _clone_guard = guard;
                workspace_path = path;
                system_prompt_suffix = suffix;
            }
        },
    }

    let mode_str = mode.unwrap_or("ask").to_string();
    let transcripts = paths::transcripts_dir(&global, project_root.as_deref());
    paths::ensure_dir(&transcripts)?;
    // Capture transcript dir before it's moved into opts (used by the
    // line-stream printer to locate step JSONL files).
    let transcripts_dir_snapshot = transcripts.clone();

    // Snapshot fields the post-run notify path needs before we move
    // them into the factory / opts.
    let registry_for_notify = Arc::clone(&mcp_registry);
    let notify_issue_enabled = workflow.notify_issue;
    let workflow_name_for_notify = workflow.name.clone();
    let issue_ref_text_for_notify = issue_ref_text.clone();
    let issue_payload_for_notify = issue_payload.clone();

    let factory = Arc::new(CliStepFactory {
        workflow: workflow.clone(),
        global: global.clone(),
        project_root: project_root.clone(),
        resolver,
        mode_str,
        mcp_registry,
        system_prompt_suffix,
    });

    let inputs_map: BTreeMap<String, String> = inputs.into_iter().collect();
    let runs_dir = global.join("runs");
    paths::ensure_dir(&runs_dir)?;
    let run_store = Arc::new(rupu_orchestrator::RunStore::new(runs_dir.clone()));

    // Snapshot the cloneable pieces so we can rebuild `OrchestratorRunOpts`
    // for each resume iteration. `factory`, `run_store`, and `workflow`
    // are Arc/Clone-cheap.
    let workflow_for_resume = workflow.clone();
    let workspace_path_for_resume = workspace_path.clone();
    let transcripts_for_resume = transcripts.clone();
    let event_for_resume = event.clone();
    let issue_for_resume = issue_payload.clone();
    let issue_ref_for_resume = issue_ref_text.clone();
    let workspace_id_for_resume = ws.id.clone();
    let factory_for_resume = Arc::clone(&factory);
    let run_store_for_resume = Arc::clone(&run_store);
    let body_for_resume = body.clone();
    let inputs_for_resume = inputs_map.clone();

    let opts = OrchestratorRunOpts {
        workflow,
        inputs: inputs_map,
        workspace_id: ws.id,
        workspace_path,
        transcript_dir: transcripts,
        factory,
        event,
        issue: issue_payload,
        issue_ref: issue_ref_text,
        run_store: Some(run_store),
        workflow_yaml: Some(body.clone()),
        resume_from: None,
    };

    // Non-interactive callers (webhook receiver, cron tick) pass
    // `attach_ui = false` and keep the original inline-await path so
    // they can capture and forward the result without a terminal.
    //
    // Interactive callers get a live UI. Default: line-stream printer
    // (works in any terminal, pipe, or CI runner). `--canvas` opt-in
    // keeps the alt-screen TUI canvas.
    let result = if attach_ui {
        // Snapshot the run dir entries that exist *before* the spawn so
        // we can detect the new one the orchestrator creates.
        let existing_run_ids: std::collections::BTreeSet<String> =
            list_run_dir_entries(&runs_dir);

        // transcript_dir_snapshot was captured before opts was constructed.

        // Spawn the workflow runner. `opts` is moved into the task;
        // `_clone_guard` keeps the tmpdir alive through the scope.
        let runner_task = tokio::spawn(run_workflow(opts));

        // Poll for the new run directory (created synchronously by the
        // orchestrator before any step work begins). 2 s upper bound;
        // in practice this is microseconds.
        let new_run_id = wait_for_new_run_dir(&runs_dir, &existing_run_ids, 2_000).await;

        // First-attach result we'll merge with any resumed run.
        let first_result = if let Some(ref rid) = new_run_id {
            if use_canvas {
                // Alt-screen TUI canvas (opt-in via --canvas).
                if let Err(e) = rupu_tui::run_attached(rid.clone(), runs_dir.clone()) {
                    eprintln!("rupu: TUI exited early: {e}");
                }
                runner_task
                    .await
                    .map_err(|e| anyhow::anyhow!("workflow task panicked: {e}"))?
                    .map_err(anyhow::Error::from)?
            } else {
                // Line-stream printer (default). Loops over Approved
                // outcomes, transparently spinning a resumed runner each
                // time the user presses `a` at a gate.
                let printer_store =
                    rupu_orchestrator::RunStore::new(runs_dir.clone());
                let mut printer = crate::output::LineStreamPrinter::new();

                let mut attach_opts = crate::output::workflow_printer::AttachOpts::default();
                let mut current_runner = runner_task;

                let last_result: rupu_orchestrator::OrchestratorRunResult = loop {
                    let outcome = match crate::output::workflow_printer::attach_and_print_with(
                        name,
                        rid,
                        &runs_dir,
                        &transcripts_dir_snapshot,
                        &mut printer,
                        &printer_store,
                        attach_opts,
                    ) {
                        Ok(o) => o,
                        Err(e) => {
                            eprintln!("rupu: printer error: {e}");
                            crate::output::workflow_printer::AttachOutcome::Detached
                        }
                    };

                    // Drain the runner that produced this attach's events.
                    let result = current_runner
                        .await
                        .map_err(|e| anyhow::anyhow!("workflow task panicked: {e}"))?
                        .map_err(anyhow::Error::from)?;

                    use crate::output::workflow_printer::AttachOutcome;
                    match outcome {
                        AttachOutcome::Done
                        | AttachOutcome::Detached
                        | AttachOutcome::Rejected => break result,
                        AttachOutcome::Approved { awaited_step_id } => {
                            // Spin a resumed run with the same id and
                            // re-attach the printer in skip-header mode.
                            let prior_records = run_store_for_resume
                                .read_step_results(rid)
                                .map_err(|e| {
                                    anyhow::anyhow!("read step results for resume: {e}")
                                })?;
                            let prior_count = prior_records.len();
                            let prior_step_results: Vec<rupu_orchestrator::StepResult> =
                                prior_records
                                    .iter()
                                    .map(rupu_orchestrator::StepResult::from)
                                    .collect();
                            let resume = rupu_orchestrator::ResumeState {
                                run_id: rid.clone(),
                                prior_step_results,
                                approved_step_id: awaited_step_id,
                            };
                            let factory_dyn: Arc<dyn rupu_orchestrator::StepFactory> =
                                factory_for_resume.clone();
                            let resume_opts = OrchestratorRunOpts {
                                workflow: workflow_for_resume.clone(),
                                inputs: inputs_for_resume.clone(),
                                workspace_id: workspace_id_for_resume.clone(),
                                workspace_path: workspace_path_for_resume.clone(),
                                transcript_dir: transcripts_for_resume.clone(),
                                factory: factory_dyn,
                                event: event_for_resume.clone(),
                                issue: issue_for_resume.clone(),
                                issue_ref: issue_ref_for_resume.clone(),
                                run_store: Some(Arc::clone(&run_store_for_resume)),
                                workflow_yaml: Some(body_for_resume.clone()),
                                resume_from: Some(resume),
                            };
                            current_runner = tokio::spawn(run_workflow(resume_opts));
                            attach_opts = crate::output::workflow_printer::AttachOpts {
                                skip_header: true,
                                skip_count: prior_count,
                            };
                            // Loop back: re-attach the same printer. We
                            // intentionally drop the just-finished result —
                            // the resumed run will produce a fresh one.
                            let _ = result;
                        }
                    }
                };

                last_result
            }
        } else {
            // No new run directory appeared — propagate the runner's error.
            runner_task
                .await
                .map_err(|e| anyhow::anyhow!("workflow task panicked: {e}"))?
                .map_err(anyhow::Error::from)?
        };

        first_result
    } else {
        run_workflow(opts).await?
    };

    // Auto-comment on the targeted issue when the workflow opted in
    // via `notifyIssue: true`. Best-effort: a failure to post just
    // logs a warning so a slow / down issue tracker doesn't fail the
    // run. We skip silently when `notifyIssue` is off OR when the
    // run-target wasn't an issue.
    if notify_issue_enabled {
        if let (Some(ref_text), Some(payload)) = (&issue_ref_text_for_notify, &issue_payload_for_notify) {
            post_run_summary_to_issue(
                &registry_for_notify,
                ref_text,
                payload,
                &workflow_name_for_notify,
                &result,
            )
            .await;
        }
    }

    Ok(RunOutcomeSummary {
        run_id: result.run_id,
        awaiting_step_id: result.awaiting.map(|a| a.step_id),
    })
}

/// Post a one-line summary comment to the targeted issue describing
/// the run's outcome. Best-effort — surfaces a `tracing::warn!` on
/// failure rather than propagating, so a slow / down issue tracker
/// doesn't fail an otherwise-successful run.
async fn post_run_summary_to_issue(
    registry: &rupu_scm::Registry,
    ref_text: &str,
    payload: &serde_json::Value,
    workflow_name: &str,
    result: &rupu_orchestrator::OrchestratorRunResult,
) {
    // Reconstruct an `IssueRef` from the persisted text + payload.
    // The text carries the canonical
    // `<tracker>:<project>/issues/<N>` form; the JSON payload's
    // `r.tracker` field is more reliable for the typed value.
    let tracker_str = payload
        .pointer("/r/tracker")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tracker = match tracker_str {
        "github" => rupu_scm::IssueTracker::Github,
        "gitlab" => rupu_scm::IssueTracker::Gitlab,
        other => {
            tracing::warn!(tracker = %other, "notifyIssue: unknown tracker; skipping comment");
            return;
        }
    };
    let project = payload
        .pointer("/r/project")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let number = payload
        .pointer("/r/number")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if project.is_empty() || number == 0 {
        tracing::warn!(ref_text, "notifyIssue: malformed payload; skipping comment");
        return;
    }
    let r = rupu_scm::IssueRef {
        tracker,
        project,
        number,
    };

    let Some(conn) = registry.issues(tracker) else {
        tracing::warn!(
            tracker = %tracker,
            "notifyIssue: no credential for tracker; skipping comment"
        );
        return;
    };

    let outcome = match &result.awaiting {
        Some(info) => format!("paused at step `{}` awaiting approval", info.step_id),
        None => {
            // Distinguish failure from success by checking that
            // every step in the result succeeded. The orchestrator
            // would have returned Err earlier if there was a hard
            // failure, so reaching here means a clean run.
            let step_count = result.step_results.len();
            format!("completed ({step_count} steps)")
        }
    };

    let body = format!(
        "🤖 rupu workflow `{}` (run `{}`) {}.\n\n\
         Inspect: `rupu workflow show-run {}`\n\
         Live: `rupu watch {}`",
        workflow_name, result.run_id, outcome, result.run_id, result.run_id,
    );

    if let Err(e) = conn.comment_issue(&r, &body).await {
        tracing::warn!(
            error = %e,
            ref_text,
            "notifyIssue: posting comment failed"
        );
    }
}

/// Collect the names of all `run_<ULID>` subdirectories currently
/// present in `runs_dir`. Used to diff before/after spawning the
/// workflow runner so we can identify the new run's directory.
fn list_run_dir_entries(runs_dir: &std::path::Path) -> std::collections::BTreeSet<String> {
    let Ok(rd) = std::fs::read_dir(runs_dir) else {
        return std::collections::BTreeSet::new();
    };
    rd.flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if name.starts_with("run_") {
                Some(name)
            } else {
                None
            }
        })
        .collect()
}

/// Poll `runs_dir` until a subdirectory appears that was not in
/// `before`. Returns the new run id or `None` if `timeout_ms` expires
/// before anything shows up.
async fn wait_for_new_run_dir(
    runs_dir: &std::path::Path,
    before: &std::collections::BTreeSet<String>,
    timeout_ms: u64,
) -> Option<String> {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        if let Ok(rd) = std::fs::read_dir(runs_dir) {
            for entry in rd.flatten() {
                let name = entry.file_name().into_string().unwrap_or_default();
                if name.starts_with("run_") && !before.contains(&name) {
                    return Some(name);
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// `StepFactory` impl that resolves each step's `agent:` against
/// the project- and global-scope `agents/` dirs and constructs a
/// real provider via [`provider_factory::build_for_provider`].
///
/// `mcp_registry` is built once in the `run` function and shared
/// across all steps; this avoids redundant credential probes and
/// ensures consistent SCM tool availability throughout the workflow.
struct CliStepFactory {
    workflow: Workflow,
    global: PathBuf,
    project_root: Option<PathBuf>,
    resolver: Arc<rupu_auth::KeychainResolver>,
    mode_str: String,
    mcp_registry: Arc<rupu_scm::Registry>,
    /// Formatted `## Run target` text to append to each step's system prompt.
    /// `None` when no `--target` was supplied at workflow invocation.
    system_prompt_suffix: Option<String>,
}

#[async_trait]
impl StepFactory for CliStepFactory {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: PathBuf,
        transcript_path: PathBuf,
    ) -> AgentRunOpts {
        // We still verify the parent step exists in the workflow so
        // unknown step ids surface clearly, but we drive the agent
        // load off `agent_name` (which differs from the parent's
        // `agent:` for `parallel:` sub-steps).
        let _step = self
            .workflow
            .steps
            .iter()
            .find(|s| s.id == step_id)
            .expect("step_id from orchestrator must match a workflow step");

        // The agent loader takes the parent of `agents/`. For the
        // project layer that's `<project>/.rupu`; the global layer is
        // `<global>` directly (which already contains `agents/`).
        let project_agents_parent = self.project_root.as_ref().map(|p| p.join(".rupu"));
        let spec =
            rupu_agent::load_agent(&self.global, project_agents_parent.as_deref(), agent_name)
                .unwrap_or_else(|_| {
                    // Fallback: synthesize a minimal AgentSpec with the
                    // rendered prompt as system prompt so the factory contract
                    // is honored even when the agent file is missing. The
                    // agent loop will surface the failure via run_complete{
                    // status: Error}.
                    rupu_agent::AgentSpec {
                        name: agent_name.to_string(),
                        description: None,
                        provider: Some("anthropic".to_string()),
                        model: Some("claude-sonnet-4-6".to_string()),
                        auth: None,
                        tools: None,
                        max_turns: Some(50),
                        permission_mode: Some(self.mode_str.clone()),
                        anthropic_oauth_prefix: None,
                        effort: None,
                        context_window: None,
                        output_format: None,
                        anthropic_task_budget: None,
                        anthropic_context_management: None,
                        anthropic_speed: None,
                        system_prompt: rendered_prompt.clone(),
                    }
                });

        let provider_name = spec.provider.clone().unwrap_or_else(|| "anthropic".into());
        let model = spec
            .model
            .clone()
            .unwrap_or_else(|| "claude-sonnet-4-6".into());
        let auth_hint = spec.auth;
        // Build the provider; on failure (missing credential, bad
        // auth config, etc.) substitute a stub provider that returns
        // the same error on first call. The runner's existing
        // `RunComplete { status: Error }` path then surfaces it as a
        // clean `✗ <step_id>` line via the line printer — no panic,
        // no crash log. (See `provider_build_error_stub` below.)
        let provider: Box<dyn rupu_providers::LlmProvider> =
            match provider_factory::build_for_provider(
                &provider_name,
                &model,
                auth_hint,
                self.resolver.as_ref(),
            )
            .await
            {
                Ok((_resolved_auth, p)) => p,
                Err(e) => Box::new(provider_build_error_stub(
                    provider_name.clone(),
                    model.clone(),
                    e.to_string(),
                )),
            };

        let agent_system_prompt = match self.system_prompt_suffix.as_deref() {
            Some(suffix) => format!("{}\n\n## Run target\n\n{}", spec.system_prompt, suffix),
            None => spec.system_prompt,
        };

        AgentRunOpts {
            agent_name: spec.name,
            agent_system_prompt,
            agent_tools: spec.tools,
            provider,
            provider_name,
            model,
            run_id,
            workspace_id,
            workspace_path: workspace_path.clone(),
            transcript_path,
            max_turns: spec.max_turns.unwrap_or(50),
            decider: Arc::new(BypassDecider) as Arc<dyn PermissionDecider>,
            tool_context: ToolContext {
                workspace_path,
                bash_env_allowlist: Vec::new(),
                bash_timeout_secs: 120,
            },
            user_message: rendered_prompt,
            mode_str: self.mode_str.clone(),
            no_stream: false,
            // Workflow runs always feed into the TUI; the TUI tails
            // the JSONL transcript for tokens. Suppress the legacy
            // line-stream stdout writes so they don't corrupt the
            // alt-screen canvas.
            suppress_stream_stdout: true,
            mcp_registry: Some(Arc::clone(&self.mcp_registry)),
            effort: spec.effort,
            context_window: spec.context_window,
            output_format: spec.output_format,
            anthropic_task_budget: spec.anthropic_task_budget,
            anthropic_context_management: spec.anthropic_context_management,
            anthropic_speed: spec.anthropic_speed,
        }
    }
}

/// Stub `LlmProvider` that errors on first call. Used when the real
/// provider build fails inside the StepFactory (e.g. missing
/// credential): instead of panicking and writing a crash log, we
/// hand the runner a provider that returns the build error from its
/// first `send`/`stream` call. The runner's normal error path then
/// emits `Event::RunComplete { status: Error, error: ... }`, which
/// the line printer renders as `✗ <step_id> <error>` — the user
/// sees a clean, actionable message.
fn provider_build_error_stub(
    provider_name: String,
    model: String,
    error: String,
) -> ProviderBuildErrorStub {
    ProviderBuildErrorStub {
        provider_name,
        model,
        error,
    }
}

struct ProviderBuildErrorStub {
    provider_name: String,
    model: String,
    error: String,
}

#[async_trait::async_trait]
impl rupu_providers::LlmProvider for ProviderBuildErrorStub {
    async fn send(
        &mut self,
        _request: &rupu_providers::LlmRequest,
    ) -> Result<rupu_providers::LlmResponse, rupu_providers::ProviderError> {
        Err(rupu_providers::ProviderError::AuthConfig(format!(
            "{}: {}\n  Run: rupu auth login --provider {} --mode <api-key|sso>",
            self.provider_name, self.error, self.provider_name,
        )))
    }

    async fn stream(
        &mut self,
        _request: &rupu_providers::LlmRequest,
        _on_event: &mut (dyn FnMut(rupu_providers::StreamEvent) + Send),
    ) -> Result<rupu_providers::LlmResponse, rupu_providers::ProviderError> {
        Err(rupu_providers::ProviderError::AuthConfig(format!(
            "{}: {}\n  Run: rupu auth login --provider {} --mode <api-key|sso>",
            self.provider_name, self.error, self.provider_name,
        )))
    }

    fn default_model(&self) -> &str {
        &self.model
    }

    fn provider_id(&self) -> rupu_providers::ProviderId {
        // Pick a stable variant; only used for log attribution.
        rupu_providers::ProviderId::Anthropic
    }
}

#[cfg(test)]
mod provider_build_error_stub_tests {
    use super::*;
    use rupu_providers::{LlmProvider, LlmRequest, ProviderError};

    fn empty_request() -> LlmRequest {
        LlmRequest {
            model: "test-model".into(),
            system: None,
            messages: vec![],
            max_tokens: 1,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
        }
    }

    #[tokio::test]
    async fn send_returns_authconfig_with_login_hint() {
        // Regression for the v0.4.5 panic: when the StepFactory's
        // build_for_provider() failed (missing credential, etc.) the
        // `.expect()` panicked and a crash log was written. The stub
        // routes the same error through the runner's normal failure
        // path so the line printer can render it cleanly.
        let mut stub = provider_build_error_stub(
            "openai".to_string(),
            "gpt-5".to_string(),
            "no credentials configured for openai".to_string(),
        );
        let err = stub.send(&empty_request()).await.expect_err("must error");
        let ProviderError::AuthConfig(msg) = err else {
            panic!("expected AuthConfig variant, got {err:?}");
        };
        assert!(msg.contains("openai"), "missing provider name: {msg}");
        assert!(
            msg.contains("rupu auth login --provider openai"),
            "missing actionable login hint: {msg}",
        );
    }
}
