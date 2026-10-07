# rupu's MCP server

`rupu mcp serve` exposes the unified SCM + issue tool catalog over JSON-RPC stdio
per the [MCP spec](https://spec.modelcontextprotocol.io/). Any MCP-aware client
can spawn it as a subprocess and call the same tools rupu's own agents call.

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
`issues.*` tool from `docs/scm.md` (plus the `findings.*` tools, which refuse
outside a workflow run; see [Tool catalog](#tool-catalog)). Authentication is shared with rupu's CLI
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

The catalog has 21 tools. Each tool's input schema is auto-generated and
returned in the MCP `tools/list` response.

| Group | Tools |
|-------|-------|
| Repositories | `scm.repos.list`, `scm.repos.get` |
| Branches | `scm.branches.list`, `scm.branches.create` |
| Files | `scm.files.read` |
| Pull / merge requests | `scm.prs.list`, `scm.prs.get`, `scm.prs.diff`, `scm.prs.comment`, `scm.prs.create` |
| Issues | `issues.list`, `issues.get`, `issues.comments` (GitHub only), `issues.comment`, `issues.create`, `issues.update_state` (open/closed only) |
| CI | `github.workflows_dispatch`, `gitlab.pipeline_trigger` |
| Findings | `findings.record`, `findings.query`, `findings.tag` |

The 18 SCM and issue tools are described one by one in
`docs/scm.md#mcp-tool-catalog`.

**The `findings.*` tools need a run context.** They read and write a project's
findings ledger, so they need to know which workspace, run and model to
attribute to. Only a workflow `action:` step supplies that (for example
`action: findings.record` or a `for_each` over `findings.query` with
`all: true`; see `docs/workflow-format.md` and `docs/coverage.md`). Everywhere
else — `rupu mcp serve`, and an agent's own tool calls — the three tools are
still listed but refuse every call with an "unavailable: … started without run
context" error rather than writing to a guessed location. An agent records and
queries findings through its own builtins instead: `report_finding`,
`query_findings` and `tag_findings`.

## Permissions

`rupu mcp serve` runs with permission mode `bypass` and an allow-all
allowlist. The upstream MCP client (Claude Desktop, Cursor) is responsible
for prompting the user before invoking write tools. This matches the rest of
the MCP ecosystem; rupu does NOT prompt from the server.

For `rupu run` invocations from the CLI, the agent's frontmatter `tools:`
list and the `--mode` flag enforce per-tool gating; the MCP server enforces
both.

## Troubleshooting

| Symptom                                       | Likely cause                                        |
|-----------------------------------------------|-----------------------------------------------------|
| Claude Desktop says `rupu` server failed      | `rupu` not on PATH, or no SCM credentials present    |
| Tool returns "no connector for github"        | `rupu auth login --provider github` needed           |
| Tools/list returns 0 entries                  | Build was missing the rupu-mcp crate (re-cargo build)|
| Stdio hangs after `tools/call`                | Long-running connector op; check rate limits         |

## See also

- `docs/scm.md` — full MCP tool catalog + schemas
- `docs/scm/github.md` / `docs/scm/gitlab.md` — per-platform walkthroughs
