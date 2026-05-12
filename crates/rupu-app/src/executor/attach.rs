//! Decides whether to attach to a run via the in-process executor's
//! broadcast channel or via FileTailRunSource against events.jsonl.

use std::sync::Arc;

use rupu_orchestrator::executor::{
    EventStream, FileTailRunSource, InProcessExecutor, RunFilter, WorkflowExecutor,
};
use rupu_orchestrator::runs::RunStore;

use super::AttachError;

pub async fn attach_stream(
    inner: &Arc<InProcessExecutor>,
    run_store: &Arc<RunStore>,
    run_id: &str,
) -> Result<EventStream, AttachError> {
    let active = inner.list_runs(RunFilter::All);
    if active.iter().any(|r| r.id == run_id) {
        return inner
            .tail(run_id)
            .map_err(|_| AttachError::RunNotFound(run_id.into()));
    }
    let events_path = run_store.events_path(run_id);
    let source = FileTailRunSource::open(&events_path).await?;
    Ok(Box::pin(source))
}
