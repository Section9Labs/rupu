# rupu's MCP server

`rupu cp serve` serves an [MCP](https://modelcontextprotocol.io/) endpoint at
`/mcp` (streamable HTTP) beside the web UI and the `/api`. An MCP client such
as Claude Code, Cursor or Claude Desktop connects to it and **operates rupu**:
list and launch workflows and agents, watch runs, approve gates, drive
sessions and message a live run. It also gets rupu's tool catalog (files,
shell, SCM, issues, findings) as plain tools.

The endpoint is an adapter, not a second implementation: the operator tools
call the control plane's own endpoints in process and return the same JSON as
the CP API ([cp-api.md](cp-api.md)), and the catalog tools are the same tool
bodies agents and workflow `action:` steps call.

## Full access

The MCP gives a connected client the same reach as the CP's operator. rupu adds
**no permission layer of its own** on `/mcp`: it does not prompt, and it has no
`--mode`. What runs is decided by the MCP client's harness (Claude Code's
permission prompts, `--allowedTools`, Cursor's approvals). Each tool carries
read-only / destructive / open-world hints so a client's permission UI can tell
`runs.list` from `runs.cancel`. Treat the bearer token like a key to the whole
control plane, and narrow what is served with [`--tools`](#choosing-the-tools---tools)
if you want less.

## Serve modes

| Command | Serves |
|---|---|
| `rupu cp serve` | web UI + `/api` + `/mcp` |
| `rupu cp serve --no-web` | `/api` + `/mcp`, no web UI (implies `--no-open`) |
| `rupu cp serve --no-mcp` | web UI + `/api`, no `/mcp` |

`--no-web --no-mcp` is refused (nothing to serve), and so is `--tools` with
`--no-mcp`. A bad `--tools` grant fails before the socket is bound. At startup
the server prints the MCP URL and the command that connects Claude Code to it.

`/mcp` sits behind the same bearer token as `/api` (`--token`), and the server
checks two things on every request:

- the `Origin` header, when present, must equal the `Host`;
- on a loopback bind, the `Host` must be a loopback name (`127.0.0.1`,
  `localhost`, `[::1]`), which closes DNS-rebinding.

Binding beyond loopback is allowed; do it only with `--token`
([control-plane.md](control-plane.md#authentication)).

## Connecting a client

### Claude Code

```sh
# no token (loopback, default bind)
claude mcp add --transport http rupu http://127.0.0.1:7878/mcp

# with a token
claude mcp add --transport http rupu http://127.0.0.1:7878/mcp \
  --header "Authorization: Bearer $CP_TOKEN"
```

`claude mcp add --scope user` makes it available in every project. Claude Code
names each tool `mcp__rupu__<tool>` with dots turned to underscores
(`runs.list` is `mcp__rupu__runs_list`). `claude mcp list` shows whether it
connected.

### Cursor

`~/.cursor/mcp.json` (or the project's `.cursor/mcp.json`):

```json
{
  "mcpServers": {
    "rupu": {
      "url": "http://127.0.0.1:7878/mcp",
      "headers": { "Authorization": "Bearer <token>" }
    }
  }
}
```

Leave `headers` out when the server runs without a token.

### Claude Desktop

Add the URL as a custom connector if your plan offers remote connectors that
can reach it. Otherwise bridge it with
[`mcp-remote`](https://www.npmjs.com/package/mcp-remote) in
`claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "rupu": {
      "command": "npx",
      "args": ["-y", "mcp-remote", "http://127.0.0.1:7878/mcp",
               "--header", "Authorization: Bearer <token>"]
    }
  }
}
```

Authentication to GitHub, GitLab and the model providers is shared with the
CLI through the same credential store, so `rupu auth login` once unlocks the
catalog tools for everything.

## Operator tools

185 tools over the CP's own endpoints, one per route (`GET /api/tools` and
`tools/list` give each tool's full input schema). Each takes the arguments of
the endpoint it wraps (path parameters, the handler's query struct, the JSON
body), an optional `host` where the API takes `?host=`, refuses unknown
arguments, and returns the API's JSON as text. A JSON object is also sent as
`structuredContent`; a list (a JSON array) is sent as text only, since MCP
allows only an object there. Names are `<area>.<verb>`; hints follow the HTTP
method (GET read-only and idempotent, PUT idempotent, DELETE destructive) and
gate nothing.

| Area | Tools |
|---|---|
| Workflow runs | `runs.list` `runs.list_all` `runs.archived` `runs.get` `runs.approve` `runs.reject` `runs.cancel` `runs.pause` `runs.resume` `runs.archive` `runs.restore` `runs.delete` `runs.graph` `runs.children` `runs.usage` `runs.usage_timeline` `runs.source` `runs.ast` `runs.autoflow` `runs.retry` |
| Agent runs and transcripts | `agent_runs.list` `agent_runs.continue` `transcripts.get` `transcripts.archive` `transcripts.delete` `events.recent` |
| Autoflow runs | `autoflow_runs.list` `autoflow_runs.problems` `autoflow_runs.cycles` `autoflow_runs.events` |
| Sessions and messages | `sessions.list` `sessions.get` `sessions.turns` `sessions.usage_timeline` `sessions.start` `sessions.send` `sessions.archive` `sessions.restore` `sessions.delete` `sessions.stop` `sessions.compact` `messages.list` `messages.send` |
| Agentiflows | `agentiflows.list` `agentiflows.get` `agentiflows.events` `agentiflows.children` `agentiflows.unit_output` `agentiflows.steer` `agentiflows.run` `agentiflows.stop` `agentiflow_defs.list` `agentiflow_defs.get` `agentiflow_defs.create` `agentiflow_defs.save` `agentiflow_defs.delete` `agentiflow_defs.validate` |
| Definitions | `workflows.list` `workflows.get` `workflows.create` `workflows.save` `workflows.delete` `workflows.validate` `workflows.generate` `workflows.run` `workflows.run_yaml` `agents.list` `agents.get` `agents.create` `agents.save` `agents.delete` `agents.generate` `agents.run` `agents.validate` `agents.validate_all` `generate.models` `autoflows.list` `autoflows.get` `autoflows.enable` `autoflows.disable` `autoflows.run` `autoflows.explain` `autoflows.doctor` `autoflows.wakes` `claims.list` `claims.release` `claims.requeue` `claims.repair` `skills.list` `skills.get` `skills.install` `skills.update` `skills.validate` `tools.list` `models.list` `models.refresh` |
| Findings, coverage, assets | `findings.list` `findings.get` `findings.tags` `findings.tag` `findings.export` `findings.export_many` `findings.artifact` `findings.import` `findings.schema` `coverage.list` `coverage.get` `coverage.audit` `coverage.catalog` `coverage.runs` `coverage.diff` `coverage.templates` `coverage.template` `coverage.gap` `coverage.rerun` `assets.list` `query.fields` `query.check` |
| Observability | `netflow.run` `netflow.agentiflow` `netflow.project` `netflow.global` `netflow.graph` `netflow.explorer` `netflow.index` `netflow.latest` `usage.get` `usage.timeline` `usage.runs` `usage.outliers` `dashboard.get` |
| Fleet, config, customers | `hosts.list` `hosts.registered` `hosts.add` `hosts.add_ssh` `hosts.add_bucket` `hosts.enroll_node` `hosts.remove` `host.info` `workers.list` `config.get` `config.put_global` `config.put_customer` `config.put_project` `config.put_policy` `customers.list` `customers.create` `customers.get` `customers.update` `customers.delete` `customers.archive` `customers.unarchive` `customers.assign_project` `customers.unassign_project` `launch.preview` `repos.list` `repos.tracked` `repos.attach` `repos.prefer` `repos.forget` `scm_accounts.list` `scm_rules.list` `scm_rules.bind` `auth.status` |
| Projects | `projects.list` `projects.get` `projects.runs` `projects.sessions` `projects.agents` `projects.workflows` `projects.autoflows` `projects.coverage` `projects.coverage_assessed` `projects.tree` `projects.source` `projects.files` `projects.init` |
| Setup and housekeeping | `fleet.list` `fleet.install` `engagement_profiles.list` `engagement_profiles.get` `cron.list` `cron.events` `cron.tick` `maintenance.prune` |

The tool names are pinned by `crates/rupu-cp/tests/fixtures/mcp/tool_names.json`.
Every CP route is a tool except the ones in `EXCLUDED`
(`crates/rupu-cp/src/mcp/ops/mod.rs`; a test fails when a route is neither):
the SSE streams (`wait` replaces them), the host-to-host transfer routes
(workspace sync, skill bundles, coverage, netflow and artifact pulls), the node
WebSocket, `/healthz`, the web UI's filesystem browser and `POST /mcp` itself.

### Results

- **Header notes.** Any `x-rupu-*` response header (for example
  `x-rupu-hosts-unfilterable`, naming hosts a fan-out left out) comes back as
  an extra text note after the JSON and in the result's
  `_meta["rupu/headers"]`.
- **Files.** `findings.export`, `findings.export_many` and `findings.artifact`
  return an **embedded resource**: `text` for text types, a base64 `blob`
  otherwise, `uri` `rupu://api/...` (the API path and query, `rupu://api` being
  the API root) and the filename in `resource._meta.name`.
- **Large bodies.** A response over 16 MiB is an error that names the API path
  to fetch with the token instead.
- **Route guard.** A path argument that would reach a sibling route (an id
  spelled `workflows` for `/api/runs/:id`, which is `/api/runs/workflows`) is
  refused with a 400 before the handler runs.
- **Host.** The `/mcp` request's `Host` is forwarded to the route, so a handler
  that derives an address from it (`hosts.enroll_node`) sees the one the client
  reached.

### `wait`

An MCP client cannot read the CP's SSE streams, so `wait` long-polls:

```json
{ "watch": [ {"kind": "run_settled", "run": "run_01J..."} ],
  "cursor": {"run:run_01J...": 12}, "timeout_ms": 30000 }
```

| Condition | Fires when | `host`? |
|---|---|---|
| `{kind: "run_settled", run}` | the run reached a terminal status | yes |
| `{kind: "run_gate", run}` | the run has a parked approval gate | yes |
| `{kind: "run_events", run, types?}` | the run wrote an event (optionally only these types) | yes |
| `{kind: "messages", root, kinds?}` | the root's message log has a new message (optionally only these kinds) | yes |
| `{kind: "agentiflow", id, types?}` | the agentiflow wrote a line (optionally only these `kind`s: `round`, `run_stopped`, ...) | local only |
| `{kind: "agent_run_end", run}` | a standalone agent run finished (read its `final_answer` from the reply) | local only |
| `{kind: "session_turn", session, run?}` | a session turn ended; pass the `run` id `sessions.send` returned, else it waits for the active turn (or the next one if the session is idle) | local only |
| `{kind: "findings", project?, run?, q?}` | new finding records were recorded (a tag change on an older finding is not one); `q` is the findings query language | local only |

Behaviour:

- It checks state first, so a run that already settled or parked answers at
  once. Then it catches up from `cursor` (ids are the SSE `id:` lines, so it
  mixes safely with the streams) and waits for a new line.
- The timeout defaults to 30 s and is capped at 55 s.
- The reply carries `events`, the advanced `cursor`, which conditions
  `fired`, `timed_out`, and a `warnings` list when something needs saying (a
  remote host that stopped answering and is being retried). Pass the returned
  `cursor` back on the next call.
- **Cursor keys** name the watched source, the same name `events` and
  `warnings` use. The cursor holds the last line id read per source that
  returns lines:
  - `run:<id>` for `run_events`;
  - `messages:<root>` for `messages`;
  - `agentiflow:<id>` for `agentiflow`;
  - `findings:<ws_id>/<target>` for `findings`, one key per ledger (a
    project's coverage target).

  A source on a remote host adds `@<host>`: `run:<id>@<host>`,
  `messages:<root>@<host>`. That includes a run found on a host without
  `host` being named. `run_settled`, `run_gate`, `agent_run_end`
  (`agent_run:<id>`) and `session_turn` (`session:<id>`) watch state, so they
  keep no cursor.
- **Cursor values** are integers (line numbers) for local sources and for a
  remote run's events; a remote message space is polled, and its cursor value
  is the last message **id**, a string.
- **Findings without a cursor start at the ledger's end** (only records
  recorded after the call began), except when scoped to a `run`, which starts
  at the beginning.
- **Remote hosts** (`host` on the four run/message kinds). A remote run is
  read from the host's event stream; its gates and a remote message space are
  polled every 2 s, or every 10 s over SSH, where each poll is a new `ssh`
  connection. Over SSH the spacing holds across calls too. A call within 10 s
  of the last ask reuses that ask's answer when it can't be wrong (no parked
  gate; messages past the call's cursor). Otherwise it waits for the ask's
  turn, up to 5 s past a shorter timeout as for a first answer, and names the
  source in `warnings` (`asked N s ago over SSH; next ask in M s`) when the
  turn comes later than that. A remote `run_settled` fires once the host's
  stream ends, about 2 s after the run's last event. A call watching a remote host can
  wait up to 5 s from its start for that host's first answer, even with
  `timeout_ms: 0` or when something already fired. A `timeout_ms: 0` peek at
  a remote run's events returns the events the host already has. A host that
  stops answering after its first answer is retried and named in `warnings`.
- A condition error (a run, session or turn that does not appear within 10 s)
  fails the whole call.
- **Progress.** When the client sends a `progressToken` and accepts SSE, the
  call streams `notifications/progress` every 5 s while it waits, so a client
  with an idle timeout keeps the connection.
- **Once `run_gate` has fired, drop it from `watch`** (or approve / reject the
  gate): a parked gate is state, so it fires immediately on every call.

A loop that launches a workflow with a gate and finishes it:

1. `workflows.run {name: "release"}` returns the run id.
2. `wait {watch: [{kind: "run_gate", run: ID}, {kind: "run_settled", run: ID}]}`
   returns with `run_gate` fired.
3. `runs.approve {id: ID}`.
4. `wait {watch: [{kind: "run_settled", run: ID}], cursor}` (no `run_gate`)
   returns when the run settles, or with `timed_out: true`, in which case call
   it again.

And one that runs an agent: `agents.run` returns the run id,
`wait {watch: [{kind: "agent_run_end", run: ID}]}` returns when it ends.

## Catalog tools

Beside the operator tools the endpoint serves the `rupu-tools` catalog: the
fs/shell tools (`read_file`, `write_file`, `edit_file`, `grep`, `glob`, `bash`,
...), the SCM and issue tools (`scm.*`, `issues.*`, `github.*`, `gitlab.*`;
[scm.md](scm.md#mcp-tool-catalog) describes each) and `findings.report` /
`findings.verify`.

The fs/shell and findings tools take a required **`workspace`** argument: a
project id, or the absolute path of a registered project. The call runs with
that project's layered config (global, then customer, then project), its SCM
registry and its findings ledger. A project's config and registry are cached
until the server restarts.

Where a catalog tool has the name of an operator tool (`agents.list`,
`workflows.list`, `agents.get`, ...), the operator tool is the one served.
Every other catalog tool the server can serve is listed, `findings.query`,
`findings.tag` and `catalog.search` included. A catalog tool whose service the
server lacks is left out and named once in the server log at startup, at `info`
level (`RUST_LOG=info`).
Findings filed over MCP record under the `mcp` scope. A served `bash`'s network
flows are not captured: the call is no run, so there is no run ledger for them.

### Choosing the tools: `--tools`

`rupu cp serve --tools <grant>` narrows the MCP to the tools it names. It
filters **every** tool: the operator tools and `wait` as well as the catalog
tools. It uses the agent `tools:` grammar ([agent-format.md](agent-format.md)):
exact names, legacy aliases, `ns.*`, `core.*` and `*`, comma-separated or
repeated. Without `--tools` every tool is served.

```sh
# operator tools, wait, messages and findings only
rupu cp serve --no-web --tools 'runs.*,wait,messages.*,findings.*'
# a read-only code browser: no operator tools at all
rupu cp serve --no-web --tools read_file,grep
```

A narrow grant such as `read_file,grep` therefore drops the operator tools
and `wait`; name them (`runs.*,wait,...`) to keep them. A name that matches no
tool fails startup, listing the namespaces served and, for a near miss, a
did-you-mean (`--tools run.list` suggests `runs.list`).

## Resources and prompts

`/mcp` also serves read-only **resources**, built into the binary (never read
from `RUPU_HOME`), so an agent can learn rupu from the MCP alone and sees the
docs of the version it talks to:

| URI | Content |
|---|---|
| `rupu://docs/<name>` | The docs under `docs/`: `using-rupu`, `agent-format`, `agent-authoring`, `workflow-format`, `workflow-authoring`, `agentiflows`, `triggers`, `engagement-profiles`, `coverage`, `messaging`, `skills`, `transcript-schema`, `cp-api`, `control-plane`, `response-outcomes`, `netflow`, `configuration`, `mcp` (Markdown) |
| `rupu://schema/finding-report` | The finding report JSON Schema (what `findings.schema` answers) |
| `rupu://query/<list>` | A list query's field registry: `findings`, `workflow_runs`, `agent_runs`, `sessions`, `autoflow_runs`, `events` (what `query.fields` answers) |
| `rupu://profiles/<id>` | Each built-in engagement profile, as TOML. `engagement_profiles.list` also lists layered ones |
| `rupu://tools` | The tool catalog, the rows of `GET /api/tools` |
| `rupu://examples/<path>` | The repo's `examples/` agents, workflows and contracts |

`resources/list` and `resources/templates/list` are cursor-paged;
`resources/read` of an unknown URI answers `-32002` with `data.uri`.

Four **prompts** (`prompts/list`, `prompts/get`): `author-agent`,
`author-workflow`, `author-agentiflow` and `triage-findings` (with a `project`
argument). Each is a short instruction that names the resources and tools to
use; the links are text, not embedded resources, because the docs run 5–73 KB.

## Attribution

An operator message sent through `messages.send`, and a finding tag event
written through `findings.tag`, are recorded with `client: "mcp"` beside
`via: "cp"`, so the Messages tab and the logs can tell an MCP client from the
web UI. A gate decision or cancel through `runs.approve`,
`runs.reject` or `runs.cancel` on the local host records the actor `mcp`
where the web UI records `web`. The mark comes from the `/mcp` endpoint
itself, never from a request header, so a plain HTTP call can't claim it.

## Protocol

MCP streamable HTTP, JSON responses:

- **POST** carries one JSON-RPC message or a batch. A body of only
  notifications answers `202`. **GET** and **DELETE** answer `405` (no server
  push, no session teardown).
- `initialize` mints an `Mcp-Session-Id`, sent back in the response header; a
  client echoes it on later calls. A `notifications/cancelled` is scoped to the
  session that sent it.
- Versions: `initialize` answers the client's `protocolVersion` when rupu
  speaks it (`2024-11-05`, `2025-03-26`, `2025-06-18`, `2025-11-25`), otherwise
  its newest.
- Methods: `initialize`, `ping`, `tools/list`, `tools/call`, `resources/list`,
  `resources/templates/list`, `resources/read`, `prompts/list`, `prompts/get`. A tool that fails,
  refuses its arguments or is unknown comes back as a `tools/call` result with
  `isError: true`, so the model sees why.

## `rupu mcp serve` is deprecated

`rupu mcp serve` printed a stdio server. It now prints a deprecation notice and
runs `rupu cp serve --no-web` on `127.0.0.1:7878`; `--transport` and `--mode`
are ignored and `--tools` is passed on. **stdio is gone.** Point clients at the
HTTP URL as above (for a client that only speaks stdio, use `mcp-remote`).

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| `401` from `/mcp` | The server runs with `--token` and the client sent none |
| `403` from `/mcp` | `Origin` differs from `Host`, or a loopback bind got a non-loopback `Host` (a proxy or tunnel rewriting it) |
| `405` on GET / DELETE | Expected: `/mcp` is POST only |
| A fs / shell / findings tool says `workspace` is required | Pass a project id or the absolute path of a registered project |
| `wait` returns instantly with `run_gate` | The gate is still parked; approve it or drop `run_gate` from `watch` |
| `wait` on a `findings`, `session_turn`, `agentiflow` or `agent_run_end` condition with a `host` is refused | `host` applies only to `run_settled`, `run_gate`, `run_events` and `messages` |
| `wait` with a remote `host` takes a few seconds even with `timeout_ms: 0` | It waits up to 5 s for the host's first answer |
| A result has an extra `x-rupu-...` text line | A fan-out left hosts out; the note names them |
| A tool errors with "over 16 MiB" | Fetch the named API path directly with the token |
| Tool is missing from `tools/list` | `--tools` excludes it, or it is a catalog tool whose service the server lacks (logged at `info` level at startup: run with `RUST_LOG=info`) |
| `--tools` fails at startup with a "did you mean" | A name matches no tool; use the suggestion |

## See also

- [control-plane.md](control-plane.md) - the server, its token and its modes
- [cp-api.md](cp-api.md) - the JSON the operator tools return
- [scm.md](scm.md#mcp-tool-catalog) - the SCM and issue tools
- [messaging.md](messaging.md) - message spaces behind `messages.*`
