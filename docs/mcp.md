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
`issues.*` tool from `docs/scm.md`. Authentication is shared with rupu's CLI
via the OS keychain — running `rupu auth login --provider github --mode sso`
once unlocks the catalog for both.

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

See `docs/scm.md#mcp-tool-catalog` for the full 17-tool surface. Each tool's
input schema is auto-generated and returned in the MCP `tools/list` response.

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
