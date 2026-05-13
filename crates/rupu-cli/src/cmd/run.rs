//! `rupu run <agent> [prompt]` — one-shot agent run.
//!
//! Wires together: paths → agent loader → config layering → permission
//! resolution → workspace upsert → auth backend → provider factory →
//! `rupu_agent::run_agent`. Prints a one-line summary on success.
//!
// TUI attach for single-agent runs deferred — see slice-c spec §4 v0.1

use crate::paths;
use crate::standalone_run_metadata::{
    metadata_path_for_run, write_metadata, StandaloneRunMetadata,
};
use clap::Args as ClapArgs;
use rupu_agent::runner::{AgentRunOpts, BypassDecider, PermissionDecider};
use rupu_agent::{load_agent, parse_mode, resolve_mode, PermissionDecision};
use rupu_runtime::provider_factory;
use rupu_runtime::WorkerKind;
use rupu_tools::{PermissionMode, ToolContext};
use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use tracing::warn;
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
    /// For `<platform>:<owner>/<repo>` targets: clone into this
    /// directory instead of the default `./<repo>/`. The directory
    /// must not already exist (refuse-by-default — pass an explicit
    /// path you own). Mutually exclusive with `--tmp`.
    #[arg(long, value_name = "PATH", conflicts_with = "tmp")]
    pub into: Option<std::path::PathBuf>,
    /// For `<platform>:<owner>/<repo>` targets: clone into a
    /// temporary directory that is auto-deleted on exit. Useful for
    /// one-shot agents that produce findings without modifying the
    /// repo. Mutually exclusive with `--into`.
    #[arg(long, conflicts_with = "into")]
    pub tmp: bool,
}

pub async fn handle(args: Args) -> ExitCode {
    match run_inner(args).await {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
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
    if let Err(err) = crate::cmd::repos::auto_track_checkout(&global, &pwd) {
        warn!(path = %pwd.display(), error = %err, "failed to auto-track checkout");
    }

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

    // Construct ONE LineStreamPrinter for the whole run. Keeping a
    // single instance means a single MultiProgress + ticker — earlier
    // we built one for the header and another for the tail loop, and
    // their indicatif draw targets stomped on each other (visible as
    // two stale spinner rows under heavy tool-call traffic).
    let mut printer = crate::output::LineStreamPrinter::new();
    printer.agent_header(
        &agent_header_name,
        &agent_header_provider,
        &agent_header_model,
        &run_id,
    );

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

    // Clone the target repo for Repo/Pr targets. Three destination
    // modes, in priority order:
    //   1. `--tmp`           → tempfile::TempDir, auto-deleted on exit
    //   2. `--into <path>`   → that path, persistent. Refuse if it exists.
    //   3. (no flag)         → `./<repo>/` in cwd, persistent. Refuse if it exists.
    //
    // Refuse-by-default on existing paths to protect uncommitted work
    // and prevent surprising clobbers. The error message points at the
    // available escape hatches.
    //
    // _clone_guard holds the TempDir handle in mode 1 so Drop runs on
    // function exit, keeping the directory alive for the run. Modes 2
    // and 3 set it to None — the user owns cleanup.
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

            let (dest, guard) = resolve_clone_dest(&pwd, repo, args.into.as_deref(), args.tmp)?;
            // Brief progress line on stderr so the user knows where the
            // clone is landing — the LineStreamPrinter rail has already
            // printed `▶ <agent>` on stdout and we don't want to break
            // its visual flow with a clone-progress line in the middle.
            eprintln!("  cloning {}/{} → {}", owner, repo, dest.display());
            conn.clone_to(&r, &dest).await?;
            _clone_guard = guard;
            dest
        }
        _ => {
            _clone_guard = None;
            pwd.clone()
        }
    };

    // Build tool context now that the resolved workspace_path is known.
    // No dispatcher is wired for bare `rupu run`; sub-agent dispatch
    // is a workflow-runner-only feature today (orchestrator wires the
    // dispatcher per-step). A `dispatch_agent` tool call from a bare
    // `rupu run` therefore returns a clear error.
    let tool_context = ToolContext {
        workspace_path: workspace_path.clone(),
        bash_env_allowlist: bash_allowlist,
        bash_timeout_secs: bash_timeout,
        dispatcher: None,
        dispatchable_agents: spec.dispatchable_agents.clone(),
        parent_run_id: None,
        depth: 0,
    };

    let mode_str = match mode {
        PermissionMode::Ask => "ask",
        PermissionMode::Bypass => "bypass",
        PermissionMode::Readonly => "readonly",
    };

    let backend_id = "local_checkout".to_string();
    let repo_ref = standalone_repo_ref(run_target.as_ref(), &workspace_path);
    let issue_ref = standalone_issue_ref(run_target.as_ref());
    let workspace_strategy =
        standalone_workspace_strategy(run_target.as_ref(), &workspace_path, args.tmp);
    let worker_ctx = crate::cmd::workflow::default_execution_worker_context(WorkerKind::Cli, None);
    let worker_record = crate::cmd::workflow::upsert_worker_record(
        &global,
        &worker_ctx,
        &backend_id,
        mode_str,
        repo_ref.as_deref(),
    )?;
    let metadata = StandaloneRunMetadata {
        version: StandaloneRunMetadata::VERSION,
        run_id: run_id.clone(),
        session_id: None,
        archived_at: None,
        workspace_path: canonicalize_if_exists(&workspace_path),
        project_root: project_root.clone(),
        repo_ref,
        issue_ref,
        backend_id,
        worker_id: Some(worker_record.worker_id.clone()),
        trigger_source: "run_cli".into(),
        target: if run_target.is_some() {
            args.target.clone()
        } else {
            None
        },
        workspace_strategy,
    };
    write_metadata(&metadata_path_for_run(&transcripts, &run_id), &metadata)?;

    let decider: Arc<dyn PermissionDecider> = pick_decider(mode, Some(printer.multi_handle()));

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
        initial_messages: Vec::new(),
        turn_index_offset: 0,
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
        // Top-level `rupu run` invocation — no parent, depth 0,
        // dispatch surface taken from the agent's frontmatter.
        parent_run_id: None,
        depth: 0,
        dispatchable_agents: spec.dispatchable_agents.clone(),
        step_id: String::new(),
        on_tool_call: None,
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

    // Tail the transcript in this thread, reusing the printer from
    // the agent_header above so we don't construct a second
    // MultiProgress that would double-render the bottom-row ticker.
    {
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

pub(crate) fn canonicalize_if_exists(path: &Path) -> std::path::PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn standalone_repo_ref(
    run_target: Option<&crate::run_target::RunTarget>,
    workspace_path: &Path,
) -> Option<String> {
    match run_target {
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
        }) => Some(format!("{platform}:{owner}/{repo}")),
        Some(crate::run_target::RunTarget::Issue {
            tracker, project, ..
        }) => Some(format!("{tracker}:{project}")),
        None => crate::cmd::issues::autodetect_repo_from_path(workspace_path)
            .ok()
            .map(|repo| crate::cmd::issues::canonical_repo_ref(&repo)),
    }
}

pub(crate) fn standalone_issue_ref(
    run_target: Option<&crate::run_target::RunTarget>,
) -> Option<String> {
    match run_target {
        Some(crate::run_target::RunTarget::Issue {
            tracker,
            project,
            number,
        }) => Some(format!("{tracker}:{project}/issues/{number}")),
        _ => None,
    }
}

pub(crate) fn standalone_workspace_strategy(
    run_target: Option<&crate::run_target::RunTarget>,
    workspace_path: &Path,
    tmp: bool,
) -> Option<String> {
    let value = match run_target {
        Some(crate::run_target::RunTarget::Repo { .. })
        | Some(crate::run_target::RunTarget::Pr { .. })
            if tmp =>
        {
            "temporary_clone"
        }
        Some(crate::run_target::RunTarget::Repo { .. })
        | Some(crate::run_target::RunTarget::Pr { .. }) => "direct_clone",
        _ if crate::cmd::issues::autodetect_repo_from_path(workspace_path).is_ok() => {
            "direct_checkout"
        }
        _ => "direct_workspace",
    };
    Some(value.into())
}

pub(crate) fn pick_decider(
    mode: PermissionMode,
    multi: Option<indicatif::MultiProgress>,
) -> Arc<dyn PermissionDecider> {
    match mode {
        PermissionMode::Bypass => Arc::new(BypassDecider),
        PermissionMode::Readonly => Arc::new(ReadonlyDecider),
        PermissionMode::Ask => Arc::new(AskDecider { multi }),
    }
}

/// Resolve where to clone a `<platform>:<owner>/<repo>` target. Returns
/// `(destination_path, optional_tempdir_guard)`. The guard, when
/// `Some`, must be held alive for the duration of the run so its Drop
/// (which deletes the directory) doesn't fire early.
///
/// Three modes, in priority order:
///   1. `tmp == true`              → fresh `tempfile::TempDir`
///   2. `into = Some(p)`           → `p`, persistent. Must not exist.
///   3. (default)                  → `cwd / <repo>`, persistent. Must not exist.
///
/// Modes 2 and 3 refuse-by-default on existing paths to protect
/// uncommitted work; the error message points at `--into`, `--tmp`,
/// or removing the existing directory as escape hatches.
pub(crate) fn resolve_clone_dest(
    cwd: &std::path::Path,
    repo: &str,
    into: Option<&std::path::Path>,
    tmp: bool,
) -> anyhow::Result<(std::path::PathBuf, Option<tempfile::TempDir>)> {
    if tmp {
        let td = tempfile::tempdir()?;
        let path = td.path().to_path_buf();
        return Ok((path, Some(td)));
    }
    let dest = match into {
        Some(p) => p.to_path_buf(),
        None => cwd.join(repo),
    };
    if dest.exists() {
        anyhow::bail!(
            "{} already exists; pass `--into <dir>` to clone elsewhere, \
             `--tmp` for a throwaway clone, or remove the directory first",
            dest.display()
        );
    }
    Ok((dest, None))
}

/// Readonly: deny writers (bash/write_file/edit_file), allow readers.
pub(crate) struct ReadonlyDecider;
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
struct AskDecider {
    /// Clone of the printer's `MultiProgress`. When `Some`, the
    /// permission prompt suspends the spinner via
    /// `MultiProgress::suspend` so the spinner's `\r`-based redraw
    /// on stdout doesn't clobber the prompt the agent runtime just
    /// wrote to stderr. (`\r` is a cursor-level operation; it moves
    /// the same physical cursor that stderr writes are using.)
    /// `None` = no spinner active (non-TTY / test) — prompt directly.
    multi: Option<indicatif::MultiProgress>,
}
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
        let do_prompt = || -> Result<PermissionDecision, rupu_agent::runner::RunError> {
            let mut stderr = std::io::stderr();
            let mut prompt = rupu_agent::PermissionPrompt::for_stdio(&mut stderr);
            prompt
                .ask(tool, input, workspace)
                .map_err(|e| rupu_agent::runner::RunError::Provider(format!("ask prompt io: {e}")))
        };
        match &self.multi {
            Some(m) => m.suspend(do_prompt),
            None => do_prompt(),
        }
    }
}

#[cfg(test)]
mod resolve_clone_dest_tests {
    use super::resolve_clone_dest;
    use std::path::Path;

    #[test]
    fn tmp_returns_a_guarded_tmpdir() {
        let cwd = Path::new("/tmp");
        let (dest, guard) = resolve_clone_dest(cwd, "rupu", None, true).unwrap();
        assert!(
            guard.is_some(),
            "tmp mode should hand back the TempDir guard"
        );
        assert!(dest.exists(), "TempDir should already exist on disk");
    }

    #[test]
    fn default_uses_cwd_repo_and_refuses_when_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let (dest, guard) = resolve_clone_dest(tmp.path(), "myrepo", None, false).unwrap();
        assert_eq!(dest, tmp.path().join("myrepo"));
        assert!(
            guard.is_none(),
            "default mode does not hold a TempDir guard"
        );
        std::fs::create_dir(&dest).unwrap();
        let err = resolve_clone_dest(tmp.path(), "myrepo", None, false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("already exists"), "got: {msg}");
        assert!(msg.contains("--into"), "error should hint at --into: {msg}");
        assert!(msg.contains("--tmp"), "error should hint at --tmp: {msg}");
    }

    #[test]
    fn into_uses_explicit_path_and_refuses_when_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("custom-name");
        let (dest, guard) = resolve_clone_dest(tmp.path(), "rupu", Some(&target), false).unwrap();
        assert_eq!(dest, target);
        assert!(guard.is_none());
        std::fs::create_dir(&target).unwrap();
        assert!(resolve_clone_dest(tmp.path(), "rupu", Some(&target), false).is_err());
    }
}
