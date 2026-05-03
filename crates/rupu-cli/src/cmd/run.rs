//! `rupu run <agent> [prompt]` — one-shot agent run.
//!
//! Wires together: paths → agent loader → config layering → permission
//! resolution → workspace upsert → auth backend → provider factory →
//! `rupu_agent::run_agent`. Prints a one-line summary on success.

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
    let provider_name = spec.provider.clone().unwrap_or_else(|| "anthropic".into());
    let model = spec
        .model
        .clone()
        .or_else(|| cfg.default_model.clone())
        .unwrap_or_else(|| "claude-sonnet-4-6".into());
    let auth_hint = spec.auth;
    let (resolved_auth, provider) =
        provider_factory::build_for_provider(&provider_name, &model, auth_hint, &resolver).await?;

    println!(
        "agent: {}  provider: {}/{}  model: {}",
        spec.name, provider_name, resolved_auth, model,
    );

    // Transcript path.
    let run_id = format!("run_{}", Ulid::new());
    let transcripts = paths::transcripts_dir(&global, project_root.as_deref());
    paths::ensure_dir(&transcripts)?;
    let transcript_path = transcripts.join(format!("{run_id}.jsonl"));

    // Tool context.
    let bash_timeout = cfg.bash.timeout_secs.unwrap_or(120);
    let bash_allowlist = cfg.bash.env_allowlist.clone().unwrap_or_default();
    let tool_context = ToolContext {
        workspace_path: pwd.clone(),
        bash_env_allowlist: bash_allowlist,
        bash_timeout_secs: bash_timeout,
    };

    let user_message = args.prompt.unwrap_or_else(|| "go".to_string());
    let mode_str = match mode {
        PermissionMode::Ask => "ask",
        PermissionMode::Bypass => "bypass",
        PermissionMode::Readonly => "readonly",
    };

    let decider: Arc<dyn PermissionDecider> = pick_decider(mode);

    let opts = AgentRunOpts {
        agent_name: spec.name.clone(),
        agent_system_prompt: spec.system_prompt.clone(),
        agent_tools: spec.tools.clone(),
        provider,
        provider_name,
        model,
        run_id: run_id.clone(),
        workspace_id: ws.id.clone(),
        workspace_path: pwd.clone(),
        transcript_path: transcript_path.clone(),
        max_turns: spec.max_turns.unwrap_or(50),
        decider,
        tool_context,
        user_message,
        mode_str: mode_str.to_string(),
        no_stream: args.no_stream,
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
