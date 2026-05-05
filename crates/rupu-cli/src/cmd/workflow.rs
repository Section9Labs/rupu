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

use crate::paths;
use crate::provider_factory;
use async_trait::async_trait;
use clap::Subcommand;
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
        name: String,
    },
    /// Run a workflow.
    Run {
        /// Workflow name (filename stem under `workflows/`).
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
        } => run(&name, target.as_deref(), input, mode.as_deref(), None).await,
        Action::Runs { limit, status } => runs(limit, status.as_deref()).await,
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

async fn runs(limit: usize, status_filter: Option<&str>) -> anyhow::Result<()> {
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

    let filtered: Vec<_> = all
        .into_iter()
        .filter(|r| match status_filter {
            None => true,
            Some(s) => r.status.as_str() == s,
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

    let mut record = store
        .load(run_id)
        .map_err(|e| anyhow::anyhow!("run not found: {e}"))?;
    // Expire stale paused runs lazily — no ticker daemon needed for v1.
    if store
        .expire_if_overdue(&mut record, chrono::Utc::now())
        .map_err(|e| anyhow::anyhow!("expire check: {e}"))?
    {
        anyhow::bail!(
            "approval expired before it was acted on — {}",
            record
                .error_message
                .as_deref()
                .unwrap_or("paused run timed out")
        );
    }
    if record.status != rupu_orchestrator::RunStatus::AwaitingApproval {
        anyhow::bail!(
            "run is `{}`, not `awaiting_approval` — only paused runs can be approved",
            record.status.as_str()
        );
    }
    let awaited_step_id = record
        .awaiting_step_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("run has no awaiting_step_id; record may be corrupt"))?;

    // Mutate the record back to Running BEFORE re-entering the
    // loop. If the re-entered workflow pauses again at another
    // approval gate, `run_workflow` will flip the status to
    // AwaitingApproval again and refresh the awaiting_*
    // fields.
    record.status = rupu_orchestrator::RunStatus::Running;
    record.awaiting_step_id = None;
    record.approval_prompt = None;
    record.awaiting_since = None;
    record.expires_at = None;
    record.error_message = None;
    store
        .update(&record)
        .map_err(|e| anyhow::anyhow!("update run record: {e}"))?;

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

    // Restore inputs, event, workspace path from the record.
    let inputs_map: BTreeMap<String, String> = record.inputs.clone();
    let event = record.event.clone();
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
    let mut record = store
        .load(run_id)
        .map_err(|e| anyhow::anyhow!("run not found: {e}"))?;
    if store
        .expire_if_overdue(&mut record, chrono::Utc::now())
        .map_err(|e| anyhow::anyhow!("expire check: {e}"))?
    {
        anyhow::bail!(
            "approval expired before it was acted on — {}",
            record
                .error_message
                .as_deref()
                .unwrap_or("paused run timed out")
        );
    }
    if record.status != rupu_orchestrator::RunStatus::AwaitingApproval {
        anyhow::bail!(
            "run is `{}`, not `awaiting_approval` — only paused runs can be rejected",
            record.status.as_str()
        );
    }
    record.status = rupu_orchestrator::RunStatus::Rejected;
    record.finished_at = Some(chrono::Utc::now());
    record.error_message = Some(
        reason
            .map(str::to_string)
            .unwrap_or_else(|| "rejected by operator".into()),
    );
    store
        .update(&record)
        .map_err(|e| anyhow::anyhow!("update run record: {e}"))?;
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
    run_with_outcome(name, None, inputs, mode, event).await
}

async fn run(
    name: &str,
    target: Option<&str>,
    inputs: Vec<(String, String)>,
    mode: Option<&str>,
    event: Option<serde_json::Value>,
) -> anyhow::Result<()> {
    run_with_outcome(name, target, inputs, mode, event)
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
                    _ => (None, pwd.clone()),
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
    let run_store = Arc::new(rupu_orchestrator::RunStore::new(runs_dir));
    let opts = OrchestratorRunOpts {
        workflow,
        inputs: inputs_map,
        workspace_id: ws.id,
        workspace_path,
        transcript_dir: transcripts,
        factory,
        event,
        run_store: Some(run_store),
        workflow_yaml: Some(body.clone()),
        resume_from: None,
    };

    let result = run_workflow(opts).await?;
    for sr in &result.step_results {
        if sr.run_id.is_empty() {
            // Skipped step (`when:` falsy) — no transcript path.
            continue;
        }
        println!(
            "rupu: step {} run {} -> {}",
            sr.step_id,
            sr.run_id,
            sr.transcript_path.display()
        );
    }
    match (&result.run_id, &result.awaiting) {
        (rid, Some(info)) if !rid.is_empty() => {
            println!();
            println!(
                "rupu: workflow paused at step `{}` awaiting approval (run {})",
                info.step_id, rid
            );
            println!("      prompt: {}", info.prompt);
            if let Some(ex) = info.expires_at {
                println!("      expires: {}", ex.format("%Y-%m-%d %H:%M:%S UTC"));
            }
            println!("      approve with: rupu workflow approve {}", rid);
            println!("      reject  with: rupu workflow reject {}", rid);
        }
        (rid, None) if !rid.is_empty() => {
            println!(
                "rupu: workflow run {} (inspect with: rupu workflow show-run {})",
                rid, rid
            );
        }
        _ => {}
    }
    Ok(RunOutcomeSummary {
        run_id: result.run_id,
        awaiting_step_id: result.awaiting.map(|a| a.step_id),
    })
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
        let (_resolved_auth, provider) = provider_factory::build_for_provider(
            &provider_name,
            &model,
            auth_hint,
            self.resolver.as_ref(),
        )
        .await
        .expect("provider build failed in step factory");

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
