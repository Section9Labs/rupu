# Operator MCP Plan 2: live check

Run from Claude Code (`claude -p`) against `target/debug/rupu cp serve --no-web --bind 127.0.0.1:7879`, built from branch `claude/operator-mcp-plan-2`, with an isolated `RUPU_HOME` that held only a copy of `~/.rupu/config.toml` (no credentials). The client used a scratch `--mcp-config` (`{"mcpServers":{"rupu-test":{"type":"http","url":"http://127.0.0.1:7879/mcp"}}}`) with `--strict-mcp-config` and `--allowedTools 'mcp__rupu-test__*'`; nothing was added to Claude Code's own MCP settings. `tools/list` returned 168 tools (138 operator tools, `wait`, the catalog tools).

Results (excerpts trimmed; ids shortened):

| Step | Result |
|---|---|
| 1. `agents.create` a tiny agent, `agents.run` it | create passed; run returned `run_01M5C184...` |
| 2. `wait {agent_run_end}` | **not exercised end to end** (see below) |
| 3. `workflows.run` a gated workflow, `wait {run_gate}`, `runs.approve`, `wait {run_settled}` | passed |
| 4. `findings.list`, `findings.tag` | passed |
| 5. `findings.export` as markdown | passed (embedded resource) |
| 6. tag log line has `client: mcp` | passed |

## Step 1-2: agent run (blocked by credentials)

```
CALL agents_create {"raw": "---\nname: tiny\nprovider: anthropic\nmodel: claude-haiku-4-5\n---\nAnswer in one word.\n"}
RESULT {"name":"tiny","provider":"anthropic","scope":"global","run_count":0,...}
CALL agents_run {"name": "tiny", "prompt": "Say hello"}
RESULT {"host_id":"local","run_id":"run_01M5C1843XZ58ZA37WBA963JNC"}
CALL wait {"watch": [{"kind": "agent_run_end", "run": "run_01M5C1843XZ58ZA37WBA963JNC"}], "timeout_ms": 40000}
RESULT [ERR] no agent run `run_01M5C1843XZ58ZA37WBA963JNC`
```

The isolated home has no provider credentials (none were copied). The launched `rupu run` exits before writing a transcript, so there is nothing for `agent_run_end` to find. Running the same command by hand shows why:

```
$ RUPU_HOME=<tmp> target/debug/rupu run tiny "hi"
[error] build provider anthropic (claude-haiku-4-5): missing credential for provider anthropic (no credentials configured for anthropic. Run: rupu auth login ...)
```

So `final_answer` was not read. The `wait` kind itself is covered by the crate's tests. Observation: a launch that fails this early surfaces to `wait` only as "no agent run" after the 10 s appear grace, not as the launch error.

## Step 3: gated workflow (no model needed)

A standalone gate workflow (`steps: [{id: ship_gate, approval: {required: true, prompt: "Approve?"}}]`) created with `workflows.create`:

```
CALL workflows_run {"name": "gated"}
RESULT {"host_id":"local","run_id":"run_01M5C190MZFA8TP7WA8G5G67VF"}
CALL wait {"watch": [{"kind": "run_gate", "run": "run_01M5C190..."}], "timeout_ms": 20000}
RESULT {"cursor":{},"events":[],"fired":[{"detail":{"gates":[{"expires_at":null,"prompt":"Approve?","step_id":"ship_gate"}]},"index":0,"kind":"run_gate"}],"timed_out":false}
CALL runs_approve {"id": "run_01M5C190..."}
RESULT {"host_id":"local","run":{...,"gate_decisions":[{"approver":"mcp","step_id":"ship_gate","verdict":"approved","via":"human"}],"status":"running",...}}
CALL wait {"watch": [{"kind": "run_settled", "run": "run_01M5C190..."}], "cursor": {}, "timeout_ms": 20000}
RESULT {"cursor":{},"events":[],"fired":[{"detail":{"status":"completed"},"index":0,"kind":"run_settled"}],"timed_out":false}
CALL runs_get {"id": "run_01M5C190..."}
RESULT {"run":{...,"status":"completed","workflow_name":"gated"}}
```

The gate decision's approver is `mcp`, as `docs/mcp.md` says.

## Steps 4-6: findings

The finding was seeded through the catalog `findings.report` tool with `workspace` = the project's `ws_id` (the isolated home registered the worktree as a project on first use).

```
CALL projects_list {}
RESULT [{"name":"wizardly-edison-a3463f","ws_id":"ws_01M5C1844XHGDWZ56JK8Z4F9QS",...}]
CALL findings_report {"workspace": "ws_01M5C184...", "scope": "line", "file_path": "src/parser/frame.rs", "line_range": [42, 58], "report": {"title": "Demo: unchecked length", ...}}
RESULT finding_id: fnd_01M5C1AXW9MNSJ8RG9BCQAAJZF
CALL findings_list {}
RESULT {"facets":{..."tag":[]...},"findings":[{"id":"fnd_01M5C1AX...","codename":"umber-crater",...}]}
CALL findings_tag {"finding_ids": ["fnd_01M5C1AX..."], "add": ["triaged"]}
RESULT {"unknown":[],"workspaces":[{"outcomes":[{"after":["triaged"],"before":[],"finding_id":"fnd_01M5C1AX..."}],"ws_id":"ws_01M5C184..."}]}
CALL findings_export {"id": "fnd_01M5C1AX...", "format": "md", "ws_id": "ws_01M5C184..."}
RESULT [Resource from rupu-test at rupu://api/findings/fnd_01M5C1AX.../export?format=md&ws_id=ws_01M5C184...]
       Filename: SEC-001 - Demo unchecked length.pdf

       # Demo: unchecked length
       **Identifier:** SEC-001 ... **Risk Rating:** Medium ...
CALL findings_list {"q": "tag:triaged"}
RESULT {"facets":{..."tag":[{"count":1,"value":"triaged"}]...},"findings":[{"id":"fnd_01M5C1AX...","tags":["triaged"],...}]}
```

(The `Filename: ....pdf` line is the report layout's own first block, present in the plain API's markdown export too; the HTTP header of that export names `...md`.)

The tag log line, read from the workspace's `.rupu/coverage/finding_tags.jsonl`:

```
{"id":"tge_01M5C1B2P1FBEE9J4GFP3C4FW9","finding_id":"fnd_01M5C1AXW9MNSJ8RG9BCQAAJZF","op":"add","tag":"triaged","by":{"kind":"operator","user":"matt","via":"cp","client":"mcp"},"at":"2026-10-20T06:52:49.217858Z"}
```

`via` is `cp` and `client` is `mcp`.
