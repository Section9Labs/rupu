# Configured MCP tools for sessions

`rupu-mcp-client` implements a bounded outbound HTTP tools client. The CLI freezes configured tools at session creation, and the worker verifies and binds them before provider preparation. Supported control-plane starters advertise `session.mcp_tools.v1`. The existing `/mcp` endpoint remains the separate operator server. This client does not attach that endpoint or reuse its bearer token. MCP discovery and session-selection routes are intentionally operator HTTP controls, excluded from the operator MCP tool inventory; they add no implicit session discovery or self-grant tool. The existing operator tool count is unchanged.

## Host configuration

The explicit loader `rupu_config::mcp::Registry::load(global_root)` reads only `global_root/mcp.toml`. This file is outside the ordinary global/customer/project configuration merge. A missing file produces an empty registry; an invalid existing file returns an error. Loading the registry performs no network operation or credential resolution.

```toml
version = 1

[servers.example]
label = "Example tools"
url = "https://example.invalid/mcp"
enabled = true
bearer_env = "RUPU_MCP_EXAMPLE_TOKEN"

[servers.example.tool_effects]
search = "read"
update = "external"
```

Authentication is optional. When present, `bearer_env` must name a dedicated `RUPU_MCP_` variable; the explicit connect operation resolves only that reference. No provider credential file, CP token lookup, project override, process command or inline token field is supported. Endpoints must use HTTPS or literal loopback HTTP, without user information, query parameters or fragments. The client disables redirects, inherited proxies and transport retries. Config and client Debug output omit endpoints and authorization values. Its HTTP client uses Rupu's required netflow factory with an explicit capture context and sink; the origin identifies the configured MCP server. Construct a separate client for each run owner rather than caching the first run's sink. Netflow records omit headers and payloads.

Servers default to disabled. Unmapped tools default to the local `external` effect, irrespective of remote annotations. Exact host policy may classify a tool as `read`, `write` or `external`; `record` and `spawn` are reserved for Rupu-owned bookkeeping and child-ceiling implementations. A local Read policy is the host operator's trust assertion about a remote tool, not proof of remote behavior or workspace isolation.

New supported sessions use enabled trusted servers as their default MCP preset, freezing exact discovered tool identities and fingerprints. Existing sessions keep their saved selection; records predating MCP retain none. An explicit empty selection disables MCP. A changed endpoint, auth reference, selected tool metadata or host effect policy makes the frozen selection stale; new tools never silently expand an existing session.

## Protocol and ownership

`HttpToolsClient::connect` selects an enabled configured server. `discover` returns immutable tool handles holding the exact configured server ID, remote tool name, local effect policy and metadata. `call` accepts only a handle from that initialized client; another client cannot substitute a same-named tool. The runtime adapter adds only selected exact names to the turn grant after builtin settings overrides. The existing runner applies its permission policy before invocation. The binding also checks the exact run, workspace ID/path and session surface, refusing reuse by another owner or child. The worker attempts bounded session teardown after the turn, including provider/preparation failures.

The implemented profile negotiates version `2025-11-25`, then sends the initialized notification. It advertises no sampling, elicitation, roots or task execution capability. Unsupported versions, missing tools capability, and task-required tools fail explicitly. Server-provided instructions are not promoted to system instructions. See the [MCP lifecycle specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle).

The client handles JSON or SSE responses, request correlation, session and protocol headers, and bounded tool-list pagination. It preserves tool execution errors, structured results and content blocks. Returned resource links remain data; the client never fetches them. Server requests can receive a ping response or an unsupported-method error, and cannot invoke local providers or tools. See the [MCP transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) and [tools specifications](https://modelcontextprotocol.io/specification/2025-11-25/server/tools).

A tool-list change invalidates later calls on that connection. An expired session requires explicit reconnect. Caller cancellation after an invocation begins, or a disconnected, timed-out, oversized or malformed call response, is an uncertain outcome: the client does not replay it and blocks subsequent calls on the same connection. Cancellation is best effort and does not establish rollback. An explicit close attempts bounded session teardown. SSE stream resumption, legacy HTTP+SSE, stdio, OAuth enrollment, prompts, resources, sampling, elicitation and tasks are outside this profile.

The client refuses the known `rupu`/`rupu-cp` operator server identity at initialization to catch accidental attachment. This is an integration guard, not authentication of an untrusted server's claimed identity. Rupu operator capabilities require separate scoped runtime adapters.

## Bounds and fixtures

Configuration is limited to 256 KiB and 32 servers. Discovery permits at most 256 tools and 32 pages, with a 2 MiB aggregate metadata budget. Each response is capped at 2 MiB, each tool definition at 256 KiB, descriptions at 16 KiB and raw names at 256 bytes. Metadata and call deadlines cannot exceed 120 seconds; defaults are 30 and 120 seconds. No progress message extends the deadline.

Tests use disposable loopback fixture servers and synthetic credentials. They cover HTTP request order and headers, JSON/SSE framing, separate connection ownership, pagination, local policy, errors, uncertainty, cancellation, redirects, resource-link preservation, operator refusal and default configuration behavior. The runtime and CLI fixtures also exercise frozen defaults, explicit empty, shared revision conflicts, stale metadata before provider preparation, and real worker grant/policy/dispatch with a mock provider. These tests do not establish live interoperability with configured external servers.

## Control-plane contract

All routes require a direct connection to the owning local host (`host=local` or omitted). The normal control-plane authentication and writable-deployment checks apply. No operator-server attachment or child catalog inheritance is introduced.

- `GET /api/mcp/tools`: `{host_id, revision, tools, default:"enabled_host_servers", permission:"host_effects_and_session_mode"}`. Each tool contains `server`, friendly `server_label`, exact remote `tool`, provider-safe `name`, `description`, trusted local `effect`, and opaque `fingerprint`. The revision/fingerprint are opaque SHA-256 strings; labels and remote names belong in the UI. Discovery never calls a tool. Endpoints, credential references and credential values are not returned.
- `POST /api/projects/:ws_id/sessions`: additive `mcp_tools` (array of `{server,tool}`) and `mcp_catalog_revision`. Omitted/null tools selects the current enabled host preset once; `[]` disables without registry/credential/discovery work; a nonempty explicit selection requires the catalog revision. The existing single creation POST and receipt remain unchanged. The CLI freezes before writing a session or spawning a worker. A stale selection returns 409, including a stale check inside the CLI adapter (exit 75). After successful creation, the session MCP GET exposes its actual frozen selection.
- `GET /api/sessions/:id/mcp`: `{session_id, host_id, workspace_id, revision, tools, defaults, effective, permission_mode, applies:"next_turn"}`. Arrays contain exact `{server,tool}` pairs. `tools:null` inherits the frozen creation selection in `defaults`; it never rediscovers host defaults. `[]` explicitly disables. `effective` is the resolved selection.
- `PUT /api/sessions/:id/mcp`: `{revision, tools, catalog_revision?}`. `tools` must be present: null resets to the frozen creation selection, empty disables, and a nonempty list freezes current metadata only with a matching catalog revision. This shares the existing settings revision/CAS. A builtin-settings PUT preserves the MCP override even when an older editor omits it. Edits apply next turn and can be made while a turn is running; the active turn retains its captured settings/catalog.

Wrong host or unsupported starter returns 501; invalid selection returns 400 (malformed JSON extraction can return 422); a stale catalog/settings revision or archived session returns 409; registry/discovery failure on the discovery/edit endpoints returns 502. The existing CLI creation adapter reports non-stale preflight failures as 400; stale creation remains 409. Requests never retry a tool invocation. Native editors should retain drafts on conflict or uncertain transport outcomes and refresh explicitly.

A version-5 session record preserves the creation snapshot. Older workers refuse that record rather than erasing it. Next-turn overrides use `settings.json` with its existing atomic replacement and shared revision lock; old settings readers reject unknown fields rather than silently discarding MCP state. New sessions freeze at most 256 remote tools across at most 32 configured servers; total discovery has a 30-second deadline. General operator MCP features require scoped runtime adapters and remain outside this transport.

## Trusted-host server settings

`mcp.server_configuration.v1` advertises the host registry settings API. Read-only deployments can inspect it; their receipts have `writable:false`, and mutations return 501. Every route is local-only and uses the normal CP authentication. Registry writes do not connect to a server, resolve an environment value, discover tools, or affect a running client's captured configuration.

`GET /api/mcp/servers?host=local` returns:

```json
{
  "host_id": "local",
  "scope": "trusted_host",
  "revision": "opaque SHA-256 revision",
  "writable": true,
  "servers": [{
    "id": "example",
    "label": "Example tools",
    "url": "https://example.invalid/mcp",
    "enabled": true,
    "credential": {"kind": "bearer_env", "configured": true},
    "tool_effects": {"search": "read"},
    "status": "configured_unverified"
  }],
  "applies": "new_sessions_and_next_turn_validation"
}
```

The credential reference and its value are never returned. `configured` means a reference exists, not that its environment value is present or that authentication worked. Secure credential enrollment is outside this profile; the supported authentication choices are no authentication or a dedicated host-provisioned `RUPU_MCP_*` environment reference. Native Settings must not substitute an app-local token or Keychain item for the owning CP process's environment.

`PUT /api/mcp/servers/:id` requires `revision`, `label`, `url`, `enabled`, `tool_effects`, and one explicit `credential` edit: `{"action":"preserve"}`, `{"action":"clear"}`, or `{"action":"set_reference","reference":"RUPU_MCP_EXAMPLE_TOKEN"}`. The identifier is an environment-variable name, never a token. This is an upsert; an Add UI should reject duplicate IDs from its captured inventory, with the revision protecting against a concurrent addition. `DELETE` on the same route requires a JSON body `{"revision":"..."}`.

Mutation success returns the canonical inventory plus `mutation:{action:"upsert"|"delete",server_id:"..."}`. Revisions are opaque hashes of exact file bytes with a missing/present marker, not counters. A semantically unchanged save can retain its revision. After a lost response, refresh can reconcile visible fields, but `configured:true` cannot prove that a particular replacement reference was saved; retain that uncertainty rather than retrying automatically.

Writes validate the complete typed registry under a registry-specific advisory lock, then sync a unique temporary sibling and rename it atomically. Stale or busy revisions and invalid existing files return 409; unknown input and invalid new configuration return 400; deleting a missing ID returns 404. Unknown existing fields are refused, never silently discarded. Malformed mutation JSON gets a generic error without echoing submitted values. Direct non-file/symlink targets are refused; this is static protection in a trusted local root, not descriptor-anchored protection against hostile filesystem races. Configuring a server or effect changes future new-session defaults; an existing frozen selection must be explicitly reselected if its captured configuration changes.

Registry mutation JSON bodies are bounded to 256 KiB; the persisted registry is
also bounded to 256 KiB, with at most 32 servers and 256 exact effect entries per
server. Invalid or oversized mutation bodies return a sanitized 400 response.

Registry inventory and mutations are intentionally operator HTTP controls,
excluded from the operator MCP tool inventory. They do not expose endpoint
configuration, effect-policy edits, or server removal as session-callable tools.

Host credential enrollment preparation adds an optional `bearer_store` binding
generation, mutually exclusive with `bearer_env`. The generation is opaque
identity metadata, omitted from old configuration and from registry inventory.
`credential.action = "use_host_store"` selects this mechanism; `preserve` retains
its generation only for the same canonical endpoint. Endpoint changes, mechanism
replacement, and delete/recreate produce a fresh generation. Clearing or deleting
registry configuration does not silently delete stored credentials. These changes
participate in frozen tool fingerprints. The schema preparation alone advertises
no credential-enrollment capability and refuses host-store transport until the
dedicated identity-aware credential resolver is installed.
