//! One tool call made outside the agent loop: an `action:` workflow step or
//! a `rupu mcp serve` client's `tools/call`. The permission decision is the
//! run's [`PermissionPolicy`] over the tool's effect, exactly as the agent
//! loop decides; what differs is only who asked.

use crate::permission::{AllowAlways, Decision, DenyReason, PermissionPolicy};
use crate::tool::{Tool, ToolContext, ToolError};
use serde_json::Value;

/// What one call came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallOutcome {
    /// The tool ran and succeeded: its text.
    Done(String),
    /// The permission policy refused the call; the tool never ran.
    Denied(String),
    /// The tool ran and failed, or refused its input.
    Failed(String),
}

impl CallOutcome {
    /// The failure or denial text; `None` for [`CallOutcome::Done`].
    pub fn error(&self) -> Option<&str> {
        match self {
            CallOutcome::Done(_) => None,
            CallOutcome::Denied(m) | CallOutcome::Failed(m) => Some(m),
        }
    }
}

/// Decide and, when allowed, invoke `tool` with `input` in `ctx`. A `Spawn`
/// call carries the decision's ceiling to the child, as in the agent loop.
pub async fn call(
    tool: &dyn Tool,
    input: Value,
    ctx: &ToolContext,
    policy: &PermissionPolicy,
    always: &mut AllowAlways,
) -> CallOutcome {
    let d = tool.descriptor();
    let mut spawn_ctx;
    let ctx = match policy.decide(d, &input, always) {
        Decision::Allow | Decision::AllowDegraded => ctx,
        Decision::Spawn { ceiling } => {
            spawn_ctx = ctx.clone();
            spawn_ctx.call.spawn_ceiling = Some(ceiling);
            &spawn_ctx
        }
        Decision::Deny {
            reason: DenyReason::Readonly,
        } => {
            return CallOutcome::Denied(format!(
                "{} mode blocks {} tools",
                policy.mode(),
                d.effect.as_str()
            ))
        }
        Decision::Deny {
            reason: DenyReason::OperatorDenied,
        } => return CallOutcome::Denied("the operator denied the call".into()),
        Decision::Stop => return CallOutcome::Denied("the operator stopped the run".into()),
    };
    match tool.invoke(input, ctx).await {
        Ok(out) => match out.error {
            None => CallOutcome::Done(out.stdout),
            Some(e) => CallOutcome::Failed(e),
        },
        // A connector failure's text is already its whole message.
        Err(ToolError::Execution(m)) => CallOutcome::Failed(m),
        Err(e) => CallOutcome::Failed(e.to_string()),
    }
}
