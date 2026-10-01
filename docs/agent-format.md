# Agent File Format Reference

> See also: [agent-authoring.md](agent-authoring.md) · [workflow-format.md](workflow-format.md) · [using-rupu.md](using-rupu.md)

---

## Overview

An agent is a Markdown file with YAML frontmatter. The frontmatter tells `rupu` how to run the agent; the Markdown body is the system prompt sent to the model.

`rupu` parses agent frontmatter with strict unknown-field rejection. Typos fail fast instead of being silently ignored.

---

## File location and resolution

```text
<dir>/agents/<name>.md
```

`<dir>` is one of:

- `~/.rupu` for global agents
- `<project>/.rupu` for project-local agents

Resolution rules:

- project-local agents shadow global agents by `name:`
- shadowing is all-or-nothing; `rupu` does not merge frontmatter or prompts
- `rupu agent list` shows whether an agent is coming from project or global scope

---

## Required structure

```markdown
---
name: fix-bug
provider: anthropic
model: claude-sonnet-4-6
permissionMode: ask
---

You are a careful engineer. Reproduce the bug, find the root cause,
apply the minimal fix, and verify the result.
```

Everything after the closing `---` is the system prompt.

---

## Frontmatter fields

| Key | Type | Required | Default | Notes |
| --- | --- | --- | --- | --- |
| `name` | string | yes | — | Agent identifier used by `rupu run <name>` and workflows |
| `description` | string | no | none | Human-readable summary shown by `rupu agent list` |
| `provider` | string | no | `anthropic` | Use an explicit provider in checked-in agents |
| `auth` | `api-key` \| `sso` | no | resolver chooses | Optional auth-mode hint when both modes exist |
| `model` | string | no | config `default_model`, else `claude-sonnet-4-6` | Provider-specific model id |
| `tools` | array<string> | no | built-in tools + discovered MCP tools | Strongly recommend declaring this explicitly |
| `maxTurns` | integer | no | `50` | Hard cap on model turns |
| `permissionMode` | `ask` \| `bypass` \| `readonly` | no | `ask` | CLI `--mode` overrides the file |
| `anthropicOauthPrefix` | bool | no | provider default | Anthropic SSO only |
| `anthropicPromptCache` | bool | no | `[providers.<name>] prompt_cache`, else on | Anthropic only; `false` disables prompt caching for this agent |
| `effort` | string | no | provider default | Cross-provider reasoning level |
| `contextWindow` | string | no | model default | Cross-provider context tier |
| `outputFormat` | `text` \| `json` | no | free-form text | Hint for structured outputs |
| `anthropicTaskBudget` | integer | no | none | Anthropic-only soft output budget |
| `anthropicContextManagement` | string | no | none | Anthropic-only context pruning |
| `anthropicSpeed` | string | no | none | Anthropic-only fast mode |
| `outputSchema` | object (inline YAML) | no | none | JSON Schema for Anthropic structured outputs; only takes effect with `outputFormat: json` |
| `dispatchableAgents` | array\<string\> | no | none (no dispatch) | Allowlist of agent names this agent may dispatch via `dispatch_agent` / `dispatch_agents_parallel` |
| `concerns` | object | no | none | Coverage-concerns block; injects the coverage tools + catalog into the system prompt |
| `findingsProfile` | `full` \| `summary` | no | `full` | Findings contract for `report_finding`; a workflow step's `findings_profile` or the workflow's `defaults.findings_profile` overrides it |
| `engagementProfiles` | array<string> | no | none (native `code` path) | Engagement profile ids this agent's findings are recorded under (`binary`, or one you author); a CLI flag or the workflow's `defaults.engagement_profiles` overrides it, and a workflow step can only narrow it |
| `maxTokens` | integer | no | `8192` | Per-request output-token budget (`max_tokens` in the LLM request); extended thinking (`effort`) draws from this same budget |
| `contextWindowTokens` | integer | no | none (compaction disabled) | Model context-window size in tokens; when set, enables proactive LLM context compaction |
| `compactAtPercent` | integer | no | `80` when `contextWindowTokens` is set | Percentage of `contextWindowTokens` at which compaction triggers; clamped to `[10, 95]` |

---

## Field details

### `name`

Use lowercase, hyphen-separated names such as `fix-bug`, `review-diff`, or `security-reviewer`.

### `provider`

Use the canonical provider names in checked-in agents:

- `anthropic`
- `openai`
- `gemini`
- `copilot`

`rupu` accepts some aliases internally, but canonical names keep your repo easier to read and maintain.

### `auth`

Valid values:

- `api-key`
- `sso`

Use `auth:` when the same provider may have multiple valid credentials and the agent depends on one path. If omitted, the credential resolver picks the available credential, preferring SSO when both exist.

### `model`

`model:` is provider-specific. `rupu` does not validate model ids at parse time. Invalid ids fail at runtime when the provider call is made.

For stable project behavior, prefer setting `model:` per agent instead of relying on a mutable global default.

### `tools`

`tools:` is the agent's tool allowlist.

Built-in tool names:

- `bash`
- `read_file`
- `write_file`
- `edit_file`
- `grep`
- `glob`
- `ast_grep`
- `dispatch_agent`
- `dispatch_agents_parallel`

`dispatch_agent` and `dispatch_agents_parallel` dispatch to child agents named
in this agent's `dispatchableAgents:` list (see below); they fail at
invocation if the requested agent isn't on that allowlist.

MCP-backed tool names are also valid, for example:

- `scm.prs.get`
- `scm.prs.diff`
- `scm.prs.create`
- `issues.get`
- `issues.comment`
- `scm.*`
- `issues.*`
- `*`

Allowlist matching rules:

- exact match: `scm.prs.get`
- prefix wildcard: `scm.*`
- global wildcard: `*`

Notes:

- if you omit `tools:`, the agent gets the full built-in surface and, when SCM / issue connectors are configured, discovered MCP tools as well
- for reusable repo agents, explicit `tools:` is better than relying on the implicit wide-open default
- `tools:` is not the same thing as a workflow step's `actions:`. `tools:` is this agent's full tool grant; `actions:` (on a workflow step) can only narrow the connector/MCP subset of that grant further for that one step — it never touches builtin tools (`bash`, `read_file`, `write_file`, `edit_file`, `grep`, `glob`, `ast_grep`, `dispatch_agent`, `dispatch_agents_parallel`) and can never grant a tool beyond what `tools:` already allows

### `permissionMode`

Valid values:

| Value | Meaning |
| --- | --- |
| `ask` | prompt before shell and write effects |
| `bypass` | execute allowed tools without confirmation |
| `readonly` | allow reads, deny writes |

`readonly` blocks:

- built-in writes: `write_file`, `edit_file`, destructive shell work via `bash`
- MCP write tools such as `scm.prs.create`, `issues.comment`, `issues.create`, `scm.branches.create`

`ask` is the safest default for agents that edit code. In non-interactive contexts, `ask` cannot proceed; use `--mode bypass` or `--mode readonly` explicitly.

### `maxTurns`

`maxTurns` is a hard stop on model turns, not a token budget. Keep it lower for narrow agents such as reviewers and higher for implementation agents.

### `effort`

Accepted values:

- `auto`
- `minimal`
- `low`
- `medium`
- `high`
- `max`

Aliases also accepted:

- `adaptive` → `auto`
- `xhigh` → `max`

Use `effort` only when the task genuinely benefits from more reasoning. Setting every agent to `max` is usually wasted latency and cost.

#### How `effort` reaches each provider

`effort` is a single cross-provider setting, but every provider takes a different
knob, so rupu translates it per provider:

| Provider | Wire form |
| --- | --- |
| Anthropic | `thinking.budget_tokens` — a token budget derived from the level |
| Gemini 3 | `generationConfig.thinkingConfig.thinkingLevel` (lowercase) |
| Gemini 2.5 and earlier | `generationConfig.thinkingConfig.thinkingBudget` (numeric) |
| OpenAI Codex | `reasoning.effort`, sent only for models that support reasoning |
| OpenAI-compatible endpoints | `reasoning_effort`, forwarded verbatim |

`auto` is special-cased: on Gemini it sends the `thinkingBudget: -1` sentinel
("model decides"), and on the openai-compatible path it sends no
`reasoning_effort` key at all.

#### Caveat for OpenAI-compatible endpoints

For a `[providers.<name>]` entry using the openai-compatible wire (Oracle GenAI,
vLLM, OpenRouter, Together, DeepSeek, Groq, xAI, …), the level is mapped to a
`reasoning_effort` string and **forwarded verbatim**. rupu applies no
per-provider allowlist, because the accepted set differs by vendor *and by
model* and there is no reliable way to know it ahead of time.

The two values most likely to be unsupported are the ends of the ladder —
`minimal` and `max` (which maps to `xhigh`). Vendor behavior on an unrecognized
`reasoning_effort` is **not consistent and not well documented**: some ignore it,
some reject the request. If a self-hosted or third-party endpoint errors on a
request that works without `effort:`, drop back to `low`/`medium`/`high`, which
are the values in widest use.

rupu deliberately does **not** clamp `minimal`/`max` for these endpoints — doing
so would silently downgrade an explicit setting on the endpoints that *do*
support them.

### `contextWindow`

Accepted values:

- `default`
- `1m`
- `1M`
- `one_million`

Use this sparingly. Most agents should let the model use its normal context window.

### `outputFormat`

Accepted values:

- `text`
- `json`

Use `json` only when the caller downstream needs machine-readable output. If you set `outputFormat: json`, the system prompt should still describe the exact JSON shape expected.

### `outputSchema`

Inline YAML mapping deserialized straight into a JSON Schema. Only takes effect when `outputFormat: json` is also set — Anthropic then guarantees a schema-conforming response via `output_config.format = {type: "json_schema", schema: <this value>}`. Without `outputSchema`, `outputFormat: json` is prompt-driven only (no server-side guarantee). Ignored by providers other than Anthropic.

### `dispatchableAgents`

Allowlist of agent names this agent may hand work to via the `dispatch_agent` / `dispatch_agents_parallel` builtin tools. Omit it (the default) to give the agent no dispatch capability at all — the dispatch tools still appear in the registry, but any call fails at invocation with "not in dispatchableAgents".

### `concerns`

Coverage-concerns block (see `docs/coverage.md`). When present, the runner flattens the concern catalog, writes a snapshot to `.rupu/coverage/<target>/catalog.yaml`, injects the four coverage tools, and prepends the catalog to the system prompt. A workflow step's own `concerns:` block takes precedence over the agent's when both are set.

### `findingsProfile`

Selects the contract an agent's findings are recorded under. It governs the agent's `report_finding` builtin, whether the tool comes from a `concerns:` block (which injects the coverage tools) or from an explicit `tools: [report_finding]` grant.

- `full` (default) — findings must include a complete `report` (see `rupu findings schema`). The tool rejects `summary`, `severity`, and `evidence` as separate arguments: rupu derives them from the report (`summary` from `title`, `severity` from `rating.risk_rating`, `evidence.rationale` from `root_cause`). A rejected call lists every validation problem at once so the agent can fix them all in one retry. When the run can record findings (a `concerns:` block or `report_finding` in `tools:`), finding-writing guidance is appended to the system prompt. rupu generates the Markdown, HTML and PDF reports from the stored report (see [coverage.md](coverage.md#exporting-reports)), so an agent should not also write a report file.
- `summary` — the lightweight `summary` / `severity` / `evidence` record. A `report` sent under `summary` is refused rather than silently dropped.

A workflow step's `findings_profile` or the workflow's `defaults.findings_profile` overrides this value. The order is step → workflow defaults → agent `findingsProfile` → `full`; see [workflow-format.md](workflow-format.md#findings_profile). A standalone `rupu run <agent>` uses `--findings-profile full|summary` when given, then the agent's value, then `full`. The flag is also how a remote workflow step's profile reaches the host that runs the agent. Sub-agents started through `dispatch_agent` resolve only from their own agent file.

**Upgrade note:** because the built-in default is `full`, an existing agent that records thin findings (a `concerns:` block, or `report_finding` in `tools:`) is now rejected unless it sets `findingsProfile: summary` (or its workflow sets `findings_profile: summary`), or its prompt is updated to send a complete `report`.

**Remote hosts:** agent frontmatter rejects unknown keys, so a rupu release that predates `findingsProfile` refuses to load an agent file that sets it. A workflow step placed on a remote host (`host:` / `distribute:`) resolves its profile from the agent file on that host, so upgrade rupu on every remote host before adding `findingsProfile` to agents they run.

```yaml
---
name: quick-scanner
concerns:
  - include: secrets-in-source
findingsProfile: summary   # lightweight findings; set to full for complete reports
---
```

See `docs/coverage.md` for what a complete report requires. Every field is required except `cwe` (it may be empty) and `artifacts`. `verification` is set by verification runs, not by the reporting agent, and a call that supplies it is rejected.

### `engagementProfiles`

Selects the engagement profiles an agent's findings are recorded under. An engagement profile (`binary`, or one you author) declares the kinds of asset an assessment works with, what a complete finding must contain, and the depth ladder for tracking coverage; see [coverage.md](coverage.md#engagement-profiles). Only `code` and `binary` ship; any other id must be a profile you authored, and an id no profile defines fails the launch with `unknown engagement profile`.

- Omitted or `[]` — the agent selects nothing. The run falls through to `code`, the native path with no engagement, which is exactly how agents written before engagement profiles behave.
- `[code]` — the same native path, stated explicitly. As a CLI flag or a workflow default, `[code]` overrides the agent's own `engagementProfiles` and puts the run on the native path.
- `[binary]` — an engagement with the built-in `binary` profile: findings name an `asset` (for example a `binary:function`), are routed to the profile that owns the asset's kind, and must satisfy that profile's completeness checks under the `full` findings profile.
- `[code, binary]` — an engagement with both; `code` is an ordinary built-in here (asset kind `code:file`), so a finding that names no `asset` is still accepted.

The run's set comes from the first non-empty of: `rupu run --engagement-profile <id>` (repeatable) or `--engagement-profiles a,b`, for a standalone run (`rupu session start` takes the same flag, snapshotted at start); the workflow's `defaults.engagement_profiles`, for a workflow run; this field; then `code`. A workflow step's own `engagement_profiles` does not replace the set, it only **narrows** it (naming an id outside the set fails the launch); see [workflow-format.md](workflow-format.md#engagement_profiles).

**Coverage depth.** An agent records how deeply it has examined an asset with the `asset_mark` tool. It is never granted automatically: list `asset_mark` in `tools:` (a `concerns:` block does not add it). It also needs an active engagement; without one the call fails. `depth` must be a rung of the owning profile's ladder (for `binary`: `located`, `disassembled`, `analyzed`).

**Sub-agents.** An agent started by `dispatch_agent` / `dispatch_agents_parallel` does not read its own `engagementProfiles`, unlike `findingsProfile`. It inherits the engagement of the run that dispatched it: for `rupu run`, the parent run's resolved set; for a workflow run, the workflow's `defaults.engagement_profiles` only (the step's narrowing, and the dispatching agent's own frontmatter, are not carried down). An `engagementProfiles` set on an agent that is only ever dispatched therefore has no effect.

**Remote hosts.** An engagement does not reach a remote host yet: a workflow step with `host:` / `distribute:` that selects an engagement is refused unless that step sets `engagement_profiles: [code]` (see [workflow-format.md](workflow-format.md#engagement_profiles)). Agent frontmatter also rejects unknown keys, so a rupu release that predates `engagementProfiles` refuses to load an agent file that sets it.

```yaml
---
name: binary-analyst
provider: anthropic
model: claude-sonnet-4-6
tools: [report_finding, asset_mark]   # asset_mark is an explicit grant; it sets coverage depth
engagementProfiles: [binary]          # run this agent under the binary profile
---

You analyse a compiled binary. Register each function with asset_mark
(a binary:function, located by sha256, address and symbol), and record each
defect with report_finding, naming the same asset.
```

The repo ships a complete sample: `.rupu/agents/binary-analyst.md`, run by `.rupu/workflows/binary-assessment.yaml`.

### `maxTokens`

Per-request output-token budget (the LLM request's `max_tokens`). Defaults to `8192` when omitted. Extended thinking (`effort`) draws from this same budget, so raise it for agents that both reason heavily and produce long output.

### `contextWindowTokens` and `compactAtPercent`

`contextWindowTokens` sets the model's context-window size in tokens and enables proactive LLM context compaction: when the previous turn's input exceeded `compactAtPercent` of this value, the runner summarizes older turns before the next turn. `compactAtPercent` defaults to `80` when `contextWindowTokens` is set and is otherwise omitted; values are clamped to `[10, 95]`. Leaving `contextWindowTokens` unset disables compaction entirely.

### Anthropic-specific fields

| Key | Valid values | Purpose |
| --- | --- | --- |
| `anthropicOauthPrefix` | `true` / `false` | Enables or disables Anthropic's OAuth system prefix |
| `anthropicPromptCache` | `true` / `false` | Enables or disables explicit prompt caching (default on) |
| `anthropicTaskBudget` | positive integer | Soft output budget, separate from `maxTurns` |
| `anthropicContextManagement` | `tool_clearing` | Server-side pruning of older tool blocks |
| `anthropicSpeed` | `fast` | Account-gated fast mode |

If an agent needs to stay portable across providers, avoid Anthropic-only fields.

#### `anthropicPromptCache`

Anthropic requests use prompt caching by default. Each request carries two
explicit `cache_control: {"type": "ephemeral"}` breakpoints (5-minute TTL):

- one on the last system block, which caches the tool definitions and the
  system prompt together (on the last tool definition when there is no system
  prompt);
- one on the last content block of the final message, which moves forward
  each turn so every turn re-reads the prior conversation from the cache.

Thinking blocks, empty text blocks, and empty tool results are never marked.
When the final message has nothing markable (for example the empty result of a
silent `bash` command), the second marker moves to the last eligible block of
the nearest earlier message — typically the preceding `tool_use` — at most
three messages back. A prefix below the
model's minimum cacheable length (512–4096 tokens, depending on the model) is
simply not cached — there is no error. Cache reads and writes show up as
`cached_tokens` and `cache_write_tokens` in usage and cost.

`anthropicPromptCache: false` turns the breakpoints off for this agent. It
overrides `[providers.<name>] prompt_cache` in either direction, so `true`
re-enables caching for one agent on a provider whose config turns it off. Use
`false` when the agent's provider is an Anthropic-compatible gateway that
rejects `cache_control`. For a whole provider, prefer
`[providers.<name>] prompt_cache = false` (see
[providers.md](providers.md#field-reference), including its caveat that
Anthropic gateway routing is currently process-wide). Omitted, the agent follows the
provider config, and caching is on when neither sets it.

---

## System prompt body

The Markdown body is passed as the system prompt exactly as written.

A good agent prompt usually contains:

1. the role it should play
2. the scope of the task
3. the expected work sequence
4. the validation bar
5. the stop condition
6. the output contract

See [agent-authoring.md](agent-authoring.md) for concrete prompting patterns.

---

## Worked examples

### Minimal read-only reviewer

```markdown
---
name: review-summary
tools: [read_file, grep, glob]
permissionMode: readonly
---

You review the files the user points at.
Return a short bulleted list of issues, or `no issues`.
Do not make edits.
```

### SCM PR reviewer

```markdown
---
name: scm-pr-review
provider: anthropic
model: claude-sonnet-4-6
tools: [scm.prs.get, scm.prs.diff, scm.prs.comment]
maxTurns: 6
permissionMode: ask
---

You are a code reviewer.
Read the PR metadata and diff.
Look for correctness, security, and missing-test issues.
Post one concise review comment.
```

### Panel reviewer with structured JSON output

```markdown
---
name: security-reviewer
tools: [read_file, grep, glob, scm.prs.get, scm.prs.diff]
permissionMode: readonly
outputFormat: json
maxTurns: 6
---

You are a security reviewer.
If given a PR ref, fetch the diff with SCM tools.
If given a local file or diff, inspect that directly.

Your final assistant message must contain:
{
  "findings": [
    { "severity": "low|medium|high|critical",
      "title": "short title",
      "body": "one sentence detail" }
  ]
}

If there are no findings, return {"findings":[]}.
```

---

## Validation and failure modes

Common failures:

- missing opening `---` or closing frontmatter delimiter
- misspelled keys such as `permision_mode`
- invalid enum values such as `outputFormat: yaml`
- unsupported or missing provider credentials at runtime
- `ask` mode in a non-interactive context

For predictable behavior:

- set `provider`, `model`, `tools`, and `permissionMode` explicitly in checked-in agents
- keep agents narrow and specialized
- prefer separate reviewer and implementer agents over one broad do-everything prompt

---

## Practical guidance

- Use [agent-authoring.md](agent-authoring.md) when you are designing a new agent.
- Use [workflow-format.md](workflow-format.md) when that agent will participate in a workflow.
- Use [examples/README.md](../examples/README.md) for complete copyable agent and workflow sets.
