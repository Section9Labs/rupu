//! The operator tool table: each tool is one CP API route, described by its
//! method, path template and the handler's own query/body structs (their
//! `JsonSchema` is the tool's input schema). Each area file lists its tools
//! with [`op`]; [`ops`] is all of them.

mod agentiflows;
mod definitions;
mod findings;
mod fleet;
mod observe;
mod projects;
mod runs;
mod sessions;

use rupu_mcp::ToolHints;

type SchemaFn = fn() -> Option<schemars::schema::RootSchema>;

/// One operator tool.
#[derive(Clone)]
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
    /// The 2xx body is a file (an export, an artifact): returned as an
    /// embedded resource rather than parsed.
    pub bytes: bool,
}

fn none() -> Option<schemars::schema::RootSchema> {
    None
}

fn schema<T: schemars::JsonSchema>() -> Option<schemars::schema::RootSchema> {
    Some(schemars::schema_for!(T))
}

/// A tool for `method path`, with no query or body and the method's hints:
/// `GET` read-only and idempotent, `PUT` idempotent, `DELETE` destructive,
/// anything else none. `description`'s first line says what the tool is for
/// on its own, ends with a period and is at most 120 characters; its second
/// paragraph names the arguments that matter and what the tool returns.
pub(crate) fn op(
    name: &'static str,
    method: http::Method,
    path: &'static str,
    description: &'static str,
) -> OpSpec {
    let hints = ToolHints {
        read_only: method == http::Method::GET,
        destructive: method == http::Method::DELETE,
        idempotent: method == http::Method::GET || method == http::Method::PUT,
        open_world: false,
    };
    OpSpec {
        name,
        method,
        path,
        description,
        query: none,
        body: none,
        hints,
        bytes: false,
    }
}

impl OpSpec {
    /// The handler's query struct.
    pub(crate) fn query<T: schemars::JsonSchema>(mut self) -> Self {
        self.query = schema::<T>;
        self
    }

    /// The handler's JSON body struct.
    pub(crate) fn body<T: schemars::JsonSchema>(mut self) -> Self {
        self.body = schema::<T>;
        self
    }

    /// The call can destroy or overwrite something.
    pub(crate) fn destructive(mut self) -> Self {
        self.hints.destructive = true;
        self
    }

    /// The call reaches outside the CP (launches a model run, contacts a host).
    pub(crate) fn open_world(mut self) -> Self {
        self.hints.open_world = true;
        self
    }

    /// A `POST` that changes nothing (validate, preview, export).
    // First used by the area tools that follow Plan 2's Task 4.
    #[allow(dead_code)]
    pub(crate) fn read_only(mut self) -> Self {
        self.hints.read_only = true;
        self
    }

    /// The 2xx body is returned as an embedded resource.
    // First used by the area tools that follow Plan 2's Task 4.
    #[allow(dead_code)]
    pub(crate) fn bytes(mut self) -> Self {
        self.bytes = true;
        self
    }
}

/// Every operator tool, in listing order: the area files' tools in the
/// order runs, sessions, agentiflows, definitions, findings, observe,
/// fleet, projects.
pub fn ops() -> &'static [OpSpec] {
    static OPS: std::sync::OnceLock<Vec<OpSpec>> = std::sync::OnceLock::new();
    OPS.get_or_init(|| {
        [
            runs::ops(),
            sessions::ops(),
            agentiflows::ops(),
            definitions::ops(),
            findings::ops(),
            observe::ops(),
            fleet::ops(),
            projects::ops(),
        ]
        .into_iter()
        .flatten()
        .collect()
    })
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
    (
        "GET",
        "/api/runs/:id/messages/stream",
        "SSE stream; wait replaces it",
    ),
    (
        "GET",
        "/api/sessions/:id/messages/stream",
        "SSE stream; wait replaces it",
    ),
    (
        "GET",
        "/api/agentiflows/:id/messages/stream",
        "SSE stream; wait replaces it",
    ),
    ("POST", "/mcp", "the MCP endpoint itself"),
];

/// Routes not yet operator tools; Plan 2 empties this list.
pub const PENDING_PLAN_2: &[(&str, &str)] = &[
    ("GET", "/api/agentiflows"),
    ("GET", "/api/agentiflows/:id"),
    ("GET", "/api/agentiflows/:id/children"),
    ("GET", "/api/agentiflows/:id/events"),
    ("POST", "/api/agentiflows/:id/steer"),
    ("GET", "/api/agentiflows/:id/units/:unit_id/output"),
    ("GET", "/api/agentiflows/:id/netflow"),
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
    ("GET", "/api/runs/:id/netflow"),
    ("GET", "/api/skills"),
    ("GET", "/api/skills/:layer/:name"),
    ("GET", "/api/tools"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use http::Method;

    fn hints(o: OpSpec) -> (bool, bool, bool, bool) {
        let h = o.hints;
        (h.read_only, h.destructive, h.idempotent, h.open_world)
    }

    #[test]
    fn hints_default_from_the_method_and_the_builder_overrides_them() {
        let o = |m| op("t.x", m, "/t", "T.");
        assert_eq!(hints(o(Method::GET)), (true, false, true, false));
        assert_eq!(hints(o(Method::PUT)), (false, false, true, false));
        assert_eq!(hints(o(Method::DELETE)), (false, true, false, false));
        assert_eq!(hints(o(Method::POST)), (false, false, false, false));
        assert_eq!(hints(o(Method::PATCH)), (false, false, false, false));
        assert_eq!(
            hints(o(Method::POST).read_only()),
            (true, false, false, false)
        );
        assert_eq!(
            hints(o(Method::POST).open_world()),
            (false, false, false, true)
        );
        assert_eq!(
            hints(o(Method::PUT).destructive()),
            (false, true, true, false)
        );
        let plain = o(Method::GET);
        assert!(!plain.bytes && (plain.query)().is_none() && (plain.body)().is_none());
        assert!(o(Method::GET).bytes().bytes);
    }
}
