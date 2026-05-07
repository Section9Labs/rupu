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
        Err(e) => crate::output::diag::fail(e)
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
    let (_resolved_auth, provider) = provider_factory::build_for_provider_with_config(
        &provider_name,
        &model,
        auth_hint,
        &resolver,
        &provider_config,
    )
    .await?;

    // Print the agent header via the line-stream printer.
    // The run_id isn't known yet; we'll use a placeholder.
    let agent_header_name = spec.name.clone();
    let agent_header_provider = provider_name.clone();
    let agent_header_model = model.clone();
    // (printed after run_id is set below)

    // Transcript path.
    let run_id = format!("run_{}", Ulid::new());
    let transcripts = paths::transcripts_dir(&global, project_root.as_deref());
    paths::ensure_dir(&transcripts)?;
    let transcript_path = transcripts.join(format!("{run_id}.jsonl"));

    // Print the agent header now that the run_id is known.
    {
        let mut printer = crate::output::LineStreamPrinter::new();
        printer.agent_header(
            &agent_header_name,
            &agent_header_provider,
            &agent_header_model,
            &run_id,
        );
    }

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
        // Suppress the agent runner's inline stdout writes; the CLI's
        // line-stream printer reads tokens from the JSONL transcript
        // instead. This prevents duplicate output when the printer is
        // active and ensures clean output when stdout is piped.
        suppress_stream_stdout: true,
        mcp_registry: Some(scm_registry),
        effort: spec.effort,
        context_window: spec.context_window,
        output_format: spec.output_format,
        anthropic_task_budget: spec.anthropic_task_budget,
        anthropic_context_management: spec.anthropic_context_management,
        anthropic_speed: spec.anthropic_speed,
    };

    // Spawn the agent in a background task and tail the transcript with
    // the line-stream printer while it runs.
    let transcript_path_for_printer = transcript_path.clone();
    let run_id_for_printer = run_id.clone();
    let spec_name_for_printer = spec.name.clone();

    // Run the agent. The printer reads from the JSONL transcript file;
    // since the agent is async and the printer is sync, we run the
    // printer in a background thread that polls the transcript while
    // the tokio task drives the agent.
    let agent_task = tokio::spawn(rupu_agent::run_agent(opts));

    // Tail the transcript in this thread.
    {
        let mut printer = crate::output::LineStreamPrinter::new();
        printer.step_start(&spec_name_for_printer, None, None, None);
        let mut tailer = crate::output::TranscriptTailer::new(&transcript_path_for_printer);
        let mut total_tokens = 0u64;

        loop {
            let events = tailer.drain();
            for ev in events {
                match &ev {
                    rupu_transcript::Event::AssistantMessage { content, .. }
                        if !content.trim().is_empty() =>
                    {
                        printer.assistant_chunk(content);
                    }
                    rupu_transcript::Event::ToolCall { tool, input, .. } => {
                        let summary = crate::output::workflow_printer::tool_summary(tool, input);
                        printer.tool_call(tool, &summary);
                    }
                    rupu_transcript::Event::RunComplete {
                        status,
                        total_tokens: tokens,
                        duration_ms,
                        error,
                        ..
                    } => {
                        total_tokens = *tokens;
                        let dur = std::time::Duration::from_millis(*duration_ms);
                        match status {
                            rupu_transcript::RunStatus::Ok => {
                                printer.step_done(&run_id_for_printer, dur, *tokens);
                            }
                            _ => {
                                let reason = error.as_deref().unwrap_or("unknown");
                                printer.step_failed(&run_id_for_printer, reason);
                            }
                        }
                    }
                    _ => {}
                }
                // Once we see RunComplete, we can stop tailing.
                if matches!(ev, rupu_transcript::Event::RunComplete { .. }) {
                    break;
                }
            }

            // Check if the agent task has finished.
            if agent_task.is_finished() {
                // Drain any remaining events.
                let tail_events = tailer.drain();
                for ev in tail_events {
                    if let rupu_transcript::Event::AssistantMessage { content, .. } = &ev {
                        if !content.trim().is_empty() {
                            printer.assistant_chunk(content);
                        }
                    }
                }
                if total_tokens == 0 {
                    // RunComplete wasn't seen yet; print a plain done.
                    printer.step_done(&run_id_for_printer, std::time::Duration::ZERO, 0);
                }
                break;
            }

            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    // Await the agent task to propagate any error.
    let result = agent_task
        .await
        .map_err(|e| anyhow::anyhow!("agent task panicked: {e}"))??;

    // Print a brief footer.
    println!("transcript: {}", transcript_path.display());
    let _ = result;
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
