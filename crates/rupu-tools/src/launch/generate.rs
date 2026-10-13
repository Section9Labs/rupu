//! `workflows.generate`: author a new workflow and save it (spec W7 §3.1).
//! Running it is a separate `dispatch {kind: workflow, file}`. The legacy
//! `generate_workflow` did both, and still does under its old name.

use std::time::Instant;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{parse_inputs, workflow_input_strings};
use crate::descriptor::{Alias, Effect, Service, ToolDescriptor};
use crate::output::{failed, ok, req_str};
use crate::ports::{ChildKind, ChildRequest, ChildTarget};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};

pub static DESCRIPTOR: ToolDescriptor = ToolDescriptor {
    name: "workflows.generate",
    aliases: &[Alias::any("generate_workflow")],
    effect: Effect::Spawn,
    needs: &[Service::WorkflowGenerator],
    uses: &[],
    wildcard: true,
    description: "Author a NEW workflow for a described task and save it. Returns the saved \
     `file`, the workflow's `name`, its step count and its declared inputs; run it with \
     `dispatch {kind: \"workflow\", file}`. The workflow may only use agents you can dispatch, \
     and it runs unattended (no approval gates, no remote placement).",
    input_schema: schema,
};

fn schema() -> Value {
    json!({
        "type": "object",
        "required": ["description"],
        "properties": {
            "description": {
                "type": "string",
                "description": "What the workflow should do, in plain language"
            }
        }
    })
}

#[derive(Debug, Default, Clone)]
pub struct WorkflowsGenerateTool;

#[async_trait]
impl Tool for WorkflowsGenerateTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &DESCRIPTOR
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let description = req_str(&input, "description")?;
        let legacy = ctx.call.called_as.as_deref() == Some("generate_workflow");
        let inputs = parse_inputs(&input)?;
        if legacy {
            workflow_input_strings(&inputs).map_err(ToolError::InvalidInput)?;
        }
        let Some(generator) = ctx.services.workflow_generator.clone() else {
            return Ok(failed("workflow generation is not available in this run").timed(started));
        };
        let generated = match generator.generate(description, ctx).await {
            Ok(g) => g,
            Err(e) => return Ok(failed(e.message).timed(started)),
        };
        if !legacy {
            return Ok(ok(json!({
                "file": generated.file.display().to_string(),
                "name": generated.name,
                "steps": generated.steps,
                "inputs": generated.input_keys,
            })
            .to_string())
            .timed(started));
        }
        // `generate_workflow`: generate, then start it as a workflow child.
        let Some(launcher) = ctx.services.launcher.clone() else {
            return Ok(failed("this run can't start child runs").timed(started));
        };
        let req = ChildRequest {
            kind: ChildKind::Workflow,
            target: ChildTarget::File(generated.file.clone()),
            prompt: None,
            inputs,
        };
        Ok(match launcher.launch(req, ctx).await {
            Ok(h) => ok(json!({
                "handle": h.handle,
                "participant": h.address.unwrap_or(h.name),
                "generated_file": generated.file.display().to_string(),
            })
            .to_string()),
            Err(e) => failed(format!("generated workflow failed to start: {}", e.message)),
        }
        .timed(started))
    }
}
