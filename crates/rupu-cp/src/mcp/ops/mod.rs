//! The operator tool table: each tool is one CP API route, described by its
//! method, path template and the handler's own query/body structs (their
//! `JsonSchema` is the tool's input schema). Each area file lists its tools
//! with [`op`]; [`ops`] is all of them.

mod agentiflows;
mod definitions;
mod findings;
mod fleet;
mod observe;
mod ops_admin;
mod projects;
mod query;
mod runs;
mod scm;
mod sessions;
mod setup;

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
/// fleet, projects, query, scm, setup, ops_admin.
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
            setup::ops(),
            ops_admin::ops(),
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
    ("POST", "/api/workflows/validate/projection", "operator stateless authoring projection; no execution dependency or model grant"),
    ("POST", "/api/runs/agents/:id/continue-options", "operator HTTP explicit continuation list review; no additional model grant or legacy fallback"),
    ("GET", "/api/transcripts/:id/delete-context", "operator exact source deletion review; no additional model mutation grant"),
    ("DELETE", "/api/transcripts/:id/exact-source", "operator exact source deletion review; no additional model mutation grant"),
    (
        "GET",
        "/api/runs/:id/board/operator",
        "operator Board inspection and guarded actions are HTTP-only; no MCP tool is exposed",
    ),
    (
        "POST",
        "/api/runs/:id/board/operator",
        "operator Board inspection and guarded actions are HTTP-only; no MCP tool is exposed",
    ),
    (
        "GET",
        "/api/sessions/:id/board/operator",
        "operator Board inspection and guarded actions are HTTP-only; no MCP tool is exposed",
    ),
    (
        "POST",
        "/api/sessions/:id/board/operator",
        "operator Board inspection and guarded actions are HTTP-only; no MCP tool is exposed",
    ),
    (
        "GET",
        "/api/agentiflows/:id/board/operator",
        "operator Board inspection and guarded actions are HTTP-only; no MCP tool is exposed",
    ),
    (
        "POST",
        "/api/agentiflows/:id/board/operator",
        "operator Board inspection and guarded actions are HTTP-only; no MCP tool is exposed",
    ),
    ("GET", "/api/runs/:id/observation/transcript", "operator verified bounded transcript windows; not a session tool grant"),
    ("GET", "/api/runs/:id/observation/transcript/children", "operator verified bounded transcript windows; not a session tool grant"),
    ("GET", "/api/runs/:id/observation/transcript/descendant", "operator verified bounded transcript windows; not a session tool grant"),
    ("GET", "/api/runs/:id/observation/frontier", "operator exact-owner metadata frontier; not a session tool grant"),
    ("GET", "/api/runs/:id/observation", "operator bounded workflow capture; no added session tool grant"),
    ("GET", "/api/agent-files/conversation-review", "operator exact saved global conversation review; no model grant"),
    ("POST", "/api/agent-files/conversation", "operator exact persistent conversation creation; no model tool or remote proxy"),
    ("GET", "/api/agent-files/run-review", "operator exact global definition and execution project review; no model grant"),
    ("POST", "/api/agent-files/run", "operator exact global run dispatch; no model tool or remote proxy"),
    ("GET", "/api/runs/:id/process-log", "operator retained process diagnostics; no session tool or runtime grant"),
    ("GET", "/api/runs/:id/observation/page", "operator immutable workflow page; no added session tool grant"),
    ("POST", "/api/findings/import/preview", "operator-only immutable cross-project report review"),
    ("POST", "/api/findings/import/commit", "operator-only reviewed report commit; not a model grant"),
    ("GET", "/api/findings/import/operations/:id", "operator-only durable report import receipt"),
    ("GET", "/api/sessions/:id/approvals", "authenticated operator per-call approval inventory; never a session self-grant tool"),
    ("GET", "/api/sessions/:id/approvals/:approval", "authenticated operator exact tool approval review"),
    ("PUT", "/api/sessions/:id/approvals/:approval/decision", "authenticated operator decision; never model-callable"),
    ("POST", "/api/sessions/:id/approvals/:approval/revoke", "authenticated operator approval revocation"),
    ("POST", "/api/workflows/draft/preview", "operator HTTP draft preview; no new session tool grant"),
    ("GET", "/api/runs/:id/workflow-source", "operator HTTP historical source inspection; no additional session tool grant"),
    (
        "GET",
        "/api/mcp/servers/:id/credential",
        "authenticated operator HTTP credential management; never an agent tool",
    ),
    (
        "PUT",
        "/api/mcp/servers/:id/credential",
        "authenticated operator HTTP credential management; never an agent tool",
    ),
    (
        "DELETE",
        "/api/mcp/servers/:id/credential",
        "authenticated operator HTTP credential management; never an agent tool",
    ),
    (
        "GET",
        "/api/mcp/credential-operations/:id",
        "authenticated operator HTTP credential management; never an agent tool",
    ),
    (
        "GET",
        "/api/mcp/credential-orphans",
        "authenticated operator HTTP credential management; never an agent tool",
    ),
    (
        "DELETE",
        "/api/mcp/credential-orphans/:generation",
        "authenticated operator HTTP credential management; never an agent tool",
    ),
    ("GET", "/api/projects/:ws_id/scm/reviews/:number/pages", "HTTP-only bounded native review continuation; no agent tool"),
    (
        "GET",
        "/api/projects/:ws_id/scm/reviews/:number",
        "HTTP-only native review inspector with captured checkout/account/head expectations; runtime SCM tools remain separate",
    ),
    ("GET", "/api/issues/:tracker/discovery", "HTTP-only bounded current issue discovery under exact selected account/project; no new model grant"),
    (
        "GET",
        "/api/issues/accounts",
        "HTTP-only current issue inspector with explicit owning account/origin and bounded continuation; runtime issue tools remain separate",
    ),
    (
        "GET",
        "/api/issues/:tracker/:number",
        "HTTP-only current issue inspector with explicit owning account/origin and bounded continuation; runtime issue tools remain separate",
    ),
    (
        "GET",
        "/api/issues/:tracker/:number/body",
        "HTTP-only current issue inspector with explicit owning account/origin and bounded continuation; runtime issue tools remain separate",
    ),
    (
        "GET",
        "/api/issues/:tracker/:number/comments",
        "HTTP-only current issue inspector with explicit owning account/origin and bounded continuation; runtime issue tools remain separate",
    ),
    (
        "GET",
        "/api/forge/write-capabilities",
        "operator-only reviewed forge intent and durable receipt; never a model write grant or automatic retry",
    ),
    (
        "POST",
        "/api/forge/intents",
        "operator-only reviewed forge intent and durable receipt; never a model write grant or automatic retry",
    ),
    (
        "GET",
        "/api/forge/intents/:id",
        "operator-only reviewed forge intent and durable receipt; never a model write grant or automatic retry",
    ),
    (
        "POST",
        "/api/forge/intents/:id/execute",
        "operator-only reviewed forge intent and durable receipt; never a model write grant or automatic retry",
    ),
    (
        "GET",
        "/api/forge/operations/:id",
        "operator-only reviewed forge intent and durable receipt; never a model write grant or automatic retry",
    ),
    ("GET", "/api/config/scoped/project/:id", "versioned operator UI config CAS; existing config tools retain their legacy contract"),
    ("PUT", "/api/config/scoped/project/:id", "versioned operator UI config CAS; existing config tools retain their legacy contract"),
    ("GET", "/api/config/scoped/customer/:slug", "versioned operator UI config CAS; existing config tools retain their legacy contract"),
    ("PUT", "/api/config/scoped/customer/:slug", "versioned operator UI config CAS; existing config tools retain their legacy contract"),
    (
        "GET",
        "/api/mcp/servers",
        "operator HTTP inventory of trusted host MCP server configuration",
    ),
    (
        "PUT",
        "/api/mcp/servers/:id",
        "operator-only host MCP endpoint and effect-policy configuration",
    ),
    (
        "DELETE",
        "/api/mcp/servers/:id",
        "operator-only removal of trusted host MCP server configuration",
    ),
    (
        "GET",
        "/api/mcp/tools",
        "operator HTTP discovery of trusted host tools; not an implicit session tool catalog",
    ),
    (
        "GET",
        "/api/sessions/:id/mcp",
        "operator HTTP view of frozen session MCP selections",
    ),
    (
        "PUT",
        "/api/sessions/:id/mcp",
        "operator-only revisioned MCP grant selection; not a self-grant tool",
    ),
    (
        "GET",
        "/api/sessions/:id/controls",
        "interactive session command discovery for the attach client",
    ),
    (
        "POST",
        "/api/sessions/:id/commands",
        "interactive attach commands; operator tools use sessions.stop and sessions.compact",
    ),
    (
        "GET",
        "/api/sessions/:id/settings",
        "session-local model and tool-grant editor for the operator UI",
    ),
    (
        "PUT",
        "/api/sessions/:id/settings",
        "operator-only session model and tool-grant configuration",
    ),
    (
        "GET",
        "/api/sessions/:id/orchestration",
        "session-local orchestration editor and effective tool inventory for the operator UI",
    ),
    (
        "PUT",
        "/api/sessions/:id/orchestration",
        "operator-only session orchestration configuration",
    ),
    ("GET", "/api/runs/:id/log", "SSE stream"),
    ("GET", "/api/events/stream", "SSE stream"),
    (
        "POST",
        "/api/events/scan",
        "bounded exact-run event scanning for the native operator inspector; no model tool grant",
    ),
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
