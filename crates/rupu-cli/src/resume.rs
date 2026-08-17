//! Workflow-resume primitive shared by `rupu workflow approve` and the
//! background session worker.
//!
//! [`resume_run`] performs phase 2 of an approval: it reloads a run that
//! the store has already flipped to `Running` (phase 1 —
//! `RunStore::approve`), rebuilds the orchestrator runtime from the
//! persisted workflow snapshot + prior step results, and re-enters
//! [`run_workflow`]. It is self-contained — it re-derives the global dir
//! and the run store the same way the CLI does — so a worker with no CLI
//! handler scope can call it identically to the `approve` subcommand.

use crate::paths;
use rupu_mcp::{McpPermission, ToolDispatcher};
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts, OrchestratorRunResult};
use rupu_orchestrator::{DefaultStepFactory, RunStore, Workflow};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Build the `action_dispatcher` every `OrchestratorRunOpts` construction
/// site wires: an in-process MCP `ToolDispatcher` over the same SCM
/// `Registry` and the run's permission mode.
///
/// **Mode** genuinely matches the agent path — `parse_mode_for_runtime`
/// (rupu-agent) is reused rather than duplicated, so it is the same mode
/// string → `PermissionMode` mapping `run_agent` applies to its own tool
/// registry, and a `readonly` run refuses Write-classified tools here too.
///
/// The **tool allowlist** deliberately does NOT match the agent path, and
/// this is not an oversight (ISSUES.md I-26). An agent step's surface is
/// its `tools:` list narrowed by `actions:` (`step_factory.rs`'s
/// `narrow_agent_tools`); this dispatcher passes `["*"]` instead, because
/// it is built once per run while the tool it may call is per-step. That
/// is sound rather than permissive, resting on three invariants:
///
/// 1. Every consumer of `opts.action_dispatcher` funnels into
///    `execute_action_step`, whose only dispatch is `dispatcher.call(tool,
///    …)` with `tool = step.action`. Agent-step tool calls never reach
///    this dispatcher.
/// 2. That tool is validated against the live MCP catalog at parse time by
///    `validate_action_step` (`rupu-orchestrator`'s `workflow.rs`).
/// 3. A step may not carry a non-empty `actions:` alongside `action:` —
///    `WorkflowParseError::ActionsOnActionStep` rejects it, since an
///    action step's tool is already explicit. So there is no per-step
///    allowlist here to honor in the first place.
///
/// In short: the only tool this dispatcher can ever be asked for is one
/// named explicitly in the workflow source and already catalog-checked.
/// Narrowing the allowlist to that single tool anyway — so the guarantee
/// is structural rather than an invariant enforced three modules away —
/// is tracked as **I-79**.
pub fn action_dispatcher_for(
    registry: &Arc<rupu_scm::Registry>,
    mode_str: &str,
) -> Arc<ToolDispatcher> {
    Arc::new(ToolDispatcher::new(
        Arc::clone(registry),
        McpPermission::new(
            rupu_agent::runner::parse_mode_for_runtime(mode_str),
            vec!["*".into()],
        ),
    ))
}

/// Result of a successful [`resume_run`], carrying everything the caller
/// needs to render the post-resume status (re-pause vs completion) without
/// re-reading the store.
pub struct ResumeOutcome {
    /// The awaited step the resume dispatched from. Used in the
    /// "resumed run … from step `…`" line.
    pub awaited_step_id: String,
    /// The full orchestrator result: `run_id`, `step_results`, and the
    /// optional `awaiting` re-pause info.
    pub result: OrchestratorRunResult,
}

/// Resume an already-approved run (phase 2 of approval).
///
/// `store.approve(run_id, ...)` must have already recorded the decision
/// and flipped the run to `Running`; this reloads the record, rebuilds the
/// runtime from disk (workflow snapshot + prior step results +
/// `KeychainResolver` + layered config + SCM registry + dispatcher +
/// `DefaultStepFactory`), and re-enters `run_workflow`.
///
/// `awaited_step_id` is the step the approval acted on (the `step_id`
/// returned by `RunStore::approve`). It must be threaded in from phase 1
/// because `approve` clears `awaiting_step_id` on the persisted record, so
/// it is no longer recoverable from the reloaded record.
///
/// The `store` reference is used for the disk reads; the runtime store
/// `Arc` is rebuilt internally from the global dir (identical to the CLI's
/// inline path), so this is safe to call from a context that holds only a
/// borrow.
///
/// `mode` overrides the permission mode for the resumed run (`ask` /
/// `bypass` / `readonly`). `None` falls back through
/// `record.resume_mode` then `record.permission_mode` (the run's own
/// launch mode, ISSUES.md I-24) before defaulting to `ask` — see
/// [`rebuild_opts_from_disk`]'s precedence.
///
/// `approver`/`via_timeout` (I-36/I-38) are the gate decision provenance
/// the CLI's `resolve_approve_gate` already resolved before calling this —
/// threaded into [`rupu_orchestrator::ResumeState::from_approval_with_actor`]
/// so the resumed run's gate-suppression path records the real actor and
/// whether this was a genuine operator decision (`via: "human"`) or a
/// `cp serve` sweep-driven `on_timeout: approve` (`via: "timeout"`).
pub async fn resume_run(
    store: &RunStore,
    run_id: &str,
    awaited_step_id: &str,
    mode: Option<&str>,
    approver: &str,
    via_timeout: bool,
) -> anyhow::Result<ResumeOutcome> {
    let awaited_step_id = awaited_step_id.to_string();
    let (mut opts, prior_step_results) = rebuild_opts_from_disk(store, run_id, mode).await?;
    opts.resume_from = Some(rupu_orchestrator::ResumeState::from_approval_with_actor(
        run_id.to_string(),
        prior_step_results,
        awaited_step_id.clone(),
        approver.to_string(),
        via_timeout,
    ));

    let result = run_workflow(opts).await?;
    Ok(ResumeOutcome {
        awaited_step_id,
        result,
    })
}

/// Rebuild `OrchestratorRunOpts` for a run's `on_reject` cleanup chain
/// (`rupu workflow reject`, and Plan 4's cp-serve reject worker), called
/// AFTER `RunStore::reject` has already finalized the run as `Rejected`.
///
/// Shares [`rebuild_opts_from_disk`] with [`resume_run`] — same disk state,
/// same wiring — but sets `resume_from` to
/// [`rupu_orchestrator::ResumeState::from_rejection`] instead of
/// `from_approval`, and never re-enters `run_workflow`: the caller passes
/// the returned opts straight to
/// [`rupu_orchestrator::runner::run_reject_cleanup`].
///
/// Returns the opts alongside the rejected gate's `on_reject` chain length
/// (0 for a legacy inline-approval step, an unknown step id, or an empty
/// chain) so the caller can print `cleanup: <n> step(s) executed` without
/// re-deriving it after `opts.workflow` has moved into the cleanup call.
pub async fn build_reject_cleanup_opts(
    store: &RunStore,
    run_id: &str,
    rejected_step_id: &str,
    reason: &str,
    mode: Option<&str>,
) -> anyhow::Result<(OrchestratorRunOpts, usize)> {
    let (mut opts, prior_step_results) = rebuild_opts_from_disk(store, run_id, mode).await?;
    let chain_len = opts
        .workflow
        .steps
        .iter()
        .find(|s| s.id == rejected_step_id)
        .and_then(|s| s.approval.as_ref())
        .map(|a| a.on_reject.len())
        .unwrap_or(0);
    opts.resume_from = Some(rupu_orchestrator::ResumeState::from_rejection(
        run_id.to_string(),
        prior_step_results,
        rejected_step_id.to_string(),
        reason.to_string(),
    ));
    Ok((opts, chain_len))
}

/// Shared disk-rebuild step for [`resume_run`] (approve-resume) and
/// [`build_reject_cleanup_opts`] (reject-cleanup): reload the persisted
/// workflow snapshot + prior step results and reconstruct the full
/// `OrchestratorRunOpts` wiring (resolver, layered config, SCM registry,
/// dispatcher, `DefaultStepFactory`, event sink) exactly as the original
/// run used. Returns the opts with `resume_from: None` — callers set it
/// afterward to the resume shape they need.
async fn rebuild_opts_from_disk(
    store: &RunStore,
    run_id: &str,
    mode: Option<&str>,
) -> anyhow::Result<(OrchestratorRunOpts, Vec<rupu_orchestrator::StepResult>)> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let runs_dir = global.join("runs");
    let store_arc = Arc::new(rupu_orchestrator::RunStore::new(runs_dir));

    // Reload the record from disk to get inputs, event, workspace path
    // for the run_workflow re-entry. The library call already persisted
    // the status flip (Running for approve, Rejected for reject), so the
    // record is coherent.
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
    let cfg = rupu_config::layer_files_locked(Some(&global_cfg_path), project_cfg_path.as_deref())?;
    // TODO(netflow task 7): pass the run's sink
    let mcp_registry = Arc::new(
        rupu_scm::Registry::discover(resolver.as_ref(), &cfg, Arc::new(rupu_netflow::NullSink))
            .await,
    );

    // ISSUES.md I-24: precedence, most specific first — an explicit
    // `--mode` on the calling command (if one exists; `reject` has none),
    // then `record.resume_mode` (set by the web-resume path), then
    // `record.permission_mode` (the run's own launch mode, persisted at
    // creation — see `run_workflow`'s fresh-record write), then `"ask"`.
    // Before this fell straight to `mode.unwrap_or("ask")`: a run launched
    // `--mode readonly` had no persisted trace of that mode anywhere but
    // `resume_mode` (which only the web-resume path ever sets), so its
    // `on_reject` cleanup silently ran under `ask` — permitting Write
    // tools a `readonly` launch had denied.
    let mode_str = mode
        .map(str::to_string)
        .or_else(|| record.resume_mode.clone())
        .or_else(|| record.permission_mode.clone())
        .unwrap_or_else(|| "ask".to_string());

    // Hoisted above the dispatcher build so `CliAgentDispatcher` can be
    // handed a clone of the same sink and emit `DispatchStarted` /
    // `DispatchCompleted` into the same `events.jsonl` the resumed run's
    // opts carry.
    let event_sink_for_resume = {
        let runs_dir = global.join("runs");
        let events_path = runs_dir.join(run_id).join("events.jsonl");
        match rupu_orchestrator::executor::JsonlSink::create(&events_path) {
            Ok(sink) => Some(Arc::new(sink) as Arc<dyn rupu_orchestrator::executor::EventSink>),
            Err(e) => {
                tracing::warn!(error = %e, "failed to open events.jsonl for resume; continuing without event sink");
                None
            }
        }
    };

    // Hoisted above the dispatcher build: sub-agent dispatch resolves its
    // provider/model through the same config-derived defaults the step
    // factory below uses (ISSUES.md I-8).
    let openai_compatible = rupu_runtime::provider_factory::openai_compatible_map(&cfg.providers);
    let provider_tuning = rupu_runtime::provider_factory::provider_tuning_map(&cfg.providers);
    let dispatcher = crate::cmd::dispatch::CliAgentDispatcher::new(
        global.clone(),
        project_root.clone(),
        record.workspace_id.clone(),
        workspace_path.clone(),
        Arc::clone(&resolver),
        mode_str.clone(),
        Arc::clone(&mcp_registry),
        Arc::clone(&store_arc),
        event_sink_for_resume.clone(),
        cfg.default_provider.clone(),
        cfg.default_model.clone(),
        openai_compatible.clone(),
        provider_tuning.clone(),
    );
    let dispatcher_dyn: Arc<dyn rupu_tools::AgentDispatcher> = dispatcher;
    let action_dispatcher = action_dispatcher_for(&mcp_registry, &mode_str);
    let factory = Arc::new(DefaultStepFactory {
        workflow: workflow.clone(),
        global: global.clone(),
        project_root: project_root.clone(),
        resolver,
        mode_str,
        mcp_registry,
        system_prompt_suffix: None,
        dispatcher: Some(dispatcher_dyn),
        openai_compatible,
        provider_tuning,
        default_provider: cfg.default_provider.clone(),
        default_model: cfg.default_model.clone(),
        bash_timeout_secs: cfg.bash.timeout_secs.unwrap_or(120),
        bash_env_allowlist: cfg.bash.env_allowlist.clone().unwrap_or_default(),
    });

    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow,
        inputs: inputs_map,
        workspace_id: record.workspace_id.clone(),
        workspace_path,
        transcript_dir: transcripts,
        factory,
        event,
        issue: issue_payload,
        issue_ref: issue_ref_text,
        run_store: Some(store_arc),
        workflow_yaml: Some(body),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: event_sink_for_resume,
        unit_dispatcher: None,
        action_dispatcher: Some(action_dispatcher),
        pause: None,
    };

    Ok((opts, prior_step_results))
}
