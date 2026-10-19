//! The operator tool table: each tool is one CP API route, described by its
//! method, path template and the handler's own query/body structs (their
//! `JsonSchema` is the tool's input schema).

use rupu_mcp::ToolHints;

type SchemaFn = fn() -> Option<schemars::schema::RootSchema>;

/// One operator tool.
pub struct OpSpec {
    pub name: &'static str,
    pub method: http::Method,
    /// CP path with `:params`; `{root}` = the message route segment chosen
    /// from the `id` argument's prefix (`MessageRoute::of_id`).
    pub path: &'static str,
    pub description: &'static str,
    /// The handler's query struct schema; its properties are taken from the
    /// arguments into the URI.
    pub query: SchemaFn,
    /// The handler's body struct schema; the remaining arguments form the
    /// JSON body.
    pub body: SchemaFn,
    pub hints: ToolHints,
}

fn none() -> Option<schemars::schema::RootSchema> {
    None
}

fn schema<T: schemars::JsonSchema>() -> Option<schemars::schema::RootSchema> {
    Some(schemars::schema_for!(T))
}

const READ: ToolHints = ToolHints {
    read_only: true,
    destructive: false,
    idempotent: false,
    open_world: false,
};
const DESTRUCTIVE: ToolHints = ToolHints {
    read_only: false,
    destructive: true,
    idempotent: false,
    open_world: false,
};
const OPEN_WORLD: ToolHints = ToolHints {
    read_only: false,
    destructive: false,
    idempotent: false,
    open_world: true,
};
const PLAIN: ToolHints = ToolHints {
    read_only: false,
    destructive: false,
    idempotent: false,
    open_world: false,
};

/// The 13 operator tools, in listing order.
pub fn ops() -> &'static [OpSpec] {
    static OPS: std::sync::OnceLock<Vec<OpSpec>> = std::sync::OnceLock::new();
    OPS.get_or_init(build)
}

fn build() -> Vec<OpSpec> {
    use crate::api::{agents, messages, runs, sessions, workflows};
    vec![
        OpSpec {
            name: "runs.list",
            method: http::Method::GET,
            path: "/api/runs/workflows",
            description: "List workflow runs, newest first.\n\nFilter with `q` (the query line, e.g. `status:running`), `lifecycle` (active|completed|failed), `since`/`until`, and page with `offset`/`limit`. Returns a JSON array of run rows.",
            query: schema::<runs::WorkflowRunsQuery>,
            body: none,
            hints: READ,
        },
        OpSpec {
            name: "runs.get",
            method: http::Method::GET,
            path: "/api/runs/:id",
            description: "Get one workflow run's full record.\n\n`id` is the run id; `host` picks a remote host. Returns the run detail: status, inputs, steps, any parked gate.",
            query: schema::<runs::RunDetailQuery>,
            body: none,
            hints: READ,
        },
        OpSpec {
            name: "runs.approve",
            method: http::Method::POST,
            path: "/api/runs/:id/approve",
            description: "Approve a workflow run's parked gate so the run continues.\n\n`id` is the run id; `gate` names the gate step when several are parked; `mode` (ask|bypass|readonly) sets the permission mode the resumed run uses. Returns once the approval is recorded, not when the run has resumed.",
            query: schema::<runs::RunControlQuery>,
            body: schema::<runs::ApproveBody>,
            hints: PLAIN,
        },
        OpSpec {
            name: "runs.reject",
            method: http::Method::POST,
            path: "/api/runs/:id/reject",
            description: "Reject a workflow run's parked gate; the run's rejection path runs.\n\n`id` is the run id; `gate` names the gate step when several are parked; `reason` is recorded with the decision. Returns once the rejection is recorded.",
            query: schema::<runs::RunControlQuery>,
            body: schema::<runs::RejectBody>,
            hints: DESTRUCTIVE,
        },
        OpSpec {
            name: "runs.cancel",
            method: http::Method::POST,
            path: "/api/runs/:id/cancel",
            description: "Cancel a running or paused workflow run.\n\n`id` is the run id; `reason` is recorded. A finished run is refused with HTTP 409. Returns once the cancel is recorded.",
            query: schema::<runs::RunControlQuery>,
            body: schema::<runs::CancelBody>,
            hints: DESTRUCTIVE,
        },
        OpSpec {
            name: "workflows.list",
            method: http::Method::GET,
            path: "/api/workflows",
            description: "List the workflows this control plane can run.\n\nTakes no arguments. Returns a JSON array of workflow summaries (name, scope, description).",
            query: none,
            body: none,
            hints: READ,
        },
        OpSpec {
            name: "workflows.run",
            method: http::Method::POST,
            path: "/api/workflows/:name/run",
            description: "Launch a workflow run.\n\n`name` is the workflow; `inputs` maps input names to string values; `mode` (ask|bypass|readonly), `target`, `working_dir`, `host` and `scope_kind`/`scope_id` are optional. Returns the new run id.",
            query: none,
            body: schema::<workflows::LaunchBody>,
            hints: OPEN_WORLD,
        },
        OpSpec {
            name: "agents.list",
            method: http::Method::GET,
            path: "/api/agents",
            description: "List the agents this control plane can run.\n\nTakes no arguments. Returns a JSON array of agent definitions (name, description, provider, model, tools).",
            query: none,
            body: none,
            hints: READ,
        },
        OpSpec {
            name: "agents.run",
            method: http::Method::POST,
            path: "/api/agents/:name/run",
            description: "Launch a standalone agent run.\n\n`name` is the agent; `prompt` is its task; `mode`, `target`, `working_dir`, `host`, `skills` and the other launch options are optional. Returns the new run id.",
            query: none,
            body: schema::<agents::AgentRunBody>,
            hints: OPEN_WORLD,
        },
        OpSpec {
            name: "sessions.start",
            method: http::Method::POST,
            path: "/api/agents/:name/session",
            description: "Start an interactive agent session.\n\n`name` is the agent; `prompt` is the first message; `mode`, `target`, `working_dir`, `host` and `skills` are optional. Returns the new session id.",
            query: none,
            body: schema::<agents::SessionStartBody>,
            hints: OPEN_WORLD,
        },
        OpSpec {
            name: "sessions.send",
            method: http::Method::POST,
            path: "/api/sessions/:id/send",
            description: "Send a message to a live session, starting a new turn.\n\n`id` is the session id; `prompt` is the message and `skills` are skills to invoke (at least one is required). Returns the run id of the turn.",
            query: schema::<sessions::SendQuery>,
            body: schema::<sessions::SendBody>,
            hints: OPEN_WORLD,
        },
        OpSpec {
            name: "messages.list",
            method: http::Method::GET,
            path: "/api/{root}/:id/messages",
            description: "Read the message log of a run, session or agentiflow.\n\n`id` is the root's id (its prefix picks run, session or agentiflow); `after` is a message id to continue from, `limit` caps the page, `kind` filters (comma-separated). Returns the participants and messages.",
            query: schema::<messages::ListQuery>,
            body: none,
            hints: READ,
        },
        OpSpec {
            name: "messages.send",
            method: http::Method::POST,
            path: "/api/{root}/:id/messages",
            description: "Send an operator message to a live run, session or agentiflow.\n\n`id` is the root's id (its prefix picks run, session or agentiflow); `body` is the text, `to` the recipient (default: the root's default), `urgent` interrupts a running turn. Returns the message id and its delivery.",
            query: schema::<messages::HostQuery>,
            body: schema::<messages::PostBody>,
            hints: PLAIN,
        },
    ]
}
