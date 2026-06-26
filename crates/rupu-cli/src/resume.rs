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
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts, OrchestratorRunResult};
use rupu_orchestrator::{DefaultStepFactory, RunStore, Workflow};
use std::collections::BTreeMap;
use std::sync::Arc;

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
/// `bypass` / `readonly`); `None` defaults to `ask`, matching the CLI's
/// `--mode`-absent behavior.
pub async fn resume_run(
    store: &RunStore,
    run_id: &str,
    awaited_step_id: &str,
    mode: Option<&str>,
) -> anyhow::Result<ResumeOutcome> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let runs_dir = global.join("runs");
    let store_arc = Arc::new(rupu_orchestrator::RunStore::new(runs_dir));

    // Reload the record from disk to get inputs, event, workspace path
    // for the run_workflow re-entry. The library call already persisted
    // the status flip to Running, so the record is coherent.
    let record = store
        .load(run_id)
        .map_err(|e| anyhow::anyhow!("reload run record: {e}"))?;

    let awaited_step_id = awaited_step_id.to_string();

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
        Arc::clone(&store_arc),
    );
    let dispatcher_dyn: Arc<dyn rupu_tools::AgentDispatcher> = dispatcher;
    let factory = Arc::new(DefaultStepFactory {
        workflow: workflow.clone(),
        global: global.clone(),
        project_root: project_root.clone(),
        resolver,
        mode_str,
        mcp_registry,
        system_prompt_suffix: None,
        dispatcher: Some(dispatcher_dyn),
    });

    let resume = rupu_orchestrator::ResumeState::from_approval(
        run_id.to_string(),
        prior_step_results,
        awaited_step_id.clone(),
    );
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
        run_store: Some(store_arc),
        workflow_yaml: Some(body),
        resume_from: Some(resume),
        run_id_override: None,
        strict_templates: false,
        event_sink: event_sink_for_resume,
    };

    let result = run_workflow(opts).await?;
    Ok(ResumeOutcome {
        awaited_step_id,
        result,
    })
}
