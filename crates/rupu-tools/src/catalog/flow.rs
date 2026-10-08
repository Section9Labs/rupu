//! Descriptors of the agentiflow tools: board, mailbox, roster, status and
//! dispatch (bodies in `rupu-agentiflow` until W5 moves them here).

use crate::descriptor::{Alias, Effect, Service, ToolDescriptor};
use serde_json::{json, Value};

/// Hard ceiling on `board.read`'s `limit`, so one call cannot flood the context.
pub const MAX_READ_LIMIT: usize = 500;
/// Hard ceiling on `join`'s `timeout_secs`, so one call cannot park a lead turn
/// forever; the lead can join again to keep waiting.
pub const MAX_JOIN_TIMEOUT_SECS: u64 = 3600;

// was `board.claim`
pub static BOARD_CLAIM: ToolDescriptor = ToolDescriptor {
    name: "board.claim",
    aliases: &[],
    effect: Effect::Record,
    needs: &[Service::MessageBus],
    description: "Atomically claim a work unit so no other participant duplicates it. \
     Returns granted, or the current holder when it is already claimed.",
    input_schema: board_claim_schema,
};

fn board_claim_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "required": ["work_unit"],
        "properties": {
            "work_unit": {
                "type": "string",
                "description": "Identifier of the unit of work to claim, e.g. host:1.1.2.2"
            }
        }
    })
}

// was `board.release`
pub static BOARD_RELEASE: ToolDescriptor = ToolDescriptor {
    name: "board.release",
    aliases: &[],
    effect: Effect::Record,
    needs: &[Service::MessageBus],
    description: "Release a work unit you previously claimed with board.claim, so another \
     participant can take it.",
    input_schema: board_release_schema,
};

fn board_release_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "required": ["work_unit"],
        "properties": {
            "work_unit": {
                "type": "string",
                "description": "Identifier of the claimed unit of work to release"
            }
        }
    })
}

// was `board.post`
pub static BOARD_POST: ToolDescriptor = ToolDescriptor {
    name: "board.post",
    aliases: &[],
    effect: Effect::Record,
    needs: &[Service::MessageBus],
    description: "Post to the shared board every participant can read: an observation, a \
     question, an answer, a vote, or a note. Optionally address it to a \
     participant id or role.",
    input_schema: board_post_schema,
};

fn board_post_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "required": ["kind", "body"],
        "properties": {
            "kind": {
                "type": "string",
                "enum": ["observation", "question", "answer", "vote", "note"]
            },
            "body": { "type": "string" },
            "addressed_to": {
                "type": "string",
                "description": "Participant id or role this post is for; omit for everyone"
            }
        }
    })
}

// was `board.read`
pub static BOARD_READ: ToolDescriptor = ToolDescriptor {
    name: "board.read",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::MessageBus],
    description: "Read the shared board's posts, oldest first, newest last. Returns the \
     most recent posts (default 50); filter to those addressed to a \
     participant id or role with addressed_to.",
    input_schema: board_read_schema,
};

fn board_read_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "addressed_to": {
                "type": "string",
                "description": "Only posts addressed to this participant id or role"
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_READ_LIMIT,
                "description": "Maximum posts to return (the most recent ones); default 50"
            }
        }
    })
}

// was `msg.send`
pub static MSG_SEND: ToolDescriptor = ToolDescriptor {
    name: "msg.send",
    aliases: &[],
    effect: Effect::Record,
    needs: &[Service::MessageBus],
    description: "Send a message. `to` is a participant id, a role, \"parent\", or \
     \"lead\" (delivered to exactly that inbox), or \"broadcast\" (every \
     participant sees it once, from the moment it started listening; the \
     broadcast log is capped per run, so don't spam it).",
    input_schema: msg_send_schema,
};

fn msg_send_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "required": ["to", "body"],
        "properties": {
            "to": {
                "type": "string",
                "description": "Recipient inbox: participant id, role, parent, lead or broadcast"
            },
            "body": { "type": "string" }
        }
    })
}

// was `agents.list`
pub static AGENTS_LIST: ToolDescriptor = ToolDescriptor {
    name: "agents.list",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Catalog],
    description: "List the agents you can draw on (global and project): each one's name, \
     description and declared tools. `tools: null` means the agent uses the \
     default tool set, not none. Use agents.get for one agent's detail.",
    input_schema: agents_list_schema,
};

fn agents_list_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

// was `agents.get`
pub static AGENTS_GET: ToolDescriptor = ToolDescriptor {
    name: "agents.get",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Catalog],
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

// was `workflows.list`
pub static WORKFLOWS_LIST: ToolDescriptor = ToolDescriptor {
    name: "workflows.list",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Catalog],
    description: "List the workflows you can run (global and project). `id` is the runnable \
     identifier (the file stem); `name` is the declared display name. A \
     workflow that failed to parse is listed with its parse_error.",
    input_schema: workflows_list_schema,
};

fn workflows_list_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

// was `workflows.get`
pub static WORKFLOWS_GET: ToolDescriptor = ToolDescriptor {
    name: "workflows.get",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Catalog],
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

// was `catalog.search`
pub static CATALOG_SEARCH: ToolDescriptor = ToolDescriptor {
    name: "catalog.search",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Catalog],
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

// was `goal.status`
pub static GOAL_STATUS: ToolDescriptor = ToolDescriptor {
    name: "goal.status",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::RunStatus],
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
pub static GOAL_COVERAGE: ToolDescriptor = ToolDescriptor {
    name: "goal.coverage",
    aliases: &[Alias::flow_lead("coverage.status")],
    effect: Effect::Read,
    needs: &[Service::RunStatus],
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

// was `budget.status`
pub static BUDGET_STATUS: ToolDescriptor = ToolDescriptor {
    name: "budget.status",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::RunStatus],
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

// was `board.directive`
pub static BOARD_DIRECTIVE: ToolDescriptor = ToolDescriptor {
    name: "board.directive",
    aliases: &[],
    effect: Effect::Record,
    needs: &[Service::MessageBus],
    description: "Steer the fleet: write a standing directive to the board. Units see it on \
     every turn until you lift it, so post sparingly and keep each short and \
     actionable. Returns the directive's `id`: pass it to `board.retract` once \
     the instruction is obsolete. Omit `addressed_to` to address everyone; name \
     a unit or role to address only it.",
    input_schema: board_directive_schema,
};

fn board_directive_schema() -> Value {
    json!({
        "type": "object",
        "required": ["body"],
        "properties": {
            "body": { "type": "string", "description": "The directive text" },
            "addressed_to": {
                "type": "string",
                "description": "A unit name or role to address; omit for everyone"
            }
        }
    })
}

// was `board.retract`
pub static BOARD_RETRACT: ToolDescriptor = ToolDescriptor {
    name: "board.retract",
    aliases: &[],
    effect: Effect::Record,
    needs: &[Service::MessageBus],
    description: "Lift a standing directive: pass the `id` that `board.directive` returned \
     (it is also shown in brackets on each standing directive you see each \
     turn). Units stop seeing it from their next turn. An unknown or already \
     retracted id is reported and changes nothing.",
    input_schema: board_retract_schema,
};

fn board_retract_schema() -> Value {
    json!({
        "type": "object",
        "required": ["id"],
        "properties": {
            "id": {
                "type": "string",
                "description": "The `id` `board.directive` returned for the directive to lift"
            }
        }
    })
}

// was `dispatch`
pub static DISPATCH: ToolDescriptor = ToolDescriptor {
    name: "dispatch",
    aliases: &[],
    effect: Effect::Spawn,
    needs: &[Service::Launcher],
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
