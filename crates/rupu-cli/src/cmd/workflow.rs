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
use tracing::warn;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List all workflows (global + project).
    List,
    /// Print a workflow's YAML body.
    Show {
        /// Workflow name (filename stem under `workflows/`).
        #[arg(add = ArgValueCompleter::new(workflow_names))]
        name: String,
        /// Disable colored output (also honored: `NO_COLOR` env var).
        #[arg(long)]
        no_color: bool,
        /// syntect theme name. Default: `base16-ocean.dark`.
        #[arg(long)]
        theme: Option<String>,
        /// Force pager. Default: page when stdout is a tty.
        #[arg(long, conflicts_with = "no_pager")]
        pager: bool,
        /// Disable pager.
        #[arg(long)]
        no_pager: bool,
    },
    /// Open a workflow file in `$VISUAL` / `$EDITOR`. Validates the
    /// YAML on save (warn-only).
    Edit {
        /// Workflow name (filename stem under `workflows/`).
        name: String,
        /// Force the project shadow (`.rupu/workflows/<name>.yaml`) or
        /// the global file (`<global>/workflows/<name>.yaml`). Default:
        /// prefer project if it exists, else global.
        #[arg(long, value_parser = ["global", "project"])]
        scope: Option<String>,
        /// Override the editor (e.g. `--editor "code --wait"`).
        /// Default: `$VISUAL` then `$EDITOR` then `vi`.
        #[arg(long)]
        editor: Option<String>,
    },
    /// Run a workflow.
    Run {
        /// Workflow name (filename stem under `workflows/`).
        #[arg(add = ArgValueCompleter::new(workflow_names))]
        name: String,
        /// Optional run-target. Accepts repo (`github:owner/repo`,
        /// `gitlab:group/proj`), PR (`github:owner/repo#42`), or
        /// issue (`github:owner/repo/issues/42`). Repo / PR targets
        /// clone to a tmpdir for the run; issue targets pre-fetch
        /// the issue payload and bind it as `{{ issue.* }}` in step
        /// prompts.
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
        /// Accepts `<platform>:<owner>/<repo>/issues/<N>` (full),
        /// `<owner>/<repo>#<N>` (GitHub shorthand), or bare `<N>`
        /// (autodetects from cwd's git remote).
        #[arg(long)]
        issue: Option<String>,
        /// Disable colored output (also honored: `NO_COLOR` env,
        /// `[ui].color = "never"` in config).
        #[arg(long)]
        no_color: bool,
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
        Action::Show {
            name,
            no_color,
            theme,
            pager,
            no_pager,
        } => {
            let pager_flag = if pager {
                Some(true)
            } else if no_pager {
                Some(false)
            } else {
                None
            };
            show(&name, no_color, theme.as_deref(), pager_flag).await
        }
        Action::Edit {
            name,
            scope,
            editor,
        } => edit(&name, scope.as_deref(), editor.as_deref()).await,
        Action::Run {
            name,
            target,
            input,
            mode,
            canvas,
        } => {
            run(
                &name,
                target.as_deref(),
                input,
                mode.as_deref(),
                None,
                canvas,
            )
            .await
        }
        Action::Runs {
            limit,
            status,
            issue,
            no_color,
        } => runs(limit, status.as_deref(), issue.as_deref(), no_color).await,
        Action::ShowRun { run_id } => show_run(&run_id).await,
        Action::Approve { run_id, mode } => approve(&run_id, mode.as_deref()).await,
        Action::Reject { run_id, reason } => reject(&run_id, reason.as_deref()).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
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

async fn show(
    name: &str,
    no_color: bool,
    theme: Option<&str>,
    pager_flag: Option<bool>,
) -> anyhow::Result<()> {
    let path = locate_workflow(name)?;
    let body = std::fs::read_to_string(&path)?;

    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg =
        rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref()).unwrap_or_default();

    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, no_color, theme, pager_flag);
    let rendered = crate::cmd::ui::highlight_yaml(&body, &prefs);
    crate::cmd::ui::paginate(&rendered, &prefs)?;
    Ok(())
}

async fn edit(
    name: &str,
    scope: Option<&str>,
    editor_override: Option<&str>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    let target = resolve_workflow_path(name, scope, &global, project_root.as_deref())?;
    let scope_label = if target.starts_with(&global) {
        "global"
    } else {
        "project"
    };
    println!("editing {} ({scope_label})", target.display());

    crate::cmd::editor::open_for_edit(editor_override, &target)?;

    match Workflow::parse_file(&target) {
        Ok(_) => {
            println!("✓ {name}: workflow YAML parses cleanly");
            Ok(())
        }
        Err(e) => {
            eprintln!("⚠ {name}: failed to re-parse after save:\n  {e}");
            Ok(())
        }
    }
}

/// Pick the on-disk file to edit. With `--scope` set we honor it
/// strictly; without it we prefer the project shadow if present and
/// fall back to global. Tries `.yaml` first, then `.yml`.
fn resolve_workflow_path(
    name: &str,
    scope: Option<&str>,
    global: &Path,
    project_root: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    let candidates_for = |dir: PathBuf| -> Vec<PathBuf> {
        vec![
            dir.join(format!("{name}.yaml")),
            dir.join(format!("{name}.yml")),
        ]
    };

    let project_dir = project_root.map(|p| p.join(".rupu").join("workflows"));
    let global_dir = global.join("workflows");

    let pick =
        |dir: PathBuf| -> Option<PathBuf> { candidates_for(dir).into_iter().find(|p| p.exists()) };

    match scope {
        Some("project") => match project_dir {
            Some(d) => pick(d.clone()).ok_or_else(|| {
                anyhow::anyhow!(
                    "workflow `{name}` not found at project scope ({}/{name}.{{yaml,yml}})",
                    d.display()
                )
            }),
            None => Err(anyhow::anyhow!(
                "no project root detected; cannot use --scope project"
            )),
        },
        Some("global") => pick(global_dir.clone()).ok_or_else(|| {
            anyhow::anyhow!(
                "workflow `{name}` not found at global scope ({}/{name}.{{yaml,yml}})",
                global_dir.display()
            )
        }),
        Some(other) => Err(anyhow::anyhow!(
            "invalid --scope `{other}` (expected `global` or `project`)"
        )),
        None => {
            if let Some(d) = project_dir {
                if let Some(p) = pick(d) {
                    return Ok(p);
                }
            }
            pick(global_dir).ok_or_else(|| {
                anyhow::anyhow!("workflow `{name}` not found in project or global workflows dir")
            })
        }
    }
}

async fn runs(
    limit: usize,
    status_filter: Option<&str>,
    issue_filter: Option<&str>,
    no_color: bool,
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

    if filtered.is_empty() {
        let scope = match (status_filter, issue_filter_canonical.as_deref()) {
            (None, None) => "(no runs yet — use `rupu workflow run <name>` to create one)".into(),
            (Some(s), None) => format!("(no runs match status={s})"),
            (None, Some(i)) => format!("(no runs match issue={i})"),
            (Some(s), Some(i)) => format!("(no runs match status={s}, issue={i})"),
        };
        println!("{scope}");
        return Ok(());
    }

    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let cfg = layered_config_workflow(&global, project_root.as_deref());
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, no_color, None, None);

    let mut table = crate::output::tables::new_table();
    table.set_header(vec![
        "RUN ID",
        "STATUS",
        "STARTED (UTC)",
        "DURATION",
        "EXPIRES",
        "TOKENS",
        "COST",
        "WORKFLOW",
    ]);
    for r in &filtered {
        let started = r.started_at.format("%Y-%m-%d %H:%M:%S").to_string();
        let duration = match r.finished_at {
            Some(fin) => format!("{}s", (fin - r.started_at).num_seconds()),
            None => "(in flight)".into(),
        };
        let expires_cell = match r.expires_at {
            Some(ex) => {
                let delta = (ex - now).num_seconds();
                crate::output::tables::relative_time_cell(delta, &prefs)
            }
            None => comfy_table::Cell::new(""),
        };

        // Aggregate Usage events from this specific run's per-step
        // transcripts (NOT the project-wide `.rupu/transcripts/`
        // directory, which would double-count every run's tokens).
        // Step-result records pin down each agent invocation's
        // transcript path, including per-panelist sub-runs.
        let agg = aggregate_run_usage_from_store(&store, &r.id);
        let tokens_cell = comfy_table::Cell::new(format_tokens_cell(&agg));
        let cost_cell = run_cost_cell(&agg, &cfg.pricing, &prefs);

        table.add_row(vec![
            comfy_table::Cell::new(&r.id),
            crate::output::tables::status_cell(r.status.as_str(), &prefs),
            comfy_table::Cell::new(started),
            comfy_table::Cell::new(duration),
            expires_cell,
            tokens_cell,
            cost_cell,
            comfy_table::Cell::new(&r.workflow_name),
        ]);
    }
    println!("{table}");
    Ok(())
}

/// Per-step transcripts for one run, sourced from the run's
/// `step_results.jsonl`. Includes panel sub-run transcripts
/// (`items[].transcript_path`) so a panel-of-3 review counts all
/// three reviewers' tokens.
///
/// This is the version used by `rupu workflow runs`: scoping to one
/// run via the run-store avoids the double-count you'd get from
/// scanning the project-wide `transcript_dir` (which collects every
/// run's transcripts together).
fn aggregate_run_usage_from_store(
    store: &rupu_orchestrator::RunStore,
    run_id: &str,
) -> Vec<rupu_transcript::UsageRow> {
    let Ok(records) = store.read_step_results(run_id) else {
        return Vec::new();
    };
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    for rec in &records {
        paths.push(rec.transcript_path.clone());
        for item in &rec.items {
            paths.push(item.transcript_path.clone());
        }
    }
    rupu_transcript::aggregate(&paths, rupu_transcript::TimeWindow::default())
}

/// Compact `input + output` token total for the runs table. Returns
/// `—` when the run had no Usage events (fresh in-flight run, or one
/// that failed before the first turn).
fn format_tokens_cell(rows: &[rupu_transcript::UsageRow]) -> String {
    let total: u64 = rows.iter().map(|r| r.input_tokens + r.output_tokens).sum();
    if total == 0 {
        return "—".into();
    }
    if total >= 1_000_000 {
        format!("{:.2}M", total as f64 / 1_000_000.0)
    } else if total >= 1_000 {
        format!("{:.1}K", total as f64 / 1_000.0)
    } else {
        total.to_string()
    }
}

/// Sum costs across every `(provider, model, agent)` triple in the
/// run. Renders `$X.XX` when at least one row had pricing, dim `—`
/// when none did.
fn run_cost_cell(
    rows: &[rupu_transcript::UsageRow],
    pricing: &rupu_config::PricingConfig,
    prefs: &crate::cmd::ui::UiPrefs,
) -> comfy_table::Cell {
    let mut total = 0.0f64;
    let mut any = false;
    for r in rows {
        if let Some(p) = crate::pricing::lookup(pricing, &r.provider, &r.model, &r.agent) {
            total += p.cost_usd(r.input_tokens, r.output_tokens, r.cached_tokens);
            any = true;
        }
    }
    if !any {
        return if prefs.use_color() {
            comfy_table::Cell::new("\x1b[2m—\x1b[0m")
        } else {
            comfy_table::Cell::new("—")
        };
    }
    comfy_table::Cell::new(format!("${total:.4}"))
}

fn layered_config_workflow(
    global: &std::path::Path,
    project_root: Option<&std::path::Path>,
) -> rupu_config::Config {
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.map(|p| p.join(".rupu/config.toml"));
    rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())
        .unwrap_or_default()
}

async fn show_run(run_id: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let cfg = layered_config_workflow(&global, project_root.as_deref());
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, false, None, None);
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    let record = store.load(run_id).map_err(|e| {
        anyhow::anyhow!(
            "run not found: {e}\n  hint: list runs with `rupu workflow runs` \
                 or start one with `rupu workflow run <name>`"
        )
    })?;
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
        println!("Paused at  : {}", since.format("%Y-%m-%d %H:%M:%S UTC"));
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

    // ── Usage summary ────────────────────────────────────────────
    // Aggregate every transcript referenced by this run's step
    // results, group by (provider, model, agent), and render with
    // the same table style `rupu usage` uses. Cost lookup honors the
    // layered pricing config + built-in defaults.
    let usage_rows = aggregate_run_usage_from_store(&store, run_id);
    if !usage_rows.is_empty() {
        println!();
        println!("USAGE:");
        let mut t = crate::output::tables::new_table();
        t.set_header(vec![
            "PROVIDER",
            "MODEL",
            "AGENT",
            "INPUT",
            "OUTPUT",
            "CACHED",
            "COST (USD)",
        ]);
        let mut total_in = 0u64;
        let mut total_out = 0u64;
        let mut total_cached = 0u64;
        let mut total_cost = 0.0f64;
        let mut any_priced = false;
        for r in &usage_rows {
            let cost = crate::pricing::lookup(&cfg.pricing, &r.provider, &r.model, &r.agent)
                .map(|p| p.cost_usd(r.input_tokens, r.output_tokens, r.cached_tokens));
            if let Some(c) = cost {
                total_cost += c;
                any_priced = true;
            }
            let cost_str = match cost {
                Some(c) => format!("${c:.4}"),
                None => "—".into(),
            };
            t.add_row(vec![
                comfy_table::Cell::new(&r.provider),
                comfy_table::Cell::new(&r.model),
                comfy_table::Cell::new(&r.agent),
                comfy_table::Cell::new(r.input_tokens.to_string()),
                comfy_table::Cell::new(r.output_tokens.to_string()),
                comfy_table::Cell::new(r.cached_tokens.to_string()),
                comfy_table::Cell::new(cost_str),
            ]);
            total_in += r.input_tokens;
            total_out += r.output_tokens;
            total_cached += r.cached_tokens;
        }
        t.add_row(vec![
            comfy_table::Cell::new("TOTAL"),
            comfy_table::Cell::new(""),
            comfy_table::Cell::new(""),
            comfy_table::Cell::new(total_in.to_string()),
            comfy_table::Cell::new(total_out.to_string()),
            comfy_table::Cell::new(total_cached.to_string()),
            comfy_table::Cell::new(if any_priced {
                format!("${total_cost:.4}")
            } else {
                "—".into()
            }),
        ]);
        println!("{t}");
    }

    let _ = prefs; // reserved for future colorized cost cells in show_run
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
            anyhow::bail!(
                "run not found: {id}\n  hint: \
                 list paused runs with `rupu workflow runs --status awaiting_approval`"
            );
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
    let dispatcher = crate::cmd::dispatch::CliAgentDispatcher::new(
        global.clone(),
        project_root.clone(),
        record.workspace_id.clone(),
        workspace_path.clone(),
        Arc::clone(&resolver),
        mode_str.clone(),
        Arc::clone(&mcp_registry),
        Arc::clone(&store),
    );
    let dispatcher_dyn: Arc<dyn rupu_tools::AgentDispatcher> = dispatcher;
    let factory = Arc::new(CliStepFactory {
        workflow: workflow.clone(),
        global: global.clone(),
        project_root: project_root.clone(),
        resolver,
        mode_str,
        mcp_registry,
        system_prompt_suffix: None,
        dispatcher: Some(dispatcher_dyn),
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
        run_id_override: None,
        strict_templates: false,
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
            anyhow::bail!(
                "run not found: {id}\n  hint: \
                 list paused runs with `rupu workflow runs --status awaiting_approval`"
            );
        }
        Err(e) => return Err(anyhow::anyhow!("reject: {e}")),
        Ok(other) => anyhow::bail!("unexpected decision: {other:?}"),
    }
    println!("rupu: run {run_id} marked rejected");
    Ok(())
}

pub(crate) fn locate_workflow_in(
    global: &Path,
    project_root: Option<&Path>,
    name: &str,
) -> anyhow::Result<PathBuf> {
    if let Some(project_root) = project_root {
        let candidate = project_root
            .join(".rupu/workflows")
            .join(format!("{name}.yaml"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    let candidate = global.join("workflows").join(format!("{name}.yaml"));
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err(anyhow::anyhow!("workflow not found: {name}"))
}

fn locate_workflow(name: &str) -> anyhow::Result<PathBuf> {
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let global = paths::global_dir()?;
    locate_workflow_in(&global, project_root.as_deref(), name)
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

#[derive(Debug, Clone)]
pub struct ExplicitWorkflowRunContext {
    pub project_root: Option<PathBuf>,
    pub workspace_path: PathBuf,
    pub workspace_id: String,
    pub inputs: Vec<(String, String)>,
    pub mode: String,
    pub event: Option<serde_json::Value>,
    pub issue: Option<serde_json::Value>,
    pub issue_ref: Option<String>,
    pub system_prompt_suffix: Option<String>,
    pub attach_ui: bool,
    pub use_canvas: bool,
    pub run_id_override: Option<String>,
    pub strict_templates: bool,
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
    run_with_outcome(name, None, inputs, mode, event, false, false, None).await
}

/// Variant of [`run_by_name`] that pins the run-id. Used by the
/// `rupu cron tick` polled-events tier, which derives a deterministic
/// id (`evt-<workflow>-<vendor>-<delivery>`) so re-delivered or
/// re-polled events don't double-fire. On collision, the underlying
/// `RunStore::create` returns `AlreadyExists`; this wrapper surfaces
/// that as `Err(...)` and the caller logs + skips.
pub async fn run_by_name_with_run_id(
    name: &str,
    inputs: Vec<(String, String)>,
    mode: Option<&str>,
    event: Option<serde_json::Value>,
    run_id: String,
) -> anyhow::Result<RunOutcomeSummary> {
    run_with_outcome(name, None, inputs, mode, event, false, false, Some(run_id)).await
}

/// Public wrapper for `rupu issues run <name> <ref>` and similar
/// callers that need to invoke a workflow with a specific
/// run-target string. Same UI semantics as `rupu workflow run`
/// (interactive line-stream by default) so the issue-targeted run
/// looks identical to the user.
pub async fn run_by_target(name: &str, target: &str, mode: Option<&str>) -> anyhow::Result<()> {
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
    run_with_outcome(name, target, inputs, mode, event, true, use_canvas, None)
        .await
        .map(|_| ())
}

/// Same as [`run`] but returns a [`RunOutcomeSummary`] so non-CLI
/// callers (the webhook receiver) can surface run-id + pause state.
/// `run` itself thin-wraps this and discards the value.
#[allow(clippy::too_many_arguments)]
async fn run_with_outcome(
    name: &str,
    target: Option<&str>,
    inputs: Vec<(String, String)>,
    mode: Option<&str>,
    event: Option<serde_json::Value>,
    attach_ui: bool,
    use_canvas: bool,
    run_id_override: Option<String>,
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
    if let Err(err) = crate::cmd::repos::auto_track_checkout(&global, &pwd) {
        warn!(path = %pwd.display(), error = %err, "failed to auto-track checkout");
    }

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
                        issue_ref_text = Some(format!("{}:{}/issues/{}", tracker, project, number));
                        (None, pwd.clone())
                    }
                };
                _clone_guard = guard;
                workspace_path = path;
                system_prompt_suffix = suffix;
            }
        },
    }

    execute_workflow_invocation(
        name,
        workflow,
        body,
        global,
        ExplicitWorkflowRunContext {
            project_root: project_root.clone(),
            workspace_path,
            workspace_id: ws.id,
            inputs,
            mode: mode.unwrap_or("ask").to_string(),
            event,
            issue: issue_payload,
            issue_ref: issue_ref_text,
            system_prompt_suffix,
            attach_ui,
            use_canvas,
            run_id_override,
            strict_templates: false,
        },
    )
    .await
}

pub async fn run_with_explicit_context(
    name: &str,
    ctx: ExplicitWorkflowRunContext,
) -> anyhow::Result<RunOutcomeSummary> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let path = locate_workflow_in(&global, ctx.project_root.as_deref(), name)?;
    let body = std::fs::read_to_string(&path)?;
    let workflow = Workflow::parse(&body)?;
    execute_workflow_invocation(name, workflow, body, global, ctx).await
}

async fn execute_workflow_invocation(
    name: &str,
    workflow: Workflow,
    body: String,
    global: PathBuf,
    ctx: ExplicitWorkflowRunContext,
) -> anyhow::Result<RunOutcomeSummary> {
    let resolver = Arc::new(rupu_auth::KeychainResolver::new());
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = ctx
        .project_root
        .as_ref()
        .map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())?;
    let mcp_registry = Arc::new(rupu_scm::Registry::discover(resolver.as_ref(), &cfg).await);

    let transcripts = paths::transcripts_dir(&global, ctx.project_root.as_deref());
    paths::ensure_dir(&transcripts)?;
    let transcripts_dir_snapshot = transcripts.clone();

    let registry_for_notify = Arc::clone(&mcp_registry);
    let notify_issue_enabled = workflow.notify_issue;
    let workflow_name_for_notify = workflow.name.clone();
    let issue_ref_text_for_notify = ctx.issue_ref.clone();
    let issue_payload_for_notify = ctx.issue.clone();

    // Run-store first so the dispatcher can be constructed alongside the
    // factory and threaded onto every step's `ToolContext`.
    let inputs_map: BTreeMap<String, String> = ctx.inputs.into_iter().collect();
    let runs_dir = global.join("runs");
    paths::ensure_dir(&runs_dir)?;
    let run_store = Arc::new(rupu_orchestrator::RunStore::new(runs_dir.clone()));

    let dispatcher = crate::cmd::dispatch::CliAgentDispatcher::new(
        global.clone(),
        ctx.project_root.clone(),
        ctx.workspace_id.clone(),
        ctx.workspace_path.clone(),
        Arc::clone(&resolver),
        ctx.mode.clone(),
        Arc::clone(&mcp_registry),
        Arc::clone(&run_store),
    );
    let dispatcher_dyn: Arc<dyn rupu_tools::AgentDispatcher> = dispatcher;

    let factory = Arc::new(CliStepFactory {
        workflow: workflow.clone(),
        global: global.clone(),
        project_root: ctx.project_root.clone(),
        resolver,
        mode_str: ctx.mode.clone(),
        mcp_registry,
        system_prompt_suffix: ctx.system_prompt_suffix.clone(),
        dispatcher: Some(dispatcher_dyn),
    });

    let workflow_for_resume = workflow.clone();
    let workspace_path_for_resume = ctx.workspace_path.clone();
    let transcripts_for_resume = transcripts.clone();
    let event_for_resume = ctx.event.clone();
    let issue_for_resume = ctx.issue.clone();
    let issue_ref_for_resume = ctx.issue_ref.clone();
    let workspace_id_for_resume = ctx.workspace_id.clone();
    let factory_for_resume = Arc::clone(&factory);
    let run_store_for_resume = Arc::clone(&run_store);
    let body_for_resume = body.clone();
    let inputs_for_resume = inputs_map.clone();
    let strict_templates = ctx.strict_templates;

    let opts = OrchestratorRunOpts {
        workflow,
        inputs: inputs_map,
        workspace_id: ctx.workspace_id,
        workspace_path: ctx.workspace_path,
        transcript_dir: transcripts,
        factory,
        event: ctx.event,
        issue: ctx.issue,
        issue_ref: ctx.issue_ref,
        run_store: Some(run_store),
        workflow_yaml: Some(body.clone()),
        resume_from: None,
        run_id_override: ctx.run_id_override,
        strict_templates,
    };

    let result = if ctx.attach_ui {
        let existing_run_ids: std::collections::BTreeSet<String> = list_run_dir_entries(&runs_dir);
        let runner_task = tokio::spawn(run_workflow(opts));
        let new_run_id = wait_for_new_run_dir(&runs_dir, &existing_run_ids, 2_000).await;

        if let Some(ref rid) = new_run_id {
            if ctx.use_canvas {
                if let Err(e) = rupu_tui::run_attached(rid.clone(), runs_dir.clone()) {
                    eprintln!("rupu: TUI exited early: {e}");
                }
                runner_task
                    .await
                    .map_err(|e| anyhow::anyhow!("workflow task panicked: {e}"))?
                    .map_err(anyhow::Error::from)?
            } else {
                let printer_store = rupu_orchestrator::RunStore::new(runs_dir.clone());
                let mut printer = crate::output::LineStreamPrinter::new();
                let mut attach_opts = crate::output::workflow_printer::AttachOpts::default();
                let mut current_runner = runner_task;

                loop {
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

                    let result = current_runner
                        .await
                        .map_err(|e| anyhow::anyhow!("workflow task panicked: {e}"))?
                        .map_err(anyhow::Error::from)?;

                    use crate::output::workflow_printer::AttachOutcome;
                    match outcome {
                        AttachOutcome::Done | AttachOutcome::Detached | AttachOutcome::Rejected => {
                            break result;
                        }
                        AttachOutcome::Approved { awaited_step_id } => {
                            let prior_records =
                                run_store_for_resume.read_step_results(rid).map_err(|e| {
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
                                run_id_override: None,
                                strict_templates,
                            };
                            current_runner = tokio::spawn(run_workflow(resume_opts));
                            attach_opts = crate::output::workflow_printer::AttachOpts {
                                skip_header: true,
                                skip_count: prior_count,
                            };
                            let _ = result;
                        }
                    }
                }
            }
        } else {
            runner_task
                .await
                .map_err(|e| anyhow::anyhow!("workflow task panicked: {e}"))?
                .map_err(anyhow::Error::from)?
        }
    } else {
        run_workflow(opts).await?
    };

    if notify_issue_enabled {
        if let (Some(ref_text), Some(payload)) =
            (&issue_ref_text_for_notify, &issue_payload_for_notify)
        {
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
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
    /// Sub-agent dispatcher wired into every step's `ToolContext`.
    /// `None` if the caller didn't construct one (no behavior change
    /// from pre-dispatch builds; `dispatch_agent` calls fail with
    /// "no dispatcher" in that case). The orchestrator constructs
    /// this alongside the factory so it has access to the same
    /// run_store + factory + agent loader.
    dispatcher: Option<Arc<dyn rupu_tools::AgentDispatcher>>,
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
                        dispatchable_agents: None,
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

        // Precompute the parent_run_id clone before moving `run_id`
        // into the struct literal (otherwise the borrow-checker
        // flags it because struct-literal field-init order is the
        // *source* order: `run_id` moves before `tool_context` is
        // constructed).
        let parent_run_id_for_tool_ctx = Some(run_id.clone());

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
                // Sub-agent dispatch wiring. The dispatcher is set on
                // the factory by the workflow runner before
                // `run_workflow` starts; the per-step ToolContext
                // gets the dispatcher Arc plus the agent's declared
                // allowlist + parent run id so the `dispatch_agent`
                // tool can enforce both gates.
                dispatcher: self.dispatcher.clone(),
                dispatchable_agents: spec.dispatchable_agents.clone(),
                parent_run_id: parent_run_id_for_tool_ctx,
                depth: 0,
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
            // Top-level workflow steps run at depth 0 with no parent.
            // Sub-agent dispatch within a step bumps depth via the
            // `dispatch_agent` tool; this struct literal only fires
            // for the workflow → agent direct dispatch.
            parent_run_id: None,
            depth: 0,
            dispatchable_agents: spec.dispatchable_agents,
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
