//! The in-process sub-agent dispatcher a workflow run hands its steps
//! (`rupu_runtime::dispatch::InProcessDispatcher`), wired to the run store
//! (sub-run allocation) and the run's event sink (`DispatchStarted` /
//! `DispatchCompleted`). One construction for `rupu workflow run`, its
//! inline approve-resume, and `rupu workflow resume` (W3, R12).

use std::sync::Arc;

use rupu_runtime::assembly::{Origin, RunAssembler, WorkspaceBinding};
use rupu_runtime::dispatch::InProcessDispatcher;

use crate::executor::{DispatchEventSink, EventSink};
use crate::runs::RunStore;

/// The dispatcher for the workflow run `run_id` of `workflow_name`: its
/// children charge the run's own ledger (`<runs>/<run_id>/usage.jsonl`),
/// report under their parent step's scope, and appear on `events` under
/// the dispatching step.
pub fn workflow_dispatcher(
    assembler: &Arc<RunAssembler>,
    store: Arc<RunStore>,
    run_id: &str,
    workflow_name: &str,
    workspace: WorkspaceBinding,
    events: Option<Arc<dyn EventSink>>,
) -> Arc<InProcessDispatcher> {
    let root = assembler.dispatch_root(
        &Origin::WorkflowStep {
            workflow_run_id: run_id.to_string(),
            workflow_name: workflow_name.to_string(),
            step_id: String::new(),
            unit: None,
            step_actions: Vec::new(),
            scope_override: None,
        },
        run_id,
        workspace,
    );
    InProcessDispatcher::new(
        Arc::clone(assembler),
        store,
        root,
        events.map(|sink| Arc::new(DispatchEventSink(sink)) as _),
    )
}
