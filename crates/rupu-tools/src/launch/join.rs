//! `join`: wait for children and collect their results (spec W7 §3.1). The
//! agentiflow's `join {handle}` (one handle, singular) answers in its old
//! single-status shape.

use std::time::Instant;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::parse_timeout;
use crate::descriptor::{Effect, Service, ToolDescriptor};
use crate::output::{failed, ok};
use crate::ports::{ChildState, ChildStatus, JoinSet, Until};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};

pub static DESCRIPTOR: ToolDescriptor = ToolDescriptor {
    name: "join",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Launcher],
    uses: &[],
    wildcard: true,
    description: "Wait for children you started with `dispatch` and collect their results. \
     Name `handles` (omit them for every child you have not collected yet). Returns each child's status: `done` (with \
     its output and whether it succeeded), `failed`, `abandoned`, or `running` / `pending` if it \
     has not finished within the timeout (join again to keep waiting). `until: any` returns as \
     soon as one of them finishes.",
    input_schema: schema,
};

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "handles": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Handles `dispatch` returned; omit for every child of this run not yet joined"
            },
            "timeout_secs": {
                "type": "integer",
                "minimum": 0,
                "maximum": super::MAX_JOIN_TIMEOUT_SECS,
                "description": "How long to wait, in seconds (0 = just look); default 300"
            },
            "until": {
                "type": "string",
                "enum": ["all", "any"],
                "description": "Return when all of them have finished (default), or any one"
            }
        }
    })
}

#[derive(Debug, Default, Clone)]
pub struct JoinTool;

/// The agentiflow `join`'s model-facing status: `{status, success?, output?,
/// error?}`.
fn legacy_status(s: &ChildStatus) -> Value {
    match s.state {
        ChildState::Pending => json!({ "status": "pending" }),
        ChildState::Running => json!({ "status": "running" }),
        ChildState::Done => json!({
            "status": "done",
            "success": s.success.unwrap_or(false),
            "output": s.output.clone().unwrap_or_default(),
        }),
        ChildState::Failed | ChildState::Abandoned | ChildState::Interrupted => json!({
            "status": "failed",
            "error": s.error.clone().unwrap_or_else(|| s.state.as_str().to_string()),
        }),
    }
}

#[async_trait]
impl Tool for JoinTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &DESCRIPTOR
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let timeout = parse_timeout(&input)?;
        let Some(launcher) = ctx.services.launcher.clone() else {
            return Ok(failed(
                "launcher_not_configured: this run has no children to join".to_string(),
            ));
        };
        // The agentiflow form: one handle, its old single-status answer.
        if let Some(handle) = input.get("handle") {
            let handle = match handle.as_str().map(str::trim) {
                Some(h) if !h.is_empty() => h.to_string(),
                _ => {
                    return Err(ToolError::InvalidInput(
                        "handle (non-empty string) required".into(),
                    ))
                }
            };
            let statuses = launcher
                .join(
                    &ctx.identity,
                    JoinSet::Handles(vec![handle]),
                    timeout,
                    Until::All,
                )
                .await;
            let body = statuses
                .first()
                .map(legacy_status)
                .unwrap_or_else(|| json!({ "status": "failed", "error": "unknown handle" }));
            return Ok(ok(body.to_string()).timed(started));
        }
        let which = match input.get("handles") {
            None | Some(Value::Null) => JoinSet::All,
            Some(Value::Array(items)) => {
                let mut handles = Vec::with_capacity(items.len());
                for item in items {
                    match item.as_str().map(str::trim) {
                        Some(h) if !h.is_empty() => handles.push(h.to_string()),
                        _ => {
                            return Err(ToolError::InvalidInput(
                                "handles must be an array of non-empty strings".into(),
                            ))
                        }
                    }
                }
                JoinSet::Handles(handles)
            }
            Some(_) => {
                return Err(ToolError::InvalidInput(
                    "handles must be an array of strings when given".into(),
                ))
            }
        };
        let until = match input.get("until").and_then(Value::as_str) {
            None => Until::All,
            Some("all") => Until::All,
            Some("any") => Until::Any,
            Some(other) => {
                return Err(ToolError::InvalidInput(format!(
                    "until must be `all` or `any`, not `{other}`"
                )))
            }
        };
        let statuses = launcher.join(&ctx.identity, which, timeout, until).await;
        let children: Vec<Value> = statuses.iter().map(ChildStatus::to_json).collect();
        Ok(ok(json!({ "children": children }).to_string()).timed(started))
    }
}
