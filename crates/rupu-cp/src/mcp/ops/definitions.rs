//! The definitions operator tools: the workflows and agents the CP can run.

use super::{op, OpSpec};
use crate::api::{agents, workflows};
use http::Method;

pub(super) fn ops() -> Vec<OpSpec> {
    vec![
        op(
            "workflows.list",
            Method::GET,
            "/api/workflows",
            "List the workflows this control plane can run.\n\nTakes no arguments. Returns a JSON array of workflow summaries (name, scope, description).",
        ),
        op(
            "workflows.run",
            Method::POST,
            "/api/workflows/:name/run",
            "Launch a workflow run.\n\n`name` is the workflow; `inputs` maps input names to string values; `mode` (ask|bypass|readonly), `target`, `working_dir`, `host` and `scope_kind`/`scope_id` are optional. Returns the new run id.",
        )
        .body::<workflows::LaunchBody>()
        .open_world(),
        op(
            "agents.list",
            Method::GET,
            "/api/agents",
            "List the agents this control plane can run.\n\nTakes no arguments. Returns a JSON array of agent definitions (name, description, provider, model, tools).",
        ),
        op(
            "agents.run",
            Method::POST,
            "/api/agents/:name/run",
            "Launch a standalone agent run.\n\n`name` is the agent; `prompt` is its task; `mode`, `target`, `working_dir`, `host`, `skills` and the other launch options are optional. Returns the new run id.",
        )
        .body::<agents::AgentRunBody>()
        .open_world(),
    ]
}
