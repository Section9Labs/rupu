//! The roster: `agents.list` / `agents.get`, `workflows.list` /
//! `workflows.get` and `catalog.search` — the agents and workflows a run can
//! draw on, read through the run's [`CatalogPort`]
//! ([`ToolServices::catalog`](crate::ToolServices::catalog)).
//!
//! Every tool returns COMPACT JSON (names, descriptions, shapes — never the
//! raw `.md` / `.yaml`) so the caller's context stays small. A catalog that
//! can't list its agents (one unparseable `.md`) is reported, not
//! propagated, and `catalog.search` still searches the workflows; a workflow
//! that does not parse is listed with its `parse_error`.

use crate::descriptor::{Effect, Service, ToolDescriptor};
use crate::output::{failed, ok_json, req_str};
use crate::ports::{AgentEntry, CatalogPort, WorkflowEntry};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub static AGENTS_LIST: ToolDescriptor = ToolDescriptor {
    name: "agents.list",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Catalog],
    uses: &[],
    description: "List the agents you can draw on (global and project): each one's name, \
     description and declared tools. `tools: null` means the agent uses the \
     default tool set, not none. Use agents.get for one agent's detail.",
    input_schema: agents_list_schema,
};

fn agents_list_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

pub static AGENTS_GET: ToolDescriptor = ToolDescriptor {
    name: "agents.get",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Catalog],
    uses: &[],
    description: "Get one agent's detail by name: description, provider, model, declared \
     tools, the agents it may dispatch, and its permission mode.",
    input_schema: agents_get_schema,
};

fn agents_get_schema() -> Value {
    json!({
        "type": "object",
        "required": ["name"],
        "properties": {
            "name": { "type": "string", "description": "The agent's name, as agents.list shows it" }
        }
    })
}

pub static WORKFLOWS_LIST: ToolDescriptor = ToolDescriptor {
    name: "workflows.list",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Catalog],
    uses: &[],
    description: "List the workflows you can run (global and project). `id` is the runnable \
     identifier (the file stem); `name` is the declared display name. A \
     workflow that failed to parse is listed with its parse_error.",
    input_schema: workflows_list_schema,
};

fn workflows_list_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

pub static WORKFLOWS_GET: ToolDescriptor = ToolDescriptor {
    name: "workflows.get",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Catalog],
    uses: &[],
    description: "Get one workflow's summary by its `id` (the runnable file stem workflows.list \
     shows, not the declared name): description, scope, declared inputs, step count.",
    input_schema: workflows_get_schema,
};

fn workflows_get_schema() -> Value {
    json!({
        "type": "object",
        "required": ["id"],
        "properties": {
            "id": { "type": "string", "description": "The workflow's id, as workflows.list shows it" }
        }
    })
}

pub static CATALOG_SEARCH: ToolDescriptor = ToolDescriptor {
    name: "catalog.search",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Catalog],
    uses: &[],
    description: "Search the agent and workflow catalog: a case-insensitive substring match \
     over agent name/description and workflow id/name/description. Each hit is \
     tagged kind=agent|workflow. `warnings` is present when part of the catalog \
     could not be read.",
    input_schema: catalog_search_schema,
};

fn catalog_search_schema() -> Value {
    json!({
        "type": "object",
        "required": ["query"],
        "properties": {
            "query": { "type": "string", "description": "Substring to look for (case-insensitive)" }
        }
    })
}

/// The run's catalog. Every launch site provides one; a run without it is
/// never offered these tools (they need [`Service::Catalog`]).
fn catalog(ctx: &ToolContext) -> Result<&Arc<dyn CatalogPort>, ToolError> {
    ctx.services
        .catalog
        .as_ref()
        .ok_or_else(|| ToolError::Execution("this run has no agent/workflow catalog".into()))
}

/// The compact row `agents.list` returns for one agent.
fn agent_row(a: &AgentEntry) -> Value {
    json!({
        "name": a.name,
        "description": a.description,
        "tools": a.tools,
    })
}

/// The compact row `workflows.list` returns for one workflow.
fn workflow_row(w: &WorkflowEntry) -> Value {
    json!({
        "id": w.id,
        "name": w.name,
        "description": w.description,
        "scope": w.scope,
        "step_count": w.step_count,
        "parse_error": w.parse_error,
    })
}

/// `agents.list` -> `[{name, description, tools}]` over global + project agents.
pub struct AgentsListTool;

#[async_trait]
impl Tool for AgentsListTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &AGENTS_LIST
    }

    async fn invoke(&self, _input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(match catalog(ctx)?.agents() {
            Ok(agents) => ok_json(Value::Array(agents.iter().map(agent_row).collect())),
            Err(e) => failed(e.to_string()),
        })
    }
}

/// `agents.get { name }` -> one agent's detail (no system prompt).
pub struct AgentsGetTool;

#[async_trait]
impl Tool for AgentsGetTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &AGENTS_GET
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let name = req_str(&input, "name")?;
        let agents = match catalog(ctx)?.agents() {
            Ok(a) => a,
            Err(e) => return Ok(failed(e.to_string())),
        };
        Ok(match agents.iter().find(|a| a.name == name) {
            Some(a) => ok_json(json!({
                "name": a.name,
                "description": a.description,
                "provider": a.provider,
                "model": a.model,
                "tools": a.tools,
                "dispatchable_agents": a.dispatchable_agents,
                "permission_mode": a.permission_mode,
                "max_turns": a.max_turns,
            })),
            None => failed(format!("agent not found: {name}")),
        })
    }
}

/// `workflows.list` -> `[{id, name, description, scope, step_count, parse_error}]`.
pub struct WorkflowsListTool;

#[async_trait]
impl Tool for WorkflowsListTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &WORKFLOWS_LIST
    }

    async fn invoke(&self, _input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(ok_json(Value::Array(
            catalog(ctx)?.workflows().iter().map(workflow_row).collect(),
        )))
    }
}

/// `workflows.get { id }` -> one workflow's summary, addressed by its `id`.
pub struct WorkflowsGetTool;

#[async_trait]
impl Tool for WorkflowsGetTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &WORKFLOWS_GET
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let id = req_str(&input, "id")?;
        Ok(
            match catalog(ctx)?.workflows().into_iter().find(|w| w.id == id) {
                Some(w) => {
                    let mut row = workflow_row(&w);
                    row["input_keys"] = json!(w.input_keys);
                    ok_json(row)
                }
                None => failed(format!("workflow not found: {id}")),
            },
        )
    }
}

/// Case-insensitive substring match of an already-lowercased `needle` over
/// any of `fields`.
fn matches(needle: &str, fields: &[Option<&str>]) -> bool {
    fields
        .iter()
        .flatten()
        .any(|f| f.to_lowercase().contains(needle))
}

/// `catalog.search { query }` -> `{hits: [{kind, id|name, description}], warnings?}`.
pub struct CatalogSearchTool;

#[async_trait]
impl Tool for CatalogSearchTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &CATALOG_SEARCH
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let needle = req_str(&input, "query")?.to_lowercase();
        let catalog = catalog(ctx)?;
        let mut hits: Vec<Value> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();

        match catalog.agents() {
            Ok(agents) => {
                for a in &agents {
                    if matches(&needle, &[Some(&a.name), a.description.as_deref()]) {
                        hits.push(json!({
                            "kind": "agent",
                            "name": a.name,
                            "description": a.description,
                        }));
                    }
                }
            }
            // One bad agent file must not hide the workflows from the search.
            Err(e) => warnings.push(e.to_string()),
        }

        for w in &catalog.workflows() {
            if matches(
                &needle,
                &[Some(&w.id), Some(&w.name), w.description.as_deref()],
            ) {
                hits.push(json!({
                    "kind": "workflow",
                    "id": w.id,
                    "name": w.name,
                    "description": w.description,
                }));
            }
        }

        let mut out = json!({ "hits": hits });
        if !warnings.is_empty() {
            out["warnings"] = json!(warnings);
        }
        Ok(ok_json(out))
    }
}
