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

/// Routes that are deliberately not operator tools, each with the reason.
/// `tests/it/mcp_routes.rs` holds this list, `ops()` and `PENDING_PLAN_2`
/// in lockstep with the registered routes.
pub const EXCLUDED: &[(&str, &str, &str)] = &[
    ("GET", "/api/runs/:id/log", "SSE stream"),
    ("GET", "/api/events/stream", "SSE stream"),
    ("GET", "/api/transcript/stream", "SSE stream"),
    ("GET", "/api/runs/:id/coverage", "host-to-host transfer"),
    (
        "GET",
        "/api/runs/:id/netflow/stream",
        "host-to-host transfer",
    ),
    (
        "GET",
        "/api/findings/artifacts/:sha256",
        "host-to-host transfer",
    ),
    ("POST", "/api/workspace/stage", "host-to-host transfer"),
    ("GET", "/api/workspace/delta", "host-to-host transfer"),
    ("DELETE", "/api/workspace/discard", "host-to-host transfer"),
    (
        "POST",
        "/api/skills/bundle/missing",
        "host-to-host transfer",
    ),
    ("PUT", "/api/skills/bundle/:hash", "host-to-host transfer"),
    (
        "GET",
        "/api/node/connect",
        "node WebSocket, not a request/response call",
    ),
    ("GET", "/healthz", "liveness probe"),
    (
        "GET",
        "/api/fs/browse",
        "server-side filesystem browser for the web UI",
    ),
    ("POST", "/mcp", "the MCP endpoint itself"),
];

/// Routes not yet operator tools; Plan 2 empties this list.
pub const PENDING_PLAN_2: &[(&str, &str)] = &[
    ("GET", "/api/agentiflows"),
    ("GET", "/api/agentiflows/:id"),
    ("GET", "/api/agentiflows/:id/children"),
    ("GET", "/api/agentiflows/:id/events"),
    ("GET", "/api/agentiflows/:id/messages/stream"),
    ("GET", "/api/agentiflows/:id/netflow"),
    ("POST", "/api/agentiflows/:id/steer"),
    ("GET", "/api/agentiflows/:id/units/:unit_id/output"),
    ("POST", "/api/agents"),
    ("DELETE", "/api/agents/:name"),
    ("GET", "/api/agents/:name"),
    ("PUT", "/api/agents/:name"),
    ("POST", "/api/agents/generate"),
    ("GET", "/api/assets"),
    ("GET", "/api/autoflows"),
    ("POST", "/api/autoflows/:name/disable"),
    ("POST", "/api/autoflows/:name/enable"),
    ("GET", "/api/autoflows/claims"),
    ("POST", "/api/autoflows/claims/release"),
    ("POST", "/api/autoflows/claims/requeue"),
    ("GET", "/api/config"),
    ("PUT", "/api/config/customer/:slug"),
    ("PUT", "/api/config/global"),
    ("PUT", "/api/config/policy"),
    ("PUT", "/api/config/project/:id"),
    ("GET", "/api/coverage"),
    ("GET", "/api/coverage/:target"),
    ("GET", "/api/coverage/:target/audit"),
    ("GET", "/api/coverage/:target/catalog"),
    ("GET", "/api/coverage/:target/diff"),
    ("GET", "/api/coverage/:target/runs"),
    ("GET", "/api/coverage/templates"),
    ("GET", "/api/coverage/templates/:name"),
    ("GET", "/api/customers"),
    ("POST", "/api/customers"),
    ("DELETE", "/api/customers/:slug"),
    ("GET", "/api/customers/:slug"),
    ("PATCH", "/api/customers/:slug"),
    ("POST", "/api/customers/:slug/archive"),
    ("DELETE", "/api/customers/:slug/projects/:ws_id"),
    ("PUT", "/api/customers/:slug/projects/:ws_id"),
    ("POST", "/api/customers/:slug/unarchive"),
    ("GET", "/api/dashboard"),
    ("GET", "/api/events"),
    ("GET", "/api/findings"),
    ("GET", "/api/findings/:id"),
    ("GET", "/api/findings/:id/artifacts/:sha256"),
    ("GET", "/api/findings/:id/export"),
    ("POST", "/api/findings/export"),
    ("GET", "/api/findings/tags"),
    ("POST", "/api/findings/tags"),
    ("GET", "/api/generate/models"),
    ("GET", "/api/host/info"),
    ("GET", "/api/hosts"),
    ("POST", "/api/hosts"),
    ("DELETE", "/api/hosts/:id"),
    ("POST", "/api/hosts/bucket"),
    ("POST", "/api/hosts/node"),
    ("GET", "/api/hosts/registered"),
    ("POST", "/api/hosts/ssh"),
    ("POST", "/api/launch/preview"),
    ("GET", "/api/models"),
    ("POST", "/api/models/refresh"),
    ("GET", "/api/netflow"),
    ("GET", "/api/netflow/explorer"),
    ("GET", "/api/netflow/graph"),
    ("GET", "/api/netflow/index"),
    ("GET", "/api/netflow/latest"),
    ("GET", "/api/projects"),
    ("GET", "/api/projects/:id/netflow"),
    ("GET", "/api/projects/:ws_id"),
    ("GET", "/api/projects/:ws_id/agents"),
    ("GET", "/api/projects/:ws_id/autoflows"),
    ("GET", "/api/projects/:ws_id/coverage"),
    ("GET", "/api/projects/:ws_id/coverage/assessed"),
    ("GET", "/api/projects/:ws_id/files"),
    ("GET", "/api/projects/:ws_id/runs"),
    ("GET", "/api/projects/:ws_id/sessions"),
    ("GET", "/api/projects/:ws_id/source"),
    ("GET", "/api/projects/:ws_id/tree"),
    ("GET", "/api/projects/:ws_id/workflows"),
    ("GET", "/api/repos"),
    ("GET", "/api/runs"),
    ("DELETE", "/api/runs/:id"),
    ("POST", "/api/runs/:id/archive"),
    ("GET", "/api/runs/:id/ast"),
    ("GET", "/api/runs/:id/autoflow"),
    ("GET", "/api/runs/:id/children"),
    ("GET", "/api/runs/:id/graph"),
    ("GET", "/api/runs/:id/messages/stream"),
    ("GET", "/api/runs/:id/netflow"),
    ("POST", "/api/runs/:id/pause"),
    ("POST", "/api/runs/:id/restore"),
    ("POST", "/api/runs/:id/resume"),
    ("GET", "/api/runs/:id/source"),
    ("GET", "/api/runs/:id/usage"),
    ("GET", "/api/runs/:id/usage-timeline"),
    ("GET", "/api/runs/agents"),
    ("GET", "/api/runs/archived"),
    ("GET", "/api/runs/autoflows"),
    ("GET", "/api/runs/autoflows/events"),
    ("GET", "/api/runs/autoflows/problems"),
    ("GET", "/api/runs/autoflows/runs"),
    ("GET", "/api/sessions"),
    ("DELETE", "/api/sessions/:id"),
    ("GET", "/api/sessions/:id"),
    ("POST", "/api/sessions/:id/archive"),
    ("GET", "/api/sessions/:id/messages/stream"),
    ("POST", "/api/sessions/:id/restore"),
    ("GET", "/api/sessions/:id/runs"),
    ("GET", "/api/sessions/:id/usage-timeline"),
    ("GET", "/api/skills"),
    ("GET", "/api/skills/:layer/:name"),
    ("GET", "/api/tools"),
    ("GET", "/api/transcript"),
    ("DELETE", "/api/transcripts/:id"),
    ("POST", "/api/transcripts/:id/archive"),
    ("GET", "/api/usage"),
    ("GET", "/api/usage/outliers"),
    ("GET", "/api/usage/runs"),
    ("GET", "/api/usage/timeline"),
    ("GET", "/api/workers"),
    ("POST", "/api/workflows"),
    ("DELETE", "/api/workflows/:name"),
    ("GET", "/api/workflows/:name"),
    ("PUT", "/api/workflows/:name"),
    ("POST", "/api/workflows/generate"),
    ("POST", "/api/workflows/validate"),
];
