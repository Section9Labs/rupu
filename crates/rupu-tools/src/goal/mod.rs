//! `goal.status` / `goal.coverage`: re-check a flow's goals and its coverage
//! target against the pooled evidence right now, through the run's
//! [`RunStatus`] port ([`ToolServices::run_status`](crate::ToolServices::run_status)).

use crate::descriptor::{Alias, Effect, Service, ToolDescriptor};
use crate::output::{failed, ok_json};
use crate::ports::{PortError, RunStatus};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub static STATUS: ToolDescriptor = ToolDescriptor {
    name: "goal.status",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::RunStatus],
    uses: &[],
    description: "Re-check every goal against the pooled evidence right now (findings and \
     assets banked since the round began count). Each goal reports `satisfied`, \
     its `current`/`target` tally and a one-line `detail`; a goal that could not \
     be evaluated carries an `error` instead. A findings goal that requires \
     verification also reports in `detail` how many findings match it and how \
     many of those are independently verified, and names the agent that must \
     verify the rest (dispatch it).",
    input_schema: goal_status_schema,
};

fn goal_status_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

// was `coverage.status`
pub static COVERAGE: ToolDescriptor = ToolDescriptor {
    name: "goal.coverage",
    aliases: &[Alias::flow_lead("coverage.status")],
    effect: Effect::Read,
    needs: &[Service::RunStatus],
    uses: &[],
    description: "Re-check the engagement coverage target against the pooled assets right \
     now. `reach` is the required fraction and `depth` the rung assets must \
     have reached (null: the ladder's terminal rung); `fraction` is what has \
     been reached so far, `satisfied` whether it meets `reach`, `per_kind` the \
     per-asset-kind fractions. `{coverage: null}` when the flow sets no \
     coverage target.",
    input_schema: goal_coverage_schema,
};

fn goal_coverage_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

/// The run's status port. A run without one is never offered these tools
/// (they need [`Service::RunStatus`]).
pub(crate) fn run_status(ctx: &ToolContext) -> Result<Arc<dyn RunStatus>, ToolError> {
    ctx.services.run_status.clone().ok_or_else(|| {
        ToolError::Execution("this run has no run status (it is not a flow lead)".into())
    })
}

/// Ask the port off the async runtime (it may read whole ledgers), and turn
/// its answer into the tool's output.
pub(crate) async fn ask<T: Send + 'static>(
    status: Arc<dyn RunStatus>,
    query: impl FnOnce(&dyn RunStatus) -> Result<T, PortError> + Send + 'static,
    render: impl FnOnce(T) -> Value,
) -> ToolOutput {
    match tokio::task::spawn_blocking(move || query(status.as_ref())).await {
        Ok(Ok(report)) => ok_json(render(report)),
        Ok(Err(e)) => failed(e.to_string()),
        Err(e) => failed(format!("status evaluation did not complete: {e}")),
    }
}

/// `goal.status` -> one row per goal ([`crate::ports::GoalReport::to_json`]).
pub struct GoalStatusTool;

#[async_trait]
impl Tool for GoalStatusTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &STATUS
    }

    async fn invoke(&self, _input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(ask(run_status(ctx)?, |s| s.goals(), |r| r.to_json()).await)
    }
}

/// `goal.coverage` (formerly `coverage.status`) -> `{reach, depth, fraction,
/// satisfied, per_kind}`, or `{coverage: null}` when the flow has no coverage
/// target.
pub struct GoalCoverageTool;

#[async_trait]
impl Tool for GoalCoverageTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &COVERAGE
    }

    async fn invoke(&self, _input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(ask(run_status(ctx)?, |s| s.coverage(), |r| r.to_json()).await)
    }
}
