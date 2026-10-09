# rupu's MCP server

`rupu mcp serve` exposes rupu's tool catalog over JSON-RPC stdio per the
[MCP spec](https://spec.modelcontextprotocol.io/). Any MCP-aware client can
spawn it as a subprocess and call the same tools rupu's own agents and
workflow `action:` steps call: the server owns no tools of its own, it is a
transport over the one catalog (`rupu-tools`), and a call goes through the
same tool body and permission policy wherever it comes from.

## Wiring into Claude Desktop

`~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "rupu": {
      "command": "/usr/local/bin/rupu",
      "args": ["mcp", "serve", "--transport", "stdio"]
    }
  }
}
```

After restart, Claude Desktop's tool catalog includes every `scm.*` and
`issues.*` tool from `docs/scm.md`, the CI triggers and the `findings.*` tools
(see [Tool catalog](#tool-catalog)). Authentication is shared with rupu's CLI
through the same credential file (`~/.rupu/auth.json`) — running `rupu auth login --provider github --mode sso`
once unlocks the catalog for both. `--provider` is an alias for `--account`;
see `docs/providers.md` if you need two accounts of the same platform (e.g.
`--account gh-work --kind github`).

## Wiring into Cursor

Cursor's MCP config lives at the IDE's settings level:

```json
{
  "mcp.servers": {
    "rupu": {
      "command": "/usr/local/bin/rupu",
      "args": ["mcp", "serve"]
    }
  }
}
```

## Tool catalog

Without `--tools`, the server serves the connector and findings tools
(`scm.*,issues.*,github.*,gitlab.*,findings.*`, 22 tools). Each tool's input
schema is returned in the MCP `tools/list` response.

| Group | Tools |
|-------|-------|
| Repositories | `scm.repos.list`, `scm.repos.get` |
| Branches | `scm.branches.list`, `scm.branches.create` |
| Files | `scm.files.read` |
| Pull / merge requests | `scm.prs.list`, `scm.prs.get`, `scm.prs.diff`, `scm.prs.comment`, `scm.prs.create` |
| Issues | `issues.list`, `issues.get`, `issues.comments` (GitHub only), `issues.comment`, `issues.create`, `issues.update_state` (open/closed only) |
| CI | `github.workflows_dispatch`, `gitlab.pipeline_trigger` |
| Findings | `findings.report` (also answers to `findings.record`), `findings.verify`, `findings.query`, `findings.tag` |

The 18 SCM and issue tools are described one by one in
`docs/scm.md#mcp-tool-catalog`.

### Choosing the tools: `--tools`

`--tools` takes the agent `tools:` grammar ([agent-format.md](agent-format.md)):
exact names, legacy aliases, `ns.*` namespaces, `core.*` (the fs/shell tools)
and `*` (the whole catalog), comma-separated or repeated:

```sh
rupu mcp serve --tools 'issues.*,findings.*'
rupu mcp serve --tools read_file,grep
```

The fs/shell tools act on the directory `rupu mcp serve` was started in (a served `bash`'s network flows are not captured: the server is no run, so there is no run ledger for them). A
tool needs services the server must be able to provide; the server provides
the SCM registry and the findings ledger of the current directory's workspace,
but not a sub-agent launcher, an agentiflow message bus, a concern catalog or
an engagement. A tool `--tools` names exactly whose service is missing (for
example `dispatch`) is not listed, and startup prints one
`tool_unavailable: …` line to stderr naming it and the missing service; a
wildcard (`*`) skips such tools silently. An unknown name fails startup with a
did-you-mean.

### Findings from an MCP client

The `findings.*` tools work on the current directory's workspace. Findings are
recorded under the `mcp` scope and attributed to the server session: each
`rupu mcp serve` process mints a run id `mcp_<ULID>`, so `findings.verify` can
tell a verification from the session that filed the finding. Findings record
under the full profile unless the layered config's `[findings]` says
otherwise; `findings.report`'s `tools/list` schema is that profile's.

## Permissions

`--mode` decides every call from the tool's effect, exactly as for an agent
run ([agent-format.md](agent-format.md#permissionmode)):

| `--mode` | Allows |
|---|---|
| `bypass` (default) | every call. The upstream MCP client (Claude Desktop, Cursor) prompts the user before a write; rupu does not prompt from the server |
| `readonly` | `read` and `record` tools (connector reads, the `findings.*` tools); refuses `write` (workspace) and `external` (connector writes) |
| `ask` | every call: there is no operator to prompt behind a stdio server, so `ask` behaves as `bypass` |

Connector reads are `read`, connector writes are `external`, `findings.report`
/ `findings.verify` / `findings.tag` are `record`, and `findings.query` is
`read`. A refused call returns an MCP error result (`isError: true`) saying
which mode refused which effect.

## Troubleshooting

| Symptom                                       | Likely cause                                        |
|-----------------------------------------------|-----------------------------------------------------|
| Claude Desktop says `rupu` server failed      | `rupu` not on PATH, or no SCM credentials present    |
| Tool returns "no connector for github"        | `rupu auth login --provider github` needed           |
| Tools/list lacks a tool `--tools` names       | It needs a service the server lacks (see the `tool_unavailable` stderr line) |
| Stdio hangs after `tools/call`                | Long-running connector op; check rate limits         |

## See also

- `docs/scm.md` — full MCP tool catalog + schemas
- `docs/scm/github.md` / `docs/scm/gitlab.md` — per-platform walkthroughs
