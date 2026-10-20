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

Thirteen tools over the CP's own endpoints. Each takes the arguments of the
endpoint it wraps, an optional `host` where the API takes `?host=`, refuses
unknown arguments, and returns the API's JSON as `structuredContent`.

| Tool | Does |
|---|---|
| `runs.list` | List workflow and agent runs (the CP's `?q=` query language applies) |
| `runs.get` | One run, with its steps |
| `runs.approve` | Approve a parked approval gate |
| `runs.reject` | Reject a parked gate |
| `runs.cancel` | Cancel a run |
| `workflows.list` | List workflow definitions |
| `workflows.run` | Launch a workflow |
| `agents.list` | List agent definitions |
| `agents.run` | Launch an agent |
| `sessions.start` | Start a session |
| `sessions.send` | Send a turn to a session |
| `messages.list` | Read a run's, session's or agentiflow's message log |
| `messages.send` | Send an operator message to a live run, session or agentiflow |

Later releases add the remaining CP routes as tools (findings,
autoflows, projects, usage, hosts and so on), `wait` support for findings,
session turns, agentiflows and remote hosts, the CLI-only extras, and MCP
resources and prompts. What is listed here is what ships today.

### `wait`

An MCP client cannot read the CP's SSE streams, so `wait` long-polls:

```json
{ "watch": [ {"kind": "run_settled", "run": "run_01J..."} ],
  "cursor": "...", "timeout_ms": 30000 }
```

| Condition | Fires when |
|---|---|
| `{kind: "run_settled", run}` | the run reached a terminal status |
| `{kind: "run_gate", run}` | the run has a parked approval gate |
| `{kind: "run_events", run, types?}` | the run wrote an event (optionally only these types) |
| `{kind: "messages", root, kinds?}` | the root's message log has a new message (optionally only these kinds) |

Behaviour:

- It checks state first, so a run that already settled or parked answers at
  once. Then it catches up from `cursor` (ids are the SSE `id:` lines, so it
  mixes safely with the streams) and waits for a new line.
- The timeout defaults to 30 s and is capped at 55 s.
- The reply carries `events`, the advanced `cursor`, which conditions `fired`,
  and `timed_out`. Pass the returned `cursor` back on the next call.
- Local host only for now: a `host` naming a remote is not yet supported.
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

`agents.list` / `agents.get`, `workflows.list` / `workflows.get` and
`findings.query` / `findings.tag` from the catalog are hidden from the list,
because the operator tools cover them.
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
and `wait`; name them (`runs.*,wait,...`) to keep them. An unknown name fails
startup with a did-you-mean.

## Attribution

An operator message sent through `messages.send` is recorded with
`client: "mcp"` beside `via: "cp"`, so the Messages tab and the log can tell an
MCP client from the web UI.

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
- Methods: `initialize`, `ping`, `tools/list`, `tools/call`. A tool that fails,
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
| `wait` on a remote host is refused | Remote hosts are not yet supported by `wait` |
| Tool is missing from `tools/list` | `--tools` excludes it, or it is hidden because an operator tool covers it |

## See also

- [control-plane.md](control-plane.md) - the server, its token and its modes
- [cp-api.md](cp-api.md) - the JSON the operator tools return
- [scm.md](scm.md#mcp-tool-catalog) - the SCM and issue tools
- [messaging.md](messaging.md) - message spaces behind `messages.*`
