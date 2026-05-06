//! `rupu run <agent> [prompt]` — one-shot agent run.
//!
//! Wires together: paths → agent loader → config layering → permission
//! resolution → workspace upsert → auth backend → provider factory →
//! `rupu_agent::run_agent`. Prints a one-line summary on success.
//!
// TUI attach for single-agent runs deferred — see slice-c spec §4 v0.1

use crate::paths;
use crate::provider_factory;
use clap::Args as ClapArgs;
use rupu_agent::runner::{AgentRunOpts, BypassDecider, PermissionDecider};
use rupu_agent::{load_agent, parse_mode, resolve_mode, PermissionDecision};
use rupu_tools::{PermissionMode, ToolContext};
use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::Arc;
use ulid::Ulid;

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Agent name (matches an `agents/*.md` file).
    pub agent: String,
    /// Optional target reference, e.g. `github:owner/repo#42`.
    /// See `docs/scm.md#target-syntax` for the full grammar.
    /// Distinguished from `prompt` by parsing as a RunTarget.
    pub target: Option<String>,
    /// Optional initial user message. Defaults to "go" if omitted.
    pub prompt: Option<String>,
    /// Override permission mode (`ask` | `bypass` | `readonly`).
    #[arg(long)]
    pub mode: Option<String>,
    /// Skip token streaming; receive the full response at once.
    #[arg(long)]
    pub no_stream: bool,
}

pub async fn handle(args: Args) -> ExitCode {
    match run_inner(args).await {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("rupu run: {e}");
            ExitCode::from(1)
        }
    }
}

async fn run_inner(args: Args) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;

    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    // Load the agent (project shadows global). `load_agent` takes the
    // parent of `agents/`, so pass `<global>` and `<project_root>`
    // (not `<project_root>/.rupu`) — but the project layout mounts
    // agents under `<project_root>/.rupu/agents/`, so the parent is
    // `<project_root>/.rupu`.
    let project_agents_parent = project_root.as_ref().map(|p| p.join(".rupu"));
    let spec = load_agent(&global, project_agents_parent.as_deref(), &args.agent)?;

    // Resolve config (global + project).
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())?;

    // Resolve permission mode.
    let cli_mode = args.mode.as_deref().and_then(parse_mode);
    let agent_mode = spec.permission_mode.as_deref().and_then(parse_mode);
    // Project-level mode override is rare; v0 reads only the cli/agent/global path.
    let project_mode = None;
    let global_mode = cfg.permission_mode.as_deref().and_then(parse_mode);
    let mode = resolve_mode(cli_mode, agent_mode, project_mode, global_mode);

    // Non-TTY + Ask = abort (spec rule).
    if matches!(mode, PermissionMode::Ask) && !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "non-tty + ask mode: rerun with `--mode bypass` or `--mode readonly`, \
             or run from an interactive terminal"
        );
    }

    // Workspace upsert.
    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, &pwd)?;

    // Provider build via CredentialResolver.
    let resolver = rupu_auth::KeychainResolver::new();

    // Build the SCM/issue registry from the same resolver + config the
    // LLM provider factory uses. Cheap when no platforms are configured;
    // missing credentials are skipped with INFO logs.
    let scm_registry = Arc::new(rupu_scm::Registry::discover(&resolver, &cfg).await);

    let provider_name = spec.provider.clone().unwrap_or_else(|| "anthropic".into());
    let model = spec
        .model
        .clone()
        .or_else(|| cfg.default_model.clone())
        .unwrap_or_else(|| "claude-sonnet-4-6".into());
    let auth_hint = spec.auth;
    let provider_config = provider_factory::ProviderConfig {
        anthropic_oauth_system_prefix: spec.anthropic_oauth_prefix,
    };
    let (resolved_auth, provider) = provider_factory::build_for_provider_with_config(
        &provider_name,
        &model,
        auth_hint,
        &resolver,
        &provider_config,
    )
    .await?;

    println!(
        "agent: {}  provider: {}/{}  model: {}",
        spec.name, provider_name, resolved_auth, model,
    );

    // Transcript path.
    let run_id = format!("run_{}", Ulid::new());
    let transcripts = paths::transcripts_dir(&global, project_root.as_deref());
    paths::ensure_dir(&transcripts)?;
    let transcript_path = transcripts.join(format!("{run_id}.jsonl"));

    // Tool context config (the path is filled in after target resolution below).
    let bash_timeout = cfg.bash.timeout_secs.unwrap_or(120);
    let bash_allowlist = cfg.bash.env_allowlist.clone().unwrap_or_default();

    // Disambiguate: if `args.target` parses as a RunTarget, it's a target.
    // Otherwise treat it (plus the remainder) as part of the user prompt.
    let (run_target, user_message) = match args.target.as_deref() {
        None => (None, args.prompt.clone().unwrap_or_else(|| "go".into())),
        Some(s) => match crate::run_target::parse_run_target(s) {
            Ok(t) => (Some(t), args.prompt.clone().unwrap_or_else(|| "go".into())),
            Err(_) => {
                // Not a target → it's the leading word(s) of the prompt.
                let combined = match args.prompt.as_deref() {
                    Some(p) => format!("{s} {p}"),
                    None => s.to_string(),
                };
                (None, combined)
            }
        },
    };

    // Preload `## Run target` into the agent system prompt when a target is set.
    let agent_system_prompt = match run_target.as_ref() {
        Some(t) => format!(
            "{}\n\n## Run target\n\n{}",
            spec.system_prompt,
            crate::run_target::format_run_target_for_prompt(t),
        ),
        None => spec.system_prompt.clone(),
    };

    // Clone the target repo into a tmpdir for Repo/Pr targets unless the
    // cwd is already that checkout. Issue targets don't need a clone.
    // _clone_guard is bound at function scope so its Drop runs on return,
    // keeping the directory alive for the duration of the run.
    let _clone_guard: Option<tempfile::TempDir>;
    let workspace_path: std::path::PathBuf = match run_target.as_ref() {
        Some(crate::run_target::RunTarget::Repo {
            platform,
            owner,
            repo,
            ..
        })
        | Some(crate::run_target::RunTarget::Pr {
            platform,
            owner,
            repo,
            ..
        }) => {
            let r = rupu_scm::RepoRef {
                platform: *platform,
                owner: owner.clone(),
                repo: repo.clone(),
            };
            let conn = scm_registry.repo(*platform).ok_or_else(|| {
                anyhow::anyhow!(
                    "no {} credential — run `rupu auth login --provider {}`",
                    platform,
                    platform
                )
            })?;
            let tmp = tempfile::tempdir()?;
            conn.clone_to(&r, tmp.path()).await?;
            let path = tmp.path().to_path_buf();
            _clone_guard = Some(tmp);
            path
        }
        _ => {
            _clone_guard = None;
            pwd.clone()
        }
    };

    // Build tool context now that the resolved workspace_path is known.
    let tool_context = ToolContext {
        workspace_path: workspace_path.clone(),
        bash_env_allowlist: bash_allowlist,
        bash_timeout_secs: bash_timeout,
    };

    let mode_str = match mode {
        PermissionMode::Ask => "ask",
        PermissionMode::Bypass => "bypass",
        PermissionMode::Readonly => "readonly",
    };

    let decider: Arc<dyn PermissionDecider> = pick_decider(mode);

    let opts = AgentRunOpts {
        agent_name: spec.name.clone(),
        agent_system_prompt,
        agent_tools: spec.tools.clone(),
        provider,
        provider_name,
        model,
        run_id: run_id.clone(),
        workspace_id: ws.id.clone(),
        workspace_path: workspace_path.clone(),
        transcript_path: transcript_path.clone(),
        max_turns: spec.max_turns.unwrap_or(50),
        decider,
        tool_context,
        user_message,
        mode_str: mode_str.to_string(),
        no_stream: args.no_stream,
        // Single-agent `rupu run` keeps the legacy line-stream UI
        // for v0; the TUI attach for this command was deferred to
        // a future slice. Workflow runs flip this to true.
        suppress_stream_stdout: false,
        mcp_registry: Some(scm_registry),
        effort: spec.effort,
        context_window: spec.context_window,
        output_format: spec.output_format,
        anthropic_task_budget: spec.anthropic_task_budget,
        anthropic_context_management: spec.anthropic_context_management,
        anthropic_speed: spec.anthropic_speed,
    };

    let result = rupu_agent::run_agent(opts).await?;
    println!(
        "Total: {} input / {} output tokens",
        result.total_tokens_in, result.total_tokens_out
    );
    println!(
        "rupu: run {} complete in {} turn(s); transcript: {}",
        run_id,
        result.turns,
        transcript_path.display()
    );
    Ok(())
}

fn pick_decider(mode: PermissionMode) -> Arc<dyn PermissionDecider> {
    match mode {
        PermissionMode::Bypass => Arc::new(BypassDecider),
        PermissionMode::Readonly => Arc::new(ReadonlyDecider),
        PermissionMode::Ask => Arc::new(AskDecider),
    }
}

/// Readonly: deny writers (bash/write_file/edit_file), allow readers.
struct ReadonlyDecider;
impl PermissionDecider for ReadonlyDecider {
    fn decide(
        &self,
        _mode: PermissionMode,
        tool: &str,
        _input: &serde_json::Value,
        _workspace: &str,
    ) -> Result<PermissionDecision, rupu_agent::runner::RunError> {
        match tool {
            "bash" | "write_file" | "edit_file" => Ok(PermissionDecision::Deny),
            _ => Ok(PermissionDecision::Allow),
        }
    }
}

/// Ask: stdin-driven prompt for writers; readers always allowed.
///
/// Prompts via [`rupu_agent::PermissionPrompt::for_stdio`], which writes
/// to stderr and reads from stdin. We re-take the stderr lock for each
/// decision so back-to-back prompts don't deadlock.
struct AskDecider;
impl PermissionDecider for AskDecider {
    fn decide(
        &self,
        _mode: PermissionMode,
        tool: &str,
        input: &serde_json::Value,
        workspace: &str,
    ) -> Result<PermissionDecision, rupu_agent::runner::RunError> {
        if !matches!(tool, "bash" | "write_file" | "edit_file") {
            return Ok(PermissionDecision::Allow);
        }
        let mut stderr = std::io::stderr();
        let mut prompt = rupu_agent::PermissionPrompt::for_stdio(&mut stderr);
        prompt
            .ask(tool, input, workspace)
            .map_err(|e| rupu_agent::runner::RunError::Provider(format!("ask prompt io: {e}")))
    }
}
