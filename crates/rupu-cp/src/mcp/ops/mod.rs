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
mod query;
mod runs;
mod scm;
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

    /// A `POST` that changes nothing (validate, preview, export); repeating
    /// it changes nothing either, so it is also idempotent.
    pub(crate) fn read_only(mut self) -> Self {
        self.hints.read_only = true;
        self.hints.idempotent = true;
        self
    }

    /// The 2xx body is returned as an embedded resource.
    pub(crate) fn bytes(mut self) -> Self {
        self.bytes = true;
        self
    }
}

/// Every operator tool, in listing order: the area files' tools in the
/// order runs, sessions, agentiflows, definitions, findings, observe,
/// fleet, projects, query, scm.
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
            query::ops(),
            scm::ops(),
        ]
        .into_iter()
        .flatten()
        .collect()
    })
}

/// Routes that are deliberately not operator tools, each with the reason.
/// `tests/it/mcp_routes.rs` holds this list and `ops()` in lockstep with the
/// registered routes.
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
            (true, false, true, false)
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
