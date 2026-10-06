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
use std::path::{Path, PathBuf};
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
///    `execute_action_step`, whose only dispatch is
///    `dispatcher.call_with_findings_profile(tool, …)` with `tool =
///    step.action` (the profile only matters to `findings.record`).
///    Agent-step tool calls never reach this dispatcher.
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
    findings: Option<rupu_mcp::FindingsContext>,
) -> Arc<ToolDispatcher> {
    let dispatcher = ToolDispatcher::new(
        Arc::clone(registry),
        McpPermission::new(
            rupu_agent::runner::parse_mode_for_runtime(mode_str),
            vec!["*".into()],
        ),
    );
    // Without this context `findings.record` is listed but refuses: an
    // action step could observe a weakness and have nowhere to record it.
    Arc::new(match findings {
        Some(ctx) => dispatcher.with_findings(ctx),
        None => dispatcher,
    })
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
/// `store.approve_gate(run_id, ...)` must have already recorded the
/// decision; this reloads the record, rebuilds the runtime from disk
/// (workflow snapshot + prior step results + `KeychainResolver` + layered
/// config + SCM registry + dispatcher + `DefaultStepFactory`), and
/// re-enters `run_workflow`, which applies every recorded gate decision —
/// only the approved gate's path is released; a sibling gate still parked
/// stays parked (spec §7). When a runner is already executing the run (a
/// gate approved while another gate's path runs), nothing runs here: the
/// result's `handed_off_to` names that runner, which applies the approval.
///
/// `awaited_step_id` is the step the approval acted on (the `step_id`
/// returned by `RunStore::approve_gate`), reported back on the outcome.
///
/// The `store` reference is used for the disk reads; the runtime store
/// `Arc` is rebuilt internally from `global` — the rupu home `store` lives
/// under (`<global>/runs`), named by the caller like
/// [`build_reject_cleanup_opts`]'s rather than resolved from `$RUPU_HOME`
/// here, so one caller's approve-resume and reject-cleanup read and write
/// the same home — identical to the CLI's inline path, so this is safe to
/// call from a context that holds only a borrow.
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
    global: &Path,
    run_id: &str,
    awaited_step_id: &str,
    mode: Option<&str>,
    approver: &str,
    via_timeout: bool,
) -> anyhow::Result<ResumeOutcome> {
    let awaited_step_id = awaited_step_id.to_string();
    let (mut opts, prior_step_results) =
        rebuild_opts_from_disk(store, global, run_id, mode).await?;
    opts.resume_from = Some(rupu_orchestrator::ResumeState::from_approval_with_actor(
        run_id.to_string(),
        prior_step_results,
        awaited_step_id.clone(),
        approver.to_string(),
        via_timeout,
    ));
    let result = run_with_pause_channel(opts, run_id).await?;
    Ok(ResumeOutcome {
        awaited_step_id,
        result,
    })
}

/// Resume a run whose gate decisions are recorded on it
/// (`RunRecord.gate_decisions`) without adding one: the runner applies
/// every recorded decision — an approval releases its gate's path, a
/// rejection prunes it and runs its `on_reject` chain — and carries on with
/// the rest of the run (spec §7, path-scoped gates). Used by `rupu workflow
/// reject` on a path-scoped run and by `rupu workflow resume` (which the
/// cp-serve resume worker spawns after a web decision). When a runner is
/// already executing the run, nothing runs here: the result's
/// `handed_off_to` names it, and it applies the decisions itself. `global`
/// is the rupu home `store` lives under, named by the caller (see
/// [`resume_run`]).
pub async fn resume_decided(
    store: &RunStore,
    global: &Path,
    run_id: &str,
    mode: Option<&str>,
) -> anyhow::Result<OrchestratorRunResult> {
    let (mut opts, prior_step_results) =
        rebuild_opts_from_disk(store, global, run_id, mode).await?;
    opts.resume_from = Some(rupu_orchestrator::ResumeState::from_decisions(
        run_id.to_string(),
        prior_step_results,
    ));
    run_with_pause_channel(opts, run_id).await
}

/// Re-enter `run_workflow` with `opts` (a resume), handing the run a
/// cooperative-pause channel.
async fn run_with_pause_channel(
    mut opts: OrchestratorRunOpts,
    run_id: &str,
) -> anyhow::Result<OrchestratorRunResult> {
    // Hand the resumed (possibly detached) run a cooperative-pause channel,
    // the same one a fresh `rupu workflow run` wires — pre-fix the rebuild
    // left `pause: None`, so `rupu workflow pause` / Esc could never stop a
    // run once it had been approved and resumed. A marker already present
    // (a pause requested while the run was parked at the gate) is honored
    // immediately; the poller then watches for one requested mid-resume.
    let pause_token = tokio_util::sync::CancellationToken::new();
    let pause_poller = opts.run_store.as_ref().map(|store_arc| {
        if store_arc.pause_marker_exists(run_id) {
            pause_token.cancel();
        }
        crate::cmd::workflow::spawn_pause_marker_poller(
            Arc::clone(store_arc),
            run_id.to_string(),
            pause_token.clone(),
        )
    });
    opts.pause = Some(pause_token);

    let result = run_workflow(opts).await;
    // Stop the poller the moment the run reaches any terminal/parked state so
    // it never outlives its run (mirrors the fresh-run path's `.abort()`).
    if let Some(handle) = pause_poller {
        handle.abort();
    }
    Ok(result?)
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
///
/// `global` is the rupu home the caller's `store` lives under
/// (`<global>/runs`): the rebuilt opts read their config from it and
/// write the chain's step results, events and netflow ledger back under
/// it, so the caller names it rather than this resolving `$RUPU_HOME`
/// on its own — the gate sweep's tests give it a temporary one.
pub async fn build_reject_cleanup_opts(
    store: &RunStore,
    global: &Path,
    run_id: &str,
    rejected_step_id: &str,
    reason: &str,
    mode: Option<&str>,
) -> anyhow::Result<(OrchestratorRunOpts, usize)> {
    let (mut opts, prior_step_results) =
        rebuild_opts_from_disk(store, global, run_id, mode).await?;
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

/// The directory a resumed run looks its customer up from: the launch's
/// persisted `<run>/customer_dir` (the repo's checkout for an autoflow
/// worktree or clone-target run), else — for a run that predates it — its
/// `workspace_path`. A sidecar that exists but cannot be read is an error,
/// never a silent fall back; so is one naming a directory that no longer
/// exists — the lookup would walk up to an ancestor and could quietly resume
/// the run on global config.
pub(crate) fn customer_lookup_dir(
    store: &RunStore,
    run_id: &str,
    workspace_path: &Path,
) -> anyhow::Result<PathBuf> {
    let Some(dir) = store
        .read_customer_dir(run_id)
        .map_err(|e| anyhow::anyhow!("read customer lookup dir of run {run_id}: {e}"))?
    else {
        return Ok(workspace_path.to_path_buf());
    };
    if !dir.is_dir() {
        anyhow::bail!(
            "the launch directory of run {run_id} ({}) no longer exists, so its customer \
             cannot be determined; restore the directory, or write the project's current \
             path into {} to resume the run under its customer",
            dir.display(),
            store.customer_dir_path(run_id).display()
        );
    }
    Ok(dir)
}

/// Shared disk-rebuild step for [`resume_run`] (approve-resume) and
/// [`build_reject_cleanup_opts`] (reject-cleanup): reload the persisted
/// workflow snapshot + prior step results and reconstruct the full
/// `OrchestratorRunOpts` wiring (resolver, layered config, SCM registry,
/// dispatcher, `DefaultStepFactory`, event sink) exactly as the original
/// run used. Returns the opts with `resume_from: None` — callers set it
/// afterward to the resume shape they need. `global` is the rupu home
/// `store` lives under (see [`build_reject_cleanup_opts`]).
async fn rebuild_opts_from_disk(
    store: &RunStore,
    global: &Path,
    run_id: &str,
    mode: Option<&str>,
) -> anyhow::Result<(OrchestratorRunOpts, Vec<rupu_orchestrator::StepResult>)> {
    let global = global.to_path_buf();
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
    let customer_lookup_dir = customer_lookup_dir(store, run_id, &workspace_path)?;
    let cfg_paths = paths::config_paths(&global, project_root.as_deref(), &customer_lookup_dir)?;
    let cfg = rupu_config::layer_files_locked(cfg_paths.layers())?;
    // Rooted at `global` like everything else here: the resolver reads
    // `<global>/auth.json` and never resolves the home on its own.
    let resolver = Arc::new(crate::accounts::resolver_for_home(&cfg, &global));

    // Netflow capture for this resumed run. This run's own linear steps
    // write to the SAME `<transcripts>/<run_id>.jsonl` file (mirrors
    // `DefaultStepFactory::build_opts_for_step`'s per-step ledger/
    // transcript naming for a linear step, which reuses the workflow's own
    // run id) — so this registry's sink and every step's own sink converge
    // on the one ledger for this run. The writer handle is intentionally
    // not held/shut down here: the sink Arc lives on inside `mcp_registry`
    // (and every connector it hands out) for as long as the resumed run
    // needs it, which is the same "drop the local handle, let the last
    // sink holder's drop close the channel" pattern every other in-run
    // sink build in this crate relies on.
    let (netflow_sink, _netflow_handle) = crate::netflow_sink::for_run(
        &global,
        project_root.as_deref(),
        run_id,
        &transcripts.join(format!("{run_id}.jsonl")),
    );
    let mcp_registry =
        Arc::new(rupu_scm::Registry::discover(resolver.as_ref(), &cfg, netflow_sink).await);

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
    let kinds = rupu_runtime::provider_factory::resolve_kind_map(&cfg.providers);
    let limits_ctx = rupu_runtime::model_limits::LimitsContext::from_config(&cfg, &global);
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
        kinds.clone(),
        crate::findings_opts::base_options(&global, &cfg.findings),
        // Dispatched children append the resumed run's own ledger.
        Some(rupu_orchestrator::usage_ledger::UsageLedger::for_run(
            &store_arc, run_id,
        )),
        limits_ctx.clone(),
        None,
        cfg.providers.clone(),
        cfg.recovery.clone(),
    );
    // One codename namer for the whole run, shared by the orchestrator
    // (static slots) and the sub-agent dispatcher (`>role#n`). Built over
    // the same `<runs>/<run_id>` dir `run_workflow` would use, so both
    // read and persist the one `codenames.json`.
    let naming = Arc::new(rupu_orchestrator::codenames::RunNaming::open(
        &workflow,
        run_id,
        Some(&store_arc.root.join(run_id)),
    ));
    dispatcher.set_namer(naming.namer());
    // Process-wide subprocess-capture backend, warmed off the async runtime
    // (its first call blocks). Shared by the dispatcher (children) and the
    // step factory (this workflow's steps).
    let net_capture = crate::netflow_sink::net_capture(&cfg.netflow).await;
    dispatcher.set_net_capture(Arc::clone(&net_capture));
    let dispatcher_dyn: Arc<dyn rupu_tools::AgentDispatcher> = dispatcher;
    let action_dispatcher = action_dispatcher_for(
        &mcp_registry,
        &mode_str,
        Some(rupu_mcp::FindingsContext {
            workspace_path: workspace_path.clone(),
            scope_name: workflow.name.clone(),
            run_id: run_id.to_string(),
            model: cfg.default_model.clone().unwrap_or_default(),
            surface: rupu_coverage::Surface::Workflow,
            options: crate::findings_opts::base_options(&global, &cfg.findings).with_profile(
                rupu_coverage::FindingProfile::resolve(
                    None,
                    workflow.defaults.findings_profile,
                    None,
                ),
            ),
            codename: Some(rupu_codename::crew_for(run_id)),
            provider: cfg.default_provider.clone(),
        }),
    );
    let factory = Arc::new(DefaultStepFactory {
        workflow: workflow.clone(),
        global: global.clone(),
        project_root: project_root.clone(),
        resolver,
        mode_str: mode_str.clone(),
        mcp_registry,
        system_prompt_suffix: None,
        dispatcher: Some(dispatcher_dyn),
        openai_compatible,
        provider_tuning,
        kinds,
        default_provider: cfg.default_provider.clone(),
        default_model: cfg.default_model.clone(),
        bash_timeout_secs: cfg.bash.timeout_secs.unwrap_or(120),
        bash_env_allowlist: cfg.bash.env_allowlist.clone().unwrap_or_default(),
        findings_base: crate::findings_opts::base_options(&global, &cfg.findings),
        limits_ctx,
        providers: cfg.providers.clone(),
        recovery: cfg.recovery.clone(),
        // A resumed run is never an agentiflow unit (workflow units refuse
        // gated workflows), so it keeps the workflow's own scope.
        scope_name_override: None,
        net_capture: Some(net_capture),
    });

    // Rebuild the `run:` step policy from the resolved mode + layered config
    // + workspace, exactly as the fresh-run path does
    // (`cmd/workflow.rs::run_step_policy_for`). Defaulting it here (the
    // pre-fix behavior) silently discarded the operator's
    // `[workflow] run_step_enabled`/allowlist, so a `run:` step reached after
    // the gate — or in an `on_reject` cleanup chain — was refused with
    // `ConfigDisabled` regardless of config.
    let run_step =
        crate::cmd::workflow::run_step_policy_for(&mode_str, &cfg, workspace_path.clone());

    // Rebuild the fan-out unit dispatcher the same way the fresh-run path
    // does (`build_dispatcher_if_needed`): `None` when the workflow has no
    // `distribute:`/`host:` step, a real dispatcher otherwise. Defaulting it
    // to `None` here would have failed any distributed fan-out step reached
    // after the gate. Wired to the same store the resumed run reads.
    let unit_dispatcher = crate::fleet_unit_dispatcher::build_dispatcher_if_needed(
        &workflow,
        &global,
        Arc::clone(&store_arc),
        cfg.pricing.clone(),
    );

    let opts = OrchestratorRunOpts {
        run_step,
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
        unit_dispatcher,
        action_dispatcher: Some(action_dispatcher),
        // `pause` is wired by `resume_run` (the approve-resume path), not
        // here: the reject-cleanup path runs uninterrupted by design (see
        // `run_reject_cleanup`), so it must keep `None`.
        pause: None,
        naming: Some(naming),
    };

    Ok((opts, prior_step_results))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_run(tmp: &Path) -> RunStore {
        std::fs::create_dir_all(tmp.join("runs/run_1")).unwrap();
        RunStore::new(tmp.join("runs"))
    }

    #[test]
    fn customer_lookup_dir_without_a_sidecar_is_the_workspace_path() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_with_run(tmp.path());
        let ws = tmp.path().join("gone-worktree");
        assert_eq!(customer_lookup_dir(&store, "run_1", &ws).unwrap(), ws);
    }

    #[test]
    fn customer_lookup_dir_uses_the_persisted_launch_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_with_run(tmp.path());
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        store.write_customer_dir("run_1", &repo).unwrap();
        assert_eq!(
            customer_lookup_dir(&store, "run_1", &tmp.path().join("worktree")).unwrap(),
            repo
        );
    }

    #[test]
    fn customer_lookup_dir_refuses_a_launch_dir_that_no_longer_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_with_run(tmp.path());
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        store.write_customer_dir("run_1", &repo).unwrap();
        std::fs::remove_dir(&repo).unwrap();
        let err = customer_lookup_dir(&store, "run_1", tmp.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("run_1"), "{err}");
        assert!(err.contains(&repo.display().to_string()), "{err}");
        assert!(err.contains("no longer exists"), "{err}");
    }
}
