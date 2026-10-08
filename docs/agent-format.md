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
| `effort` | string | no | provider default | Cross-provider reasoning level (`auto`/`minimal`/`low`/`medium`/`high`/`max`) |
| `thinkingDisplay` | `summarized` \| `omitted` \| `updates` | no | `summarized` | Cross-provider reasoning-display hint; honored by Anthropic adaptive models only |
| `contextWindow` | string | no | model default | Cross-provider context tier |
| `outputFormat` | `text` \| `json` | no | free-form text | Hint for structured outputs |
| `anthropicTaskBudget` | integer | no | none | Anthropic-only soft output budget |
| `anthropicContextManagement` | string | no | none | Anthropic-only context pruning |
| `anthropicSpeed` | string | no | none | Anthropic-only fast mode |
| `outputSchema` | object (inline YAML) | no | none | JSON Schema for Anthropic structured outputs; only takes effect with `outputFormat: json` |
| `dispatchableAgents` | array\<string\> | no | none (no dispatch) | Allowlist of agent names this agent may dispatch via `dispatch_agent` / `dispatch_agents_parallel` |
| `concerns` | object | no | none | Coverage-concerns block; injects the coverage tools + catalog into the system prompt |
| `findingsProfile` | `full` \| `summary` | no | `full` | Findings contract for `findings.report`; a workflow step's `findings_profile` or the workflow's `defaults.findings_profile` overrides it |
| `fallbacks` | array of `{model, provider?}` | no | `[recovery].fallbacks` from config, else none | Ordered fallback models tried when a reply cannot be used (a refusal, say). Wins over the config table; not merged with it. See [response-outcomes.md](response-outcomes.md#3-configuring-fallbacks) |
| `maxTokens` | integer | no | discovered output cap | Per-request output-token cap. Overrides the cap discovered from the provider's model list; when neither is known, Anthropic gets `8192` and other providers get no cap (model max). `0` is ignored. Extended thinking (`effort`) draws from this budget |
| `contextWindowTokens` | integer | no | discovered input limit | Input-token limit used for proactive compaction. Overrides the discovered limit; compaction is off only when neither is known. `0` is ignored |
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

#### The `ast_grep` tool

`ast_grep` searches the workspace by syntax tree rather than by text, so a
pattern matches code with the same shape however it is spaced, wrapped or
commented. It runs the [ast-grep](https://ast-grep.github.io/) binary, which
must be on `PATH` as `ast-grep` (`brew install ast-grep` or
`cargo install ast-grep`; the short `sg` alias is deliberately not used,
because it collides with a system tool on macOS). Without it, every call
returns an "ast-grep not found" error. It is a read tool: `readonly` mode
allows it.

Input:

| Field | Required | Meaning |
|-------|----------|---------|
| `pattern` | yes | A code snippet in the target language, with metavariables |
| `lang` | yes | The grammar to parse the pattern and the files with: any language ast-grep supports, e.g. `rust`, `python`, `typescript`, `tsx`, `javascript`, `go`, `java`, `c`, `cpp` |
| `path` | no | A sub-path of the workspace to search. Defaults to the workspace root; a path that leaves the workspace is refused |

Metavariables:

- `$NAME` (upper-case) matches exactly one syntax node and captures it. The
  same name used twice must match identical code: `$A == $A`.
- `$$$` matches zero or more nodes — an argument list, a statement block, a
  run of parameters. `$$$NAME` captures them.
- `$_` matches one node without capturing it.

Examples (invented):

| `lang` | `pattern` | Finds |
|--------|-----------|-------|
| `rust` | `impl $TRAIT for $TYPE { $$$ }` | every trait implementation |
| `rust` | `$X.unwrap()` | every `.unwrap()` call, whatever it is called on |
| `python` | `subprocess.run($$$, shell=True, $$$)` | shell-invoking subprocess calls |
| `typescript` | `fetch($URL, $$$)` | `fetch` call sites with any options |
| `go` | `if err != nil { return $$$ }` | early returns on error |
| `javascript` | `$EL.innerHTML = $VALUE` | direct `innerHTML` assignments |

The output is one `path:line:col: <first line of match>` line per match
(1-based, workspace-relative); no matches is empty output, not an error. A
malformed pattern or a bad path surfaces ast-grep's diagnostic as the error
rather than an empty result. Alongside the text, each call records up to 200
matches as structured data (file, range, full matched text, and every
metavariable's captured text), which the control plane uses to show a source
slice and a syntax-tree preview for each match. The syntax-tree half is
picked by file extension and covers Rust, Python, TypeScript, TSX, JavaScript,
Go and JSON files (`.rs`, `.py`, `.ts`, `.tsx`, `.js`/`.jsx`/`.mjs`/`.cjs`,
`.go`, `.json`); other files get no tree. Under a `concerns:` block, each matched file is recorded in
the coverage ledger like a `grep` hit.

### `permissionMode`

Valid values: `ask`, `bypass`, `readonly`.

Every tool declares one **effect**, and the mode decides each call from the
effect alone — never from the tool's name, so a new tool is gated correctly
the day it lands:

| Effect | Examples |
| --- | --- |
| `read` | `read_file`, `grep`, `glob`, `ast_grep`, `findings.query`, `coverage.status`, `scm.prs.get`, `goal.status`, `join` |
| `record` | rupu's own bookkeeping: `findings.report`, `findings.verify`, `findings.tag`, `assets.mark`, `coverage.mark`, `board.post`, `msg.send` |
| `write` | changes the workspace or runs code: `bash`, `write_file`, `edit_file` |
| `external` | acts outside this machine: `scm.prs.create`, `scm.branches.create`, `issues.comment`, `issues.create`, `github.workflows_dispatch` |
| `spawn` | starts another run or model call: `dispatch_agent`, `dispatch_agents_parallel`, `dispatch`, `run_workflow`, `workflows.generate` |

| Effect ↓ / Mode → | `bypass` | `ask` (interactive) | `ask` (no operator) | `readonly` |
| --- | --- | --- | --- | --- |
| read | allow | allow | allow | allow |
| record | allow | allow | allow | allow |
| write | allow | **prompt** | allow, with a notice | **deny** |
| external | allow | **prompt** | allow, with a notice | **deny** |
| spawn | allow; child keeps its own mode | allow; child capped at `ask` | allow; child capped at `ask` | allow; **child capped at `readonly`** |

- **A child never has more than its parent.** A sub-agent runs at the more
  restrictive of the parent's mode and its own `permissionMode`; a readonly
  run's children are readonly whatever their frontmatter says. An interactive
  parent's operator answers its children's prompts, one at a time.
- **`readonly` still records findings.** It means "no change to your
  workspace or the outside world"; rupu's own ledgers are bookkeeping, so a
  readonly reviewer can report, tag and verify findings.
- **`ask` without an operator** (a workflow step, a session turn, a flow, a
  detached run) cannot prompt, so it allows write/external calls and the
  transcript says so once with a `permission_mode_degraded` notice. An
  interactive `rupu run` in `ask` prompts; a non-TTY `rupu run` in `ask`
  is refused at startup.
- **"Allow always"** (`a` at the prompt) allows that one tool for the rest of
  the run, not every tool.

`ask` is the safest default for agents that edit code. Pass `--mode readonly`
for unattended runs that must not change anything.

#### Tool names

Tools are named `namespace.verb` (`findings.report`, `coverage.mark`); the
core fs/shell tools stay unqualified (`bash`, `read_file`, …). Earlier names
remain accepted forever, in `tools:` and when a model calls them, as aliases:

| Canonical | Also accepted |
| --- | --- |
| `findings.report` | `report_finding`, `findings.record` |
| `findings.verify` | `finding.verify` |
| `findings.query` / `findings.tag` | `query_findings` / `tag_findings` |
| `assets.mark` | `asset_mark` |
| `coverage.mark` / `coverage.status` / `coverage.remaining` | `coverage_mark` / `coverage_status` / `coverage_remaining` |
| `coverage.concerns.search` / `coverage.concerns.detail` | `coverage_concerns_search` / `coverage_concerns_detail` |
| `workflows.generate` | `generate_workflow` |
| `goal.coverage` (agentiflow lead) | `coverage.status`, inside a lead only |

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
| Anthropic — adaptive models (4.6+ and the Claude-5 family) | `thinking.type: adaptive` plus `output_config.effort` (`minimal`/`low`→`low`, `medium`→`medium`, `high`→`high`, `max`→`max`). `auto` sends adaptive with **no** effort knob (the server picks). |
| Anthropic — pre-4.6 and Haiku | `thinking.budget_tokens` — a token budget derived from the level (`auto` → adaptive on the OAuth path) |
| Gemini 3 | `generationConfig.thinkingConfig.thinkingLevel` (lowercase) |
| Gemini 2.5 and earlier | `generationConfig.thinkingConfig.thinkingBudget` (numeric) |
| OpenAI Codex | `reasoning.effort`, sent only for models that support reasoning (`max`→`xhigh`) |
| OpenAI-compatible endpoints | `reasoning_effort`, forwarded verbatim (`max`→`xhigh`) |

On the Anthropic adaptive path there is no `minimal` or `xhigh` on the wire:
`minimal` floors to `low` (the API has no "minimal") and both `max` and the
`xhigh` alias map to `max`. Every value rupu emits is valid on every adaptive
tier (4.6 accepts `low`/`medium`/`high`/`max`; 4.7+ also accept `xhigh`, which
this ladder never sends), so no per-model clamping is needed. Sending the old
`thinking.budget_tokens` shape to a 4.7+/Claude-5 model is a 400 — that is why
the adaptive path exists.

`auto` is special-cased: on Anthropic it sends adaptive thinking with no effort
knob, on Gemini it sends the `thinkingBudget: -1` sentinel ("model decides"),
and on the openai-compatible path it sends no `reasoning_effort` key at all.

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

### `thinkingDisplay`

Accepted values:

- `summarized` (default)
- `omitted`
- `updates`

Controls what reasoning the model returns alongside its answer — the
`thinking.display` knob. It is a display hint, honored **only by Anthropic
adaptive models** (4.6+ and the Claude-5 family); other providers and the
pre-4.6 `budget_tokens` path ignore it.

| Value | Effect |
| --- | --- |
| `summarized` | A readable summary of the reasoning is returned and captured into the transcript. This is the default when the field is omitted, so reasoning capture stays non-empty. |
| `omitted` | Thinking still runs (and is billed) but its text comes back empty — a quieter, cheaper transcript. |
| `updates` | Between-tool-call progress notes instead of full reasoning. Needs the `thinking-display-updates-2026-08-18` beta and a capable model (`claude-fable-5`, `claude-fable-5-1`, `claude-mythos-5-1`, `claude-opus-5-5`, `claude-sonnet-5-5`); on any other model it degrades to `summarized`. |

Only `auto`/explicit-effort requests on an adaptive model carry a `display` at
all. If you set `thinkingDisplay` on a model that cannot honor it — a pre-4.6
or Haiku Anthropic model, a non-Anthropic model, or `updates` on a
non-updates model — the run degrades to the model's default display and logs a
one-time warning naming the agent and model, rather than failing.

### `contextWindow`

Accepted values:

- `default`
- `1m`
- `1M`
- `one_million`

Use this sparingly. Most agents should let the model use its normal context window.

On Anthropic, `1m` opts an API-key account into the 1M-token context beta; an SSO account opts in with a `[1m]` suffix on `model:` instead, and the suffix is stripped from the model id sent to the API. Neither changes the input limit rupu discovers (`contextWindowTokens`, below). If the account has no extra-usage billing, Anthropic refuses such a request with a 429: rupu then stops sending the beta for the rest of that run, falls back to the standard 200,000-token window, writes a `model_limits_clamped` notice and retries once. Every new run starts with the beta again, so remove `contextWindow: 1m` (or `[1m]`) to skip the refused request. See [providers.md](providers.md#when-the-provider-rejects-a-request).

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

Selects the contract an agent's findings are recorded under. It governs the agent's `findings.report` builtin, whether the tool comes from a `concerns:` block (which injects the coverage tools) or from an explicit `tools: [findings.report]` grant.

- `full` (default) — findings must include a complete `report` (see `rupu findings schema`). The tool rejects `summary`, `severity`, and `evidence` as separate arguments: rupu derives them from the report (`summary` from `title`, `severity` from `rating.risk_rating`, `evidence.rationale` from `root_cause`). A rejected call lists every validation problem at once so the agent can fix them all in one retry. When the run can record findings (a `concerns:` block or `findings.report` in `tools:`), finding-writing guidance is appended to the system prompt. rupu generates the Markdown, HTML and PDF reports from the stored report (see [coverage.md](coverage.md#exporting-reports)), so an agent should not also write a report file.
- `summary` — the lightweight `summary` / `severity` / `evidence` record. A `report` sent under `summary` is refused rather than silently dropped.

A workflow step's `findings_profile` or the workflow's `defaults.findings_profile` overrides this value. The order is step → workflow defaults → agent `findingsProfile` → `full`; see [workflow-format.md](workflow-format.md#findings_profile). A standalone `rupu run <agent>` uses `--findings-profile full|summary` when given, then the agent's value, then `full`. The flag is also how a remote workflow step's profile reaches the host that runs the agent. Sub-agents started through `dispatch_agent` resolve only from their own agent file.

**Upgrade note:** because the built-in default is `full`, an existing agent that records thin findings (a `concerns:` block, or `findings.report` in `tools:`) is now rejected unless it sets `findingsProfile: summary` (or its workflow sets `findings_profile: summary`), or its prompt is updated to send a complete `report`.

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

### `fallbacks`

An ordered list of models to try when the agent's own model cannot give a usable reply or the provider fails the request (a refusal, repeated truncation, an overloaded provider). Each entry has a `model` and an optional `provider`; an entry without a `provider` means the provider the run started on.

```yaml
fallbacks:
  - model: claude-sonnet-5-5        # same provider as the agent
  - provider: openai-codex          # another provider
    model: gpt-5.6-cyber
```

The list replaces `[recovery].fallbacks` from `config.toml` for this agent; it is not merged with it, and `fallbacks: []` means no fallbacks. Unknown keys inside an entry are an error. A rupu release that predates `fallbacks:` refuses to load an agent file that sets it, so upgrade remote hosts before adding it. A cross-provider entry sends the conversation to that provider; see [response-outcomes.md](response-outcomes.md#3-configuring-fallbacks) for the ladder, the rung split and that data-handling note.

### `maxTokens`

Per-request output-token cap (the LLM request's `max_tokens`). You rarely need to set it: rupu discovers each model's real output cap from the provider's model list, where the provider reports one (Codex/OpenAI and OpenAI-compatible servers report none), and sends that on every turn. A `maxTokens` value overrides the discovered cap. When neither is known, Anthropic requests carry `8192` (the API requires a cap) and every other provider gets no cap, so the model's own maximum applies. Extended thinking (`effort`) draws from this same budget, so a low `maxTokens` can starve an agent that both reasons heavily and produces long output.

On Anthropic, the `max` effort level's thinking budget scales with the output cap (cap − 2,000 tokens), so a discovered 64K cap raises both the budget and its cost; pin a smaller `maxTokens` to bound it. A fixed thinking budget is always kept at least 1,024 tokens below the cap (the API requires the budget to be smaller), and thinking is skipped when the cap leaves no room for the 1,024-token minimum. A `maxTokens` of `0` is ignored. `--no-stream` only changes display — requests still stream on the wire, so the full discovered output cap applies. Only a `stream = false` OpenAI-compatible server (one that genuinely cannot stream) carries `max_tokens: 8192` when no cap is known, instead of omitting the field.

### `contextWindowTokens` and `compactAtPercent`

`contextWindowTokens` is the model's input-token limit, used for proactive context compaction. rupu discovers it from the provider's model list; set it only to override the discovered value (for example to compact earlier than the model requires, or on a provider that reports no limits). Compaction is off only when neither a pin nor a discovered value is available.

`compactAtPercent` (default `80`, clamped to `[10, 95]`) is the share of the input limit at which the runner summarizes older turns before the next turn. When the output cap is known and the model's output counts against the same window as its input (Anthropic, Codex, OpenAI-compatible), the threshold is the lower of that percentage and the input limit minus the output cap, so a full-length reply still fits. Otherwise the threshold is simply `input × compactAtPercent / 100`: Copilot and Gemini budget input and output independently, and an unknown output cap (including the Anthropic `8192` wire fallback, which is not a discovered limit) gives no headroom to subtract.

Where the limits come from, per field, in precedence order:

1. the agent's `contextWindowTokens` / `maxTokens`;
2. a `[[providers.<name>.models]]` entry in `config.toml` (value greater than zero);
3. the provider's live model list, cached for 1 hour in `~/.rupu/cache/models/` (see [providers.md](providers.md#model-resolution) for each provider's source);
4. unknown.

A pin of `0` is ignored (the notice says `ignored contextWindowTokens: 0`), and so is a `0` in a config entry.

Every run writes a `model_limits` notice to its transcript stating the input limit, output cap and compaction threshold it will use and where each came from, including an explicit "unknown" and the reason when a limit could not be discovered. If the model-list fetch failed or timed out, runs don't retry it for 5 minutes (the notice says so; `rupu models refresh` ignores the pause).

If the provider later rejects a request as too long and reports the real maximum, rupu lowers the input limit to that value for the rest of the run (a `model_limits_clamped` notice), compacts with a summary, and retries. It recognizes the rejection formats of Anthropic, OpenAI, Copilot, vLLM and Gemini. When the input limit is known, rupu compacts once per turn this way even if the error carries no number; dropping the oldest turns is the last resort. Anthropic's older "input length and `max_tokens` exceed context limit" error means the output reservation, not the input, is too big: rupu retries once with a lower `max_tokens` for that request and leaves the input limit alone. See [providers.md](providers.md#when-the-provider-rejects-a-request).

A session resolves its limits on its first turn and keeps them, including a lowered limit, for later turns, even when the turn that learned it failed (a first turn that found no limit at all is resolved again on the next turn).

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

---

## Continuing an interrupted run

If a run stops before it finishes — a closed terminal, a killed process, a
crash — pick it up where it left off instead of starting over:

    rupu run <agent> --continue <agent_run_id>

rupu rebuilds the conversation from the run's transcript, drops the turn that
was in flight when it stopped, and tells the agent it was interrupted (work
from that turn may be partially applied, so the agent re-checks before going
on). The new run's transcript links back to the old one rather than copying
it. If the run had actually finished, its recorded answer is printed without
calling the model; a run that failed can't be continued — start it fresh.
Run the command from the same project as the original run. The new run needs
its own run id: a `--run-id` that already has a transcript (the run being
continued, or any other run) is refused rather than overwritten.

A run that failed on a provider-side outcome — a refusal, a reply that kept
getting cut off — with no fallback left ends with a hint naming this command.
Its conversation is intact, so it can be continued on another model:

    rupu run <agent> --continue <agent_run_id> --model <model> [--provider <provider>]

The agent is told the earlier attempt stopped and why, and which model it is
continuing on. `--model` and `--provider` override the agent's own `model:` /
`provider:` for any `rupu run`, not only a continuation. A run that failed for
another reason (max turns, say) still can't be continued this way.
