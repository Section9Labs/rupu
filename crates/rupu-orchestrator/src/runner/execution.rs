//! Scheduler execution observation, separate from legacy event/attachment state.
//!
//! The task local is explicitly carried across spawned tasks. A generation belongs
//! to the owning runner invocation, while loop coordinates belong to the current
//! scheduler scope. Child agents are not separate step-body activations.

use super::{OrchestratorRunOpts, RunWorkflowError};
use crate::execution_observation::{
    ExecutionGuard, ExecutionHandle, ExecutionIteration, ExecutionOwner,
};
use crate::runs::RunRecord;
use std::future::Future;
use tracing::warn;

#[derive(Clone)]
struct Context {
    handle: ExecutionHandle,
    iterations: Vec<ExecutionIteration>,
}

tokio::task_local! {
    static CURRENT: Option<Context>;
}

/// Scope only the execution portion of a claimed run. Setup failures before this
/// point have no executing body and cannot establish an observation owner.
pub(super) async fn observe_run<T>(
    opts: &OrchestratorRunOpts,
    run_id: &str,
    record: Option<&RunRecord>,
    future: impl Future<Output = Result<T, RunWorkflowError>>,
) -> Result<T, RunWorkflowError> {
    // A second/unverifiable stored producer must not run outside an incumbent's
    // complete set. Ownership failure is therefore fatal before polling a body.
    // Legacy in-memory and unsupported-platform invocations have no such owner.
    let owner = match opts.run_store.as_ref().zip(record) {
        Some((store, record)) if crate::execution_observation::supported() => Some(
            ExecutionOwner::start(&store.root.join(run_id), record).map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!("execution observation ownership: {error}"),
                )
            })?,
        ),
        _ => None,
    };
    let context = owner.as_ref().map(|owner| Context {
        handle: owner.handle(),
        iterations: Vec::new(),
    });
    let outcome = CURRENT.scope(context, future).await;
    if let Some(owner) = owner {
        if outcome.is_ok() {
            if let Err(error) = owner.finish() {
                warn!(%run_id, %error, "could not close execution observation");
            }
        } else {
            owner.invalidate("scheduler_failed");
        }
    }
    outcome
}

/// Capture at the spawn site, not when the new task first polls. Tokio task
/// locals are otherwise not inherited by spawned futures.
pub(super) fn inherit<F: Future>(future: F) -> impl Future<Output = F::Output> {
    let context = CURRENT.try_with(Clone::clone).ok().flatten();
    CURRENT.scope(context, future)
}

/// Only the actual bounded-loop driver adds ancestry. Template context also
/// contains previously completed loops and is not an execution identity source.
pub(super) fn iteration<F: Future>(
    loop_id: &str,
    iteration: u32,
    future: F,
) -> impl Future<Output = F::Output> {
    let context = CURRENT
        .try_with(Clone::clone)
        .ok()
        .flatten()
        .map(|mut context| {
            context.iterations.push(ExecutionIteration {
                loop_id: loop_id.to_string(),
                iteration,
            });
            context
        });
    CURRENT.scope(context, future)
}

pub(super) struct Guard(Option<ExecutionGuard>);

/// Enter after any scheduler permit is acquired. Admission failures revoke the
/// observation, but do not change the pre-existing execution/cancellation policy.
pub(super) fn enter(step_id: &str) -> Guard {
    let guard = CURRENT
        .try_with(|context| {
            context.as_ref().and_then(|context| {
                match context.handle.enter(step_id, context.iterations.clone()) {
                    Ok(guard) => Some(guard),
                    Err(error) => {
                        context.handle.invalidate("activation_entry_failed");
                        warn!(%step_id, %error, "step execution observation is no longer current");
                        None
                    }
                }
            })
        })
        .ok()
        .flatten();
    Guard(guard)
}

impl Guard {
    /// Normal return only: dropping an unfinished guard invalidates the whole
    /// generation. In particular, an aborted aggregate may have detached inner
    /// JoinHandles, so it must never become a complete current empty set.
    pub(super) fn finish(self) {
        if let Some(guard) = self.0 {
            if let Err(error) = guard.finish() {
                warn!(%error, "step execution observation could not record completion");
            }
        }
    }

    pub(super) fn finish_result<T, E>(self, outcome: &Result<T, E>) {
        if outcome.is_ok() {
            self.finish();
        }
    }
}
