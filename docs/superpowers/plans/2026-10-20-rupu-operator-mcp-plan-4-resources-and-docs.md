# Operator MCP, Plan 4: resources, prompts and the docs rewrite

> **For agentic workers:** use superpowers:test-driven-development. Part 1 (resources and prompts) can run alongside Plan 3a. Part 2 (docs) starts after Plan 3a's packages have merged, because it documents their tools.

**Goal.** An agent can learn rupu from the MCP alone (spec goal 4). `/mcp` serves read-only resources (docs, schemas, query registries, profiles, the tool catalog, examples) and four prompts. After that, the docs are rewritten for the HTTP MCP.

**Spec.** `docs/superpowers/specs/2026-10-19-rupu-operator-mcp-design.md`, M9 and "Docs". Plan 2's Global Constraints apply here too.

## Part 1: resources and prompts

### Protocol (`crates/rupu-mcp`)

- `initialize` advertises `capabilities.resources: {}` and `capabilities.prompts: {}`, but only when the server has a resource or prompt source. A server built without one advertises nothing new, so the existing tests keep passing.
- **Methods:**
  - `resources/list`, cursor-paged
  - `resources/templates/list`, for the parameterized URIs
  - `resources/read {uri}`, which returns `contents: [{uri, mimeType, text}]`
  - `prompts/list`
  - `prompts/get {name, arguments?}`
- Only the shapes common to protocol versions 2025-03-26 and later are used. An unknown URI is the resources spec's `-32002` with `data: {uri}`; an unknown prompt or a bad argument is `-32602` naming it.
- A `ResourceSource` / `PromptSource` trait sits next to the existing tool `Source`, so `rupu-mcp` stays free of `rupu-cp` knowledge. The CP provides the implementations.

### Resources (`crates/rupu-cp/src/mcp/resources.rs`)

| URI | Content | MIME |
|---|---|---|
| `rupu://docs/<name>` | The docs embedded at build time with `include_str!`, so an agent sees the docs of the binary it talks to: `using-rupu`, `agent-format`, `agent-authoring`, `workflow-format`, `workflow-authoring`, `agentiflows`, `triggers`, `engagement-profiles`, `coverage`, `messaging`, `skills`, `transcript-schema`, `cp-api`, `control-plane`, `response-outcomes`, `netflow`, `configuration`, `mcp`. Include only the names that exist under `docs/`; a test fails if a listed doc is missing. | `text/markdown` |
| `rupu://schema/finding-report` | `rupu_coverage::report::canonical_schema()` | `application/schema+json` |
| `rupu://query/<list>` | The field registry of each list query: findings, workflow_runs, agent_runs, sessions, autoflow_runs. Use the same serialization as the `tests/fixtures/list_query/*.fields.json` pins. | `application/json` |
| `rupu://profiles/<id>` | Each built-in engagement profile, as its TOML | `application/toml` |
| `rupu://tools` | `ToolCatalog::all()`, the same rows as `GET /api/tools` | `application/json` |
| `rupu://examples/<path>` | The repo's `examples/` agents, workflows and contracts, embedded at build time. A `build.rs` or `include_dir`-style list is the builder's call. A workspace dep is needed if a crate is used; check the root `Cargo.toml` first. | by extension |

Every resource is read-only and served from the binary or the registries. None is read from the user's `RUPU_HOME`.

### Prompts

The prompts are `author-agent`, `author-workflow`, `author-agentiflow` and `triage-findings`. Each one is:

- a short instruction, of a few sentences;
- plus `resource` links to the matching docs and schemas;
- plus a `project` argument for `triage-findings`, which names the `findings.list` query to start from.

The links are deliberately `rupu://…` lines in the message text, not embedded `resource` or `resource_link` content. Embedding would put the full docs (5–73 KB each) into every prompt. `resource_link` content arrived in 2025-06-18, and the HTTP server keeps no per-session protocol version to gate it on.

### Tests (`crates/rupu-cp/tests/it/mcp_resources.rs`)

- `resources/list` names every URI above, and each one reads back non-empty with its MIME type.
- `rupu://docs/cp-api` equals `docs/cp-api.md` byte for byte.
- An unknown URI gets `-32002` with `data: {uri}`.
- The prompts list, and `prompts/get` for each, returns resource links that resolve.
- `initialize` advertises `resources` and `prompts` on `/mcp`.

## Part 2: the docs rewrite (after Plan 3a)

- **`docs/mcp.md`** is rewritten. It covers:
  - the modes (`cp serve`, `--no-web`, `--no-mcp`)
  - wiring for Claude Code (`claude mcp add --transport http …`), Cursor, and Claude Desktop (connector URL or `mcp-remote`)
  - the tool areas, generated from or checked against `tool_names.json`
  - `wait`, resources and prompts
  - the full-access stance
  - the `rupu mcp serve` deprecation
- **`docs/control-plane.md`** covers the serve modes.
- **`docs/cp-api.md`** is checked so that every Plan 3a endpoint is present.
- **`CLAUDE.md`**: update the `rupu-mcp` and `rupu-cp` entries, and add Plan 3a, 3b and 4 to "Read first".
- **`CHANGELOG.md`**: one entry.
