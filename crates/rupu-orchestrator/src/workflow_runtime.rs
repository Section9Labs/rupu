//! What every local workflow run is built on (spec 2026-10-07 W3, R12): the
//! run's assembler, the in-process sub-agent dispatcher its steps share, the
//! step factory, the run's codename namer and what its `action:` steps call
//! their tools with (W4). One construction for
//! `rupu workflow run`, its inline approve-resume, and `rupu workflow
//! resume` / reject cleanup.

use std::sync::Arc;

use rupu_runtime::assembly::{AssemblyContext, RunAssembler, WorkspaceBinding};
use rupu_runtime::dispatch::InProcessDispatcher;
use rupu_tools::PermissionMode;

use crate::codenames::RunNaming;
use crate::executor::EventSink;
use crate::runner::ActionServices;
use crate::runs::RunStore;
use crate::step_factory::DefaultStepFactory;
use crate::workflow::Workflow;

/// What a workflow run's runtime is built from.
pub struct WorkflowRuntimeInputs {
    /// The run's config, SCM registry, findings base (with its engagement),
    /// capture backend and customer.
    pub context: AssemblyContext,
    pub workflow: Workflow,
    pub run_id: String,
    pub workspace: WorkspaceBinding,
    pub store: Arc<RunStore>,
    /// Where the dispatcher reports `DispatchStarted` / `DispatchCompleted`.
    pub events: Option<Arc<dyn EventSink>>,
    pub mode: PermissionMode,
    /// The run's `## Run target` — recorded at launch, read back on resume
    /// (R8).
    pub system_prompt_suffix: Option<String>,
    /// An agentiflow-launched workflow unit's findings scope.
    pub scope_name_override: Option<String>,
}

/// A workflow run's assembler, sub-agent dispatcher, step factory, namer and
/// action services.
pub struct WorkflowRuntime {
    pub assembler: Arc<RunAssembler>,
    pub dispatcher: Arc<InProcessDispatcher>,
    pub factory: Arc<DefaultStepFactory>,
    /// One codename namer for the whole run, shared by the orchestrator
    /// (static slots) and the dispatcher (`>role#n`), over the
    /// `<runs>/<run_id>` dir `run_workflow` uses, so both read and persist
    /// one `codenames.json`.
    pub naming: Arc<RunNaming>,
    /// What the run's `action:` steps (gate `notify:` hooks and `on_reject`
    /// cleanup steps too) call their catalog tool with.
    pub action_services: ActionServices,
}

impl WorkflowRuntime {
    pub fn new(inputs: WorkflowRuntimeInputs) -> Self {
        let WorkflowRuntimeInputs {
            context,
            workflow,
            run_id,
            workspace,
            store,
            events,
            mode,
            system_prompt_suffix,
            scope_name_override,
        } = inputs;
        let assembler = Arc::new(RunAssembler::new(context));
        let action_services = action_services(
            &assembler,
            &workflow,
            &run_id,
            &workspace,
            mode,
            scope_name_override.as_deref(),
        );
        let naming = Arc::new(RunNaming::open(
            &workflow,
            &run_id,
            Some(&store.root.join(&run_id)),
        ));
        let dispatcher = crate::subagents::workflow_dispatcher(
            &assembler,
            store,
            &run_id,
            &workflow.name,
            workspace,
            events,
        );
        dispatcher.set_namer(naming.namer());
        let factory = Arc::new(DefaultStepFactory {
            workflow,
            assembler: Arc::clone(&assembler),
            mode,
            system_prompt_suffix,
            dispatcher: Some(Arc::clone(&dispatcher) as Arc<dyn rupu_tools::AgentDispatcher>),
            scope_name_override,
        });
        Self {
            assembler,
            dispatcher,
            factory,
            naming,
            action_services,
        }
    }
}

/// The action services of the workflow run `run_id`: its identity — the
/// run id, its crew codename (an action step is no agent instance), the
/// configured default model/provider, the workflow surface, the findings
/// scope (the workflow's name unless an agentiflow unit pools it) — over the
/// assembler's call-site context (workspace, SCM registry, findings base,
/// customer). Each call's findings profile is the step's
/// (`Workflow::action_findings_profile`); the run default seeds it here.
fn action_services(
    assembler: &RunAssembler,
    workflow: &Workflow,
    run_id: &str,
    workspace: &WorkspaceBinding,
    mode: PermissionMode,
    scope_name_override: Option<&str>,
) -> ActionServices {
    let config = &assembler.context().config;
    let identity = rupu_tools::RunIdentity {
        run_id: run_id.to_string(),
        codename: Some(rupu_codename::crew_for(run_id)),
        model: config.default_model.clone().unwrap_or_default(),
        provider: config.default_provider.clone().unwrap_or_default(),
        surface: rupu_tools::Surface::Workflow,
        scope_name: Some(
            scope_name_override
                .map(str::to_string)
                .unwrap_or_else(|| workflow.name.clone()),
        ),
        ..Default::default()
    };
    // No subprocess runs here (`bash` is not action-eligible), and the
    // connector tools' HTTP is attributed by the SCM registry's own sink.
    let mut context =
        assembler.call_site_context(identity, workspace, Arc::new(rupu_netflow::NullSink));
    if let Some(findings) = context.services.findings.as_mut() {
        findings.profile =
            rupu_coverage::FindingProfile::resolve(None, workflow.defaults.findings_profile, None);
    }
    ActionServices { context, mode }
}
