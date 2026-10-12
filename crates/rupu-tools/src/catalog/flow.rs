//! Descriptors of the agentiflow launch tools — `dispatch`, `join`,
//! `run_workflow` and `workflows.generate` — whose bodies stay in
//! `rupu-agentiflow` until W7 replaces them with the unified `dispatch` /
//! `join`. Every other flow tool (board, mailbox, roster, status) is a
//! catalog tool with its body in this crate (W5).

use crate::descriptor::{Alias, Effect, Service, ToolDescriptor};
use serde_json::{json, Value};

/// Hard ceiling on `join`'s `timeout_secs`, so one call cannot park a lead turn
/// forever; the lead can join again to keep waiting.
pub const MAX_JOIN_TIMEOUT_SECS: u64 = 3600;

/// Every launch-tool descriptor: the catalog tools with no body in this crate
/// ([`crate::bodies::body`] returns `None` for them).
pub static ALL: &[&ToolDescriptor] = &[&DISPATCH, &RUN_WORKFLOW, &WORKFLOWS_GENERATE, &JOIN];

// was `dispatch`
pub static DISPATCH: ToolDescriptor = ToolDescriptor {
    name: "dispatch",
    aliases: &[],
    effect: Effect::Spawn,
    needs: &[Service::Launcher],
    uses: &[],
    description: "Start a pool agent as an independent unit working on a prompt, in its own \
     process. Returns a handle immediately without waiting; pass it to `join` \
     to wait for the unit's result. Only agents in the flow's pool can be \
     dispatched.",
    input_schema: dispatch_schema,
};

fn dispatch_schema() -> Value {
    json!({
        "type": "object",
        "required": ["agent", "prompt"],
        "properties": {
            "agent": {
                "type": "string",
                "description": "Name of a pool agent to run"
            },
            "prompt": {
                "type": "string",
                "description": "The unit's task: what to do and what to report back"
            }
        }
    })
}

// was `run_workflow`
pub static RUN_WORKFLOW: ToolDescriptor = ToolDescriptor {
    name: "run_workflow",
    aliases: &[],
    effect: Effect::Spawn,
    needs: &[Service::Launcher],
    uses: &[],
    description: "Start a pool workflow as an independent unit, in its own process. Returns \
     a handle immediately without waiting; pass it to `join` to wait for the \
     workflow's result. Only workflows in the flow's pool can be started, and \
     only if every agent they dispatch is also in the pool. Workflows with an \
     approval gate or a host / distribute placement cannot be run as a unit.",
    input_schema: run_workflow_schema,
};

fn run_workflow_schema() -> Value {
    json!({
        "type": "object",
        "required": ["workflow"],
        "properties": {
            "workflow": {
                "type": "string",
                "description": "Id of a pool workflow to run"
            },
            "inputs": {
                "type": "object",
                "description": "The workflow's declared inputs, as name: value pairs",
                "additionalProperties": { "type": ["string", "number", "boolean"] }
            }
        }
    })
}

// was `generate_workflow`
pub static WORKFLOWS_GENERATE: ToolDescriptor = ToolDescriptor {
    name: "workflows.generate",
    aliases: &[Alias::any("generate_workflow")],
    effect: Effect::Spawn,
    needs: &[Service::WorkflowGenerator],
    uses: &[],
    description: "Author a NEW workflow for a described task and run it as an independent \
     unit, in its own process. Returns a handle immediately without waiting; \
     pass it to `join` to wait for the workflow's result. The workflow may \
     only dispatch agents in this flow's pool; it runs unattended (no approval \
     gates, no remote placement).",
    input_schema: workflows_generate_schema,
};

fn workflows_generate_schema() -> Value {
    json!({
        "type": "object",
        "required": ["description"],
        "properties": {
            "description": {
                "type": "string",
                "description": "What the workflow should do, in plain language"
            },
            "inputs": {
                "type": "object",
                "description": "Values for any inputs the generated workflow declares, as name: value pairs",
                "additionalProperties": { "type": ["string", "number", "boolean"] }
            }
        }
    })
}

// was `join`
pub static JOIN: ToolDescriptor = ToolDescriptor {
    name: "join",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Launcher],
    uses: &[],
    description: "Wait for a dispatched unit to finish and return its result. Reports \
     status `done` (with the unit's output and whether it succeeded), `failed`, \
     or `running` / `pending` if it has not finished within the timeout (join \
     again to keep waiting).",
    input_schema: join_schema,
};

fn join_schema() -> Value {
    json!({
        "type": "object",
        "required": ["handle"],
        "properties": {
            "handle": {
                "type": "string",
                "description": "The handle `dispatch` returned"
            },
            "timeout_secs": {
                "type": "integer",
                "minimum": 0,
                "maximum": MAX_JOIN_TIMEOUT_SECS,
                "description": "How long to wait, in seconds; default 300"
            }
        }
    })
}
