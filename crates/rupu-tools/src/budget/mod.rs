//! `budget.status`: re-check a flow's budget against what it has spent right
//! now, through the run's [`RunStatus`](crate::ports::RunStatus) port.

use crate::descriptor::{Effect, Service, ToolDescriptor};
use crate::goal::{ask, run_status};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};

pub static STATUS: ToolDescriptor = ToolDescriptor {
    name: "budget.status",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::RunStatus],
    uses: &[],
    description: "Re-check the run's budget against what it has spent right now: the \
     lead's own and every unit's usage so far, read fresh. `stage` is `ok`, \
     `soft` (a dimension crossed its soft threshold: converge) or `hard` (a \
     cap is reached and the run stops at the next round check; `tripped` \
     names the dimension). Each dimension the flow sets is reported with its \
     `cap`, what it has used (`spent` for `usd` / `tokens`, `elapsed` rounds \
     completed before this one or `elapsed_secs` for `wall_clock`), what is \
     `remaining` and its own `state`. A `usd` cap nothing can price carries \
     `enforced: false` and a note. `{budget: null}` when the flow sets no \
     budget.",
    input_schema: budget_status_schema,
};

fn budget_status_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

/// `budget.status` -> [`crate::ports::BudgetReport::to_json`].
pub struct BudgetStatusTool;

#[async_trait]
impl Tool for BudgetStatusTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &STATUS
    }

    async fn invoke(&self, _input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(ask(run_status(ctx)?, |s| s.budget(), |r| r.to_json()).await)
    }
}
