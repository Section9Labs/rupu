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
| `maxTokens` | integer | no | discovered output cap | Per-request output-token cap. Overrides the cap discovered from the provider's model list; when neither is known, Anthropic gets `8192` and other providers get no cap (model max). Extended thinking (`effort`) draws from this budget |
| `contextWindowTokens` | integer | no | discovered input limit | Input-token limit used for proactive compaction. Overrides the discovered limit; compaction is off only when neither is known |
| `compactAtPercent` | integer | no | `80` | Percentage of the input limit at which compaction triggers; clamped to `[10, 95]`. When the output cap is known and shares the input window, compaction also triggers early enough that a full-length reply still fits |

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

### `maxTokens`

Per-request output-token cap (the LLM request's `max_tokens`). You rarely need to set it: rupu discovers each model's real output cap from the provider's model list, where the provider reports one (Codex/OpenAI and OpenAI-compatible servers report none), and sends that on every turn. A `maxTokens` value overrides the discovered cap. When neither is known, Anthropic requests carry `8192` (the API requires a cap) and every other provider gets no cap, so the model's own maximum applies. Extended thinking (`effort`) draws from this same budget, so a low `maxTokens` can starve an agent that both reasons heavily and produces long output.

On Anthropic, the `max` effort level's thinking budget scales with the output cap (cap − 2,000 tokens), so a discovered 64K cap raises both the budget and its cost; pin a smaller `maxTokens` to bound it. `--no-stream` only changes display — requests still stream on the wire, so the full discovered output cap applies. Only a `stream = false` OpenAI-compatible server (one that genuinely cannot stream) carries `max_tokens: 8192` when no cap is known, instead of omitting the field.

### `contextWindowTokens` and `compactAtPercent`

`contextWindowTokens` is the model's input-token limit, used for proactive context compaction. rupu discovers it from the provider's model list; set it only to override the discovered value (for example to compact earlier than the model requires, or on a provider that reports no limits). Compaction is off only when neither a pin nor a discovered value is available.

`compactAtPercent` (default `80`, clamped to `[10, 95]`) is the share of the input limit at which the runner summarizes older turns before the next turn. When the output cap is known and the model's output counts against the same window as its input (Anthropic, Codex, OpenAI-compatible), the threshold is the lower of that percentage and the input limit minus the output cap, so a full-length reply still fits. Otherwise the threshold is simply `input × compactAtPercent / 100`: Copilot and Gemini budget input and output independently, and an unknown output cap (including the Anthropic `8192` wire fallback, which is not a discovered limit) gives no headroom to subtract.

Where the limits come from, per field, in precedence order:

1. the agent's `contextWindowTokens` / `maxTokens`;
2. a `[[providers.<name>.models]]` entry in `config.toml` (value greater than zero);
3. the provider's live model list, cached for 1 hour in `~/.rupu/cache/models/` (see [providers.md](providers.md#model-resolution) for each provider's source);
4. unknown.

Every run writes a `model_limits` notice to its transcript stating the input limit, output cap and compaction threshold it will use and where each came from, including an explicit "unknown" when a limit could not be discovered. If the provider later rejects a request as "prompt too long" and reports the real maximum, rupu lowers the input limit to that value for the rest of the run (a `model_limits_clamped` notice), compacts with a summary, and retries. When the input limit is known, rupu compacts once per turn this way even if the error carries no number; dropping the oldest turns is the last resort. A session resolves its limits on its first turn and keeps them, including a lowered limit, for later turns (a first turn that found no limit at all is resolved again on the next turn).

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
