//! What every local workflow run is built on (spec 2026-10-07 W3, R12): the
//! run's assembler, the in-process sub-agent dispatcher its steps share, the
//! step factory and the run's codename namer. One construction for
//! `rupu workflow run`, its inline approve-resume, and `rupu workflow
//! resume` / reject cleanup.

use std::sync::Arc;

use rupu_runtime::assembly::{AssemblyContext, RunAssembler, WorkspaceBinding};
use rupu_runtime::dispatch::InProcessDispatcher;
use rupu_tools::PermissionMode;

use crate::codenames::RunNaming;
use crate::executor::EventSink;
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

/// A workflow run's assembler, sub-agent dispatcher, step factory and namer.
pub struct WorkflowRuntime {
    pub assembler: Arc<RunAssembler>,
    pub dispatcher: Arc<InProcessDispatcher>,
    pub factory: Arc<DefaultStepFactory>,
    /// One codename namer for the whole run, shared by the orchestrator
    /// (static slots) and the dispatcher (`>role#n`), over the
    /// `<runs>/<run_id>` dir `run_workflow` uses, so both read and persist
    /// one `codenames.json`.
    pub naming: Arc<RunNaming>,
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
        }
    }
}
