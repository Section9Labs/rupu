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
| `dispatchableAgents` | array\<string\> | no | none (no dispatch) | Allowlist of agent names this agent may start with `dispatch` (kinds `subagent` and `agent`, and the legacy `dispatch_agent` / `dispatch_agents_parallel`) |
| `dispatchableWorkflows` | array\<string\> | no | none | Allowlist of workflow names this agent may start with `dispatch {kind: workflow}` (and the legacy `run_workflow`) |
| `messaging` | bool | no | `true` | `false` turns the messaging tools (`board.*`, `msg.send`) off for this agent wherever it runs; naming one in `tools:` as well is a load error (see [messaging.md](messaging.md#turning-it-off)) |
| `concerns` | object | no | none | Coverage-concerns block; injects the coverage tools + catalog into the system prompt |
| `findingsProfile` | `full` \| `summary` | no | `full` | Findings contract for `findings.report`; a workflow step's `findings_profile` or the workflow's `defaults.findings_profile` overrides it |
| `fallbacks` | array of `{model, provider?}` | no | `[recovery].fallbacks` from config, else none | Ordered fallback models tried when a reply cannot be used (a refusal, say). Wins over the config table; not merged with it. See [response-outcomes.md](response-outcomes.md#3-configuring-fallbacks) |
| `maxTokens` | integer | no | discovered output cap | Per-request output-token cap. Overrides the cap discovered from the provider's model list; when neither is known, Anthropic gets `8192` and other providers get no cap (model max). `0` is ignored. Extended thinking (`effort`) draws from this budget |
| `contextWindowTokens` | integer | no | discovered input limit | Input-token limit used for proactive compaction. Overrides the discovered limit; compaction is off only when neither is known. `0` is ignored |
| `skills` | array\<string\> | no | none | Skills the model may load with `skills.load`; see [`skills` and `preloadSkills`](#skills-and-preloadskills) |
| `preloadSkills` | array\<string\> | no | none | Skills whose whole body goes into the system prompt; implies granted |
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

`tools:` is the agent's tool grant: what its model may call. Each entry is
one of:

| Entry | Means |
|---|---|
| `bash`, `findings.report`, `scm.prs.get`, … | that tool, by its canonical name |
| `report_finding`, `query_findings`, … | a legacy name, accepted forever as an alias of its canonical tool (see [Tool names](#tool-names)) |
| `findings.*`, `scm.*`, `issues.*`, `board.*`, … | every tool in that namespace |
| `core.*` | the core tools: `bash`, `read_file`, `write_file`, `edit_file`, `grep`, `glob`, `ast_grep` |
| `*` | the whole catalog, except the authority tools `board.directive` and `board.retract` (see below) |

There are no other wildcards (`scm*`, `coverage_*` are errors).

**Authority is never implied by `*`.** `board.directive` and `board.retract`
let one agent direct the others in an agentiflow, so `*` leaves them out: an
agent gets them only by naming them (or `board.*`), and an agentiflow lead
gets them from its origin grant. `board.retract` lifts only directives the
caller wrote itself.

**An unknown name is an error.** An agent whose `tools:` names a tool rupu
doesn't know fails to load — `rupu run`, a workflow step, a sub-agent dispatch
all refuse it — with a suggestion when a known name is close:

```text
agent `reviewer`: unknown tool "repot_finding" in tools: (did you mean "report_finding" → findings.report?)
```

Check your agents with `rupu agent validate <name>` or `rupu agent validate
--all` (every file in the global and project agent dirs; exits nonzero if any
fail). The CP's agent page shows the same error.

**A tool is offered only where it works.** Some tools need something the run
may not have: the `scm.*` / `issues.*` / `github.*` / `gitlab.*` connector tools
need configured SCM connectors, `coverage.*` needs a `concerns:` block,
`assets.mark` an engagement profile, the board and mailbox tools (`board.*`,
`msg.send`) a message space, which every run has when its agent names one of
them, its workflow sets `messaging:`, or it is in an agentiflow or a parent's
space (see [messaging.md](messaging.md)), `goal.*` and `budget.status` an
agentiflow lead, and `workflows.generate` a provider to author with. `dispatch`
and `join` work in every agent run: `rupu run`, a workflow step, a session
turn, a sub-agent, an agentiflow lead or unit. The roster tools (`agents.list`,
`agents.get`, `workflows.list`, `workflows.get`, `catalog.search`) read the
agent and workflow files and work in every run. A wildcard
silently leaves out a tool the run can't serve. A tool you name exactly that
the run can't serve is left out too, and the run says so with a
`tool_unavailable` notice in its transcript.

**Omitting `tools:`** gives the default grant: the core tools, the launch
pair and every connector tool (`core.*`, `dispatch`, `join`, `scm.*`,
`issues.*`, `github.*`, `gitlab.*`). For reusable agents an explicit `tools:`
is better than the default.

**Ambient grants.** The run's context adds tools the agent didn't list:
a `concerns:` block adds the coverage tools and `findings.report`; an active
engagement profile adds `findings.report` and `assets.mark`; an agentiflow
lead's or unit's origin adds its coordination tools. A run granted `dispatch`
(by any route, including a legacy name like `dispatch_agent`) also gets
`join`, since `dispatch` returns before its child ends. They're never hidden:
every run's transcript opens with a `tool_grant` event listing exactly the
tools the model was offered and why (`declared`, `declared:scm.*`, `default`,
`ambient:concerns`, `ambient:engagement`, `origin:flow_lead`,
`origin:flow_unit`, `implied:dispatch`), and every call is audited
(`tool_audit`). See [transcript-schema.md](transcript-schema.md#tool_grant).

`dispatch` starts child runs: agents named in this agent's
`dispatchableAgents:` list and workflows named in its `dispatchableWorkflows:`
list (see below). A call naming anything else fails with
`agent_not_dispatchable` or `workflow_not_dispatchable`. See
[Launching children](#launching-children).

`tools:` is not the same thing as a workflow step's `actions:`. `tools:` is
this agent's full grant; `actions:` (on a workflow step) can only narrow the
connector subset of that grant for that one step — it never touches any other
tool, and never grants a tool beyond what `tools:` already allows. `*` means
the same thing with or without `actions:`.

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
| `spawn` | starts another run or model call: `dispatch` (and its aliases `dispatch_agent`, `dispatch_agents_parallel`, `run_workflow`), `workflows.generate` |

| Effect ↓ / Mode → | `bypass` | `ask` (interactive) | `ask` (no operator) | `readonly` |
| --- | --- | --- | --- | --- |
| read | allow | allow | allow | allow |
| record | allow | allow | allow | allow |
| write | allow | **prompt** | allow, with a notice | **deny** |
| external | allow | **prompt** | allow, with a notice | **deny** |
| spawn | allow; child keeps its own mode | allow; child capped at `ask` | allow; child capped at `ask` | allow; **child capped at `readonly`** |

- **A child never has more than its parent.** A child of any kind runs at
  the more restrictive of the parent's mode and its own `permissionMode`; a
  readonly run's children are readonly whatever their frontmatter says. This
  holds across a process boundary too: an `agent` or `workflow` child gets the
  parent's mode as its ceiling. An interactive parent's operator answers its
  in-process sub-agents' prompts, one at a time.
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
| `dispatch` | `dispatch_agent`, `dispatch_agents_parallel`, `run_workflow` (see [Legacy launch names](#legacy-launch-names)) |
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

Allowlist of agent names this agent may start with `dispatch`, as a `subagent` or an `agent` child (and through the legacy `dispatch_agent` / `dispatch_agents_parallel`). Omit it (the default) to give the agent no agents to dispatch: `dispatch` is still offered, but a call naming an agent fails with `agent_not_dispatchable`. Inside an agentiflow the flow's pool applies instead, intersected with this list when the agent declares both. A child's own list is also capped by what its parent may launch. See [Launching children](#launching-children).

### `dispatchableWorkflows`

Allowlist of workflow names this agent may start with `dispatch {kind: workflow}` (and the legacy `run_workflow`). Omit it to allow none; a call naming a workflow then fails with `workflow_not_dispatchable`. It follows the same pool and parent-cap rules as `dispatchableAgents`. A workflow child must also only run agents this agent may dispatch.

### `messaging`

Every run with a message space is offered `board.claim`, `board.release`, `board.post`, `board.read` and `msg.send` by default. `messaging: false` turns them off for this agent wherever it runs: as a standalone run, a workflow step, a sub-agent, a session's agent or an agentiflow's lead or unit. The run is still in its space, so the operator and its children reach it, and every child it launches has messaging off too. A file that sets `messaging: false` and also names a messaging tool in `tools:` (by name, alias or `board.*` / `msg.*`) fails to load. See [messaging.md](messaging.md#turning-it-off).

### `concerns`

Coverage-concerns block (see `docs/coverage.md`). When present, the runner flattens the concern catalog, writes a snapshot to `.rupu/coverage/<target>/catalog.yaml`, injects the four coverage tools, and prepends the catalog to the system prompt. A workflow step's own `concerns:` block takes precedence over the agent's when both are set.

### `findingsProfile`

Selects the contract an agent's findings are recorded under. It governs the agent's `findings.report` builtin, whether the tool comes from a `concerns:` block (which injects the coverage tools) or from an explicit `tools: [findings.report]` grant.

- `full` (default) — findings must include a complete `report` (see `rupu findings schema`). The tool rejects `summary`, `severity`, and `evidence` as separate arguments: rupu derives them from the report (`summary` from `title`, `severity` from `rating.risk_rating`, `evidence.rationale` from `root_cause`). A rejected call lists every validation problem at once so the agent can fix them all in one retry. When the run can record findings (a `concerns:` block or `findings.report` in `tools:`), finding-writing guidance is appended to the system prompt. rupu generates the Markdown, HTML and PDF reports from the stored report (see [coverage.md](coverage.md#exporting-reports)), so an agent should not also write a report file.
- `summary` — the lightweight `summary` / `severity` / `evidence` record. A `report` sent under `summary` is refused rather than silently dropped.

A workflow step's `findings_profile` or the workflow's `defaults.findings_profile` overrides this value. The order is step → workflow defaults → agent `findingsProfile` → `full`; see [workflow-format.md](workflow-format.md#findings_profile). A standalone `rupu run <agent>` uses `--findings-profile full|summary` when given, then the agent's value, then `full`. The flag is also how a remote workflow step's profile reaches the host that runs the agent. Children started with `dispatch` resolve only from their own agent file.

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

### `skills` and `preloadSkills`

Grant the agent [Agent Skills](skills.md) installed under `~/.rupu/skills/` or the project's `.rupu/skills/`. Installing a skill gives it to no agent: an agent without these keys has no skills, no skill index and no `skills.load` tool.

```yaml
skills: [pdf, code-review]     # listed in the prompt; the model loads one when a task matches it
preloadSkills: [house-style]   # its whole body is in the system prompt from the start
```

Each entry is one of:

| Entry | Grants |
|---|---|
| `code-review` | the skill of that name in whichever layer has it. If both the global and the project layer have it, the agent fails to load until the entry names a layer. |
| `global:code-review`, `project:code-review` | that layer's skill |
| `*` | every valid global skill (never a project skill) |
| `project:*` | every valid project skill |

A run binds each name once, so granting the same name from both layers (`global:x` and `project:x`, or `*` and `project:*` over a name in both) is an error. An unknown name, an invalid skill or a malformed entry fails the agent's load with a did-you-mean, and `rupu agent validate` reports the same. A skill deleted after the agent loaded fails the run at launch, with the directories rupu looked in.

A granted skill reaches the run as:

- **The index.** Each skill in `skills:` is listed under `## Skills` in the system prompt, after the agent's body and any `## Run target`, with its description. The model calls `skills.load` with a name to get the skill's instructions, base directory and file list.
- **Preloaded bodies.** Each skill in `preloadSkills:` is rendered in full under `## Skill: <name>`, ahead of the index, and is not listed in it.
- **Its files.** `read_file`, `grep`, `glob` and `ast_grep` may read under a granted skill's directory even outside the workspace; `write_file` and `edit_file` may not. A skill's scripts run through `bash`, under the run's permission mode.

A skill never widens the agent's `tools:` grant or its permission mode. A skill whose `allowed-tools` names a tool the run lacks still loads, with a `skill_tools_missing` notice. A skill marked `disable-model-invocation: true` is reachable only by preloading it; granted in `skills:` alone, it writes a `skill_not_indexed` notice, and when no granted skill is loadable the run gets no `skills.load`.

A child started with `dispatch` (any kind, or a legacy name like `dispatch_agent`) gets its own agent file's skills, never its parent's. A session fixes its agent's skills when it starts (see [skills.md](skills.md#skills-in-sessions)). A rupu release that predates these keys refuses to load an agent file that sets them, so upgrade remote hosts first.

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

## Launching children

An agent with `dispatch` in its grant can start child runs and keep working
while they run. `join` collects their results.

### `dispatch`

| Field | Meaning |
| --- | --- |
| `kind` | `subagent` (default), `agent` or `workflow`, below |
| `name` | The agent, or for kind `workflow` the workflow, to run |
| `file` | Kind `workflow` only, instead of `name`: a file `workflows.generate` returned in this run |
| `prompt` | An agent child's task. Required for `subagent` and `agent` |
| `inputs` | A workflow's declared inputs (strings, numbers or booleans). For an agent child, structured inputs appended to its prompt as a JSON block |
| `wait` | `true` starts the child and waits for it in one call. Default `false` |
| `timeout_secs` | With `wait`: how long to wait. Default 300, at most 3600 |

| `kind` | Runs as | Its record |
| --- | --- | --- |
| `subagent` | a task in the parent's process; it dies with the parent | transcript under the parent's run: `<RUPU_HOME>/runs/<parent run id>/sub/<sub_id>/transcript.jsonl` |
| `agent` | its own `rupu run` process | its own run and transcript, like any `rupu run` |
| `workflow` | its own `rupu workflow run --plain` process | its own workflow run under `<RUPU_HOME>/runs/<id>/` |

`dispatch` returns at once with `{handle, kind, name, run_id, codename}`, plus
`transcript` for an agent child and `address` for a process child inside an
agentiflow. With `wait: true` the reply also carries `result`, the child's
`join` entry.

A workflow child is checked before anything starts: it parses, has no approval
gate and no `host:` / `distribute:` step, every agent its steps run is one the
caller may dispatch, and its inputs resolve. The child is handed the workflow
file that was checked, and refuses to start if a step's agent is outside what
its parent may dispatch. `workflows.generate {description}`
writes a new workflow under `<RUPU_HOME>/runs/<run id>/generated/`, applies the
same checks, and returns `{file, name, steps, inputs}`. It does not start it:
run it with `dispatch {kind: "workflow", file}`.

### `join`

| Field | Meaning |
| --- | --- |
| `handles` | Handles `dispatch` returned. Omit for every child of this run that `join` has not yet returned finished |
| `timeout_secs` | How long to wait. Default 300, at most 3600; `0` just looks |
| `until` | `all` (default) returns when every named child has ended; `any` when one has |

It returns `{children: [...]}`, one entry per child: `handle`, `kind`, `name`,
`run_id`, `codename`, `status`, and once it ended `success`, `output`, `error`,
`cause`, `tokens` and `duration_ms`. `status` is `pending` (waiting for a slot,
or not started yet), `running`, `done` (it answered; `success` says whether it
succeeded), `failed` (it could not start, crashed or was killed), `abandoned`
or `interrupted` (its parent paused, crashed or was stopped by a signal; see
[When the parent ends](#when-the-parent-ends)). `tokens` covers every run of
the child: a child continued after its parent's pause or crash counts its
earlier run's tokens too.
A child still running when the timeout passes is reported as it is; join again
to keep waiting.

While a run has children it has not joined, each turn starts with a status
block (`2 children not joined ...`, then one line per child), so the agent
knows what is still out without polling `join`.

### Rules every kind shares

- **Allowlist.** The agents and workflows a run may start are its
  `dispatchableAgents` / `dispatchableWorkflows`. Inside an agentiflow the
  flow's pool applies, intersected with the declared lists when the agent has
  them. Either way the result is capped by what the parent may launch, so a
  child can never reach outside its parent's set. Neither declared nor in a
  flow means none.
- **Depth.** A run you start is depth 0 and each child is one deeper, in the
  same process or across one. A run at depth 5 can't start a child
  (`max_dispatch_depth_exceeded`).
- **Permission ceiling.** A child runs at the more restrictive of the parent's
  mode and its own `permissionMode` (see [`permissionMode`](#permissionmode)).
  A sub-agent of an `ask` parent asks the parent's operator. A process child
  can't reach that prompt, so under an interactive `ask` parent it runs
  `readonly`; under an unattended `ask` parent (a workflow step, another
  launched run) it runs `ask` unattended.
- **Connector narrowing.** A workflow step's `actions:` narrowing reaches the
  step's children: a child can't call a connector tool its parent couldn't,
  and neither can a launched workflow's `action:` steps. A generated workflow
  may call a connector tool from an `action:` step only when the run that
  generates or starts it is offered that tool itself, narrowed or not.
- **Concurrency.** At most `[dispatch].max_concurrent_children` (default 8)
  children of one run run at once; the rest wait as `pending`. A process child
  that gets a slot has started by the time `dispatch` returns. Sub-agents also
  wait for machine admission (`[runtime].max_concurrent_jobs`). See
  [configuration.md](configuration.md#dispatch).
- **Engagement.** Children run under the parent's engagement profiles. A
  child's findings profile comes from its own agent file (or, for a workflow,
  its own defaults), not from the parent's step.
- **Usage.** A sub-agent's calls are recorded in its parent's usage ledger.
  A process child keeps its own ledger and also copies each call into the
  ledger of the run at the root of the launch tree, marked `rollup: true`. An
  agentiflow's budget counts those copies; the control plane's usage reports
  skip them, so nothing is counted twice.

### When the parent ends

A run's unjoined children are stopped when the run ends. A sub-agent stops at
its next safe boundary (it is dropped after 30 s, and the children it
launched are stopped with it). A process child's process group gets SIGTERM,
then SIGKILL 12 s later. Each is recorded `abandoned`, and so is whatever it
left below it: nothing will continue it.

A workflow step that pauses (`rupu workflow pause`) stops its unjoined
children the same way, queued ones included, but records each `interrupted`.
A workflow child's run is left as it stands, to be resumed, and is not
recorded cancelled the way an operator's cancel records it. Nothing is left
running across the pause, so a reboot before the resume loses nothing.

A `rupu run` or `rupu workflow run` stopped by SIGTERM or SIGINT (a cancel, a
Ctrl-C, a reboot) does the same before it dies: SIGTERM to each child's
group, up to 5 s, then SIGKILL, each recorded `interrupted`. From that moment
it starts nothing: a child still waiting for a slot is recorded `interrupted`
rather than taking the slot a stopped one frees, and a `dispatch` is refused
(`stopping`). A process child stopped that way does the same for its own
children, so the whole launch tree winds down, and continuing the top of it
continues every level.

When a run's conversation is continued, the continued run takes on every
child the earlier attempt was given before its first turn, under the same
handles, so the conversation's `dispatch` replies still name them. That
happens when `rupu workflow resume` continues a paused or interrupted linear
step or `for_each` unit from its transcript, and on
`rupu run <agent> --continue <run id>`.

- A child that ended is restored as it ended, with its answer. One that
  `join` already collected stays collected: `join {}` skips it.
- A child recorded `failed` whose own run says a signal or its runner's
  death ended it is continued like an interrupted one, unless `join` already
  collected it. A reboot signals every process at once, so a child can record
  its own end before its parent records the stop. The run says so in its
  typed cause, never in its message: a workflow run recorded `failed` while
  its process was being stopped by SIGTERM or SIGINT has the cause
  `stopped_by_signal`, one the reaper finalized because its runner process
  had died has `runner_died`, and an agent run stopped by SIGTERM closes its
  transcript `aborted` with the cause `stopped_by_signal`.
- An interrupted child whose own records say it ended on its own (its
  workflow run failed before the stop for a reason of its own, was cancelled
  by an operator or completed; its agent run answered or failed) is restored
  with that end, and is never run again.
- An interrupted child that never started (it was still waiting for a slot,
  or its process was stopped before it wrote its transcript or its run
  record) is launched afresh with what it was launched with, under the same
  handle and codename and a new run id.
- Any other interrupted child is continued from its own records. An `agent`
  child runs as `rupu run <agent> --continue <its run id>`, under a new run id
  with the same handle and codename. A `workflow` child runs as
  `rupu workflow resume <its run id>`. A sub-agent runs in the parent's
  process, seeded from its transcript. One that can't be continued (its
  transcript ends on an unanswered tool call, say) is restored `failed` with
  the reason.
- A continued or relaunched child runs no wider than it was launched: at the
  lower of its original ceiling and the continuing parent's, with only the
  agents, workflows and connector tools both allow.
- The earlier attempt's record of each child is closed (`continued`), so the
  control plane lists each child once, as its latest record says.

A resume that starts the step over instead (`--restart-interrupted`, or a
step that can't be continued) records the earlier attempt's unfinished
children `abandoned` ("its run restarted"). A workflow run that ends for good
(other than paused or stopped by a signal) records whatever its attempts left
`interrupted` `abandoned`.

If the parent's process dies without stopping its children (SIGKILL, a
crash, a power loss), nothing is recorded, and a process child keeps
running. Continuing the parent's conversation first stops what is still
running and records it `interrupted`, or records the end a child's own
records give when it ended on its own after the crash. The orphan reaper
(`rupu agentiflow serve`, or `rupu cp serve`'s sweep;
`[agentiflow].reaper_enabled`) does the same when it sees the launching
process gone, and records a child `abandoned` only once nothing can continue
it any more: its parent's own run record, or that of the run at the root of
its launch tree, says that run ended (a workflow run that failed can still be
resumed, and a `rupu run` stopped by a signal can be continued, so neither
counts), or it belongs to an agentiflow. A crashed `rupu run`'s children
wait, `interrupted`, for `rupu run --continue`.

Nothing is signalled on a recorded pid alone: the pid's process must have
started when the recorded child did. A child whose start time couldn't be
read is never signalled, nor taken for running. Once a crashed child's
process is gone, its process group's id could have been taken by a
stranger's, so what is left in the group is signalled only while one of its
processes carries the child's run marker. A launcher starts every process
child with `RUPU_RUN_ID=<its run id>` in its environment, which everything
it starts inherits (`bash` always forwards it). The sweep reads each group
member's environment (`/proc/<pid>/environ` on Linux, `ps -E` on macOS) and
signals the group only when one holds that exact entry; otherwise the group
is left alone and a warning is logged. macOS doesn't show another process
the environment of Apple's own binaries (`/bin/sh`, `/bin/sleep`), so a
group left with only those is not signalled there. A child already recorded
`interrupted` was stopped already and is
not signalled again. A child whose task crashed in its parent's process has
its process stopped and is recorded `failed`. A child is recorded ended once:
of two stoppers, the first one's record stands. An `interrupted` record is
the exception, followed by one more end.

### Where children are recorded

Every launch appends to the parent's children ledger,
`<RUPU_HOME>/runs/<run id>/children.jsonl` (an agentiflow lead's is
`<flow dir>/children.jsonl`): a `started` line as it is launched (handle,
kind, name, run id, codename, transcript, depth, what it was launched with,
the earlier run it continues and the tokens its earlier runs used), a
`spawned` line for a process child (pid and start time), a `finished` line
(status, success, answer, error, cause, tokens, duration), a `joined` line
once `join` has collected it, and a `continued` line once a later run took it
on. `join`, the
orphan reaper, an agentiflow's unit list and the control plane read it. The
control plane's workflow run page has a **Children** tab listing every child
the run's steps launched, nested under the run that launched it
(`GET /api/runs/:id/children`; local runs only). Runs recorded before the
ledger existed show their sub-agents from their `sub/` directories.

Each child also gets a codename below its parent's (`<parent>><role>#n`). A
workflow run's `events.jsonl` records `dispatch_started` / `dispatch_completed`
for its steps' children, with the child's `kind`.

A sub-agent can be continued like any run, by its sub-run id:
`rupu run <agent> --continue <sub_id>` (see
[Continuing an interrupted run](#continuing-an-interrupted-run)).

### Legacy launch names

The tools that predate `dispatch` keep working, in `tools:` and when a model
calls them, with their old behavior and output shapes:

| Old call | Does | Returns |
| --- | --- | --- |
| `dispatch_agent {agent, prompt, inputs?}` | a `subagent`, waited for | `{ok, agent, output, findings, tokens_used, duration_ms, transcript_path, sub_run_id, codename, error?}` |
| `dispatch_agents_parallel {agents: [{id, agent, prompt, inputs?}], max_parallel?}` | one `subagent` per entry, all checked before any starts, at most `max_parallel` at once, all waited for | `{ok, results: {<id>: ...}, all_succeeded}` |
| `run_workflow {workflow, inputs?}` | a `workflow` child, not waited for | `{handle, participant}` |
| `generate_workflow {description, inputs?}` | `workflows.generate`, then starts it | `{handle, participant, generated_file}` |
| `join {handle}` | `join` on one child | `{status, success?, output?}`, or `{status: "failed", error}` (an abandoned child reads `failed`) |

A model only sees `dispatch`, so a prompt written for an old name arrives as a
`dispatch` call in the old shape. An `agents` array is
`dispatch_agents_parallel`. An `agent` field without `kind` is the agentiflow
`dispatch {agent, prompt}` inside a flow (an `agent` child, `{handle,
participant}` back) and `dispatch_agent` everywhere else.

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
The run's command is restored too: its repo target and every flag it was
launched with (mode, model, provider, findings profile, engagement, skills and
the rest, recorded as `launch_args` on its transcript's `run_start`). A flag
given on the continue replaces only its own setting, and a repeatable flag
(`--skill`, `--engagement-profile`) replaces its whole list. A repo target
reuses the checkout the original run cloned when it is still there (`--tmp`
clones afresh). Run the command from the same directory as the original run. A sub-agent's
`sub_…` id works too: rupu finds its transcript under the run that dispatched
it. The new run needs
its own run id: a `--run-id` that already has a transcript (the run being
continued, or any other run) is refused rather than overwritten.

The continued run also takes on the children the interrupted one launched,
under the same handles: finished ones as they ended, the rest continued (see
[When the parent ends](#when-the-parent-ends)).

A run that failed on a provider-side outcome — a refusal, a reply that kept
getting cut off — with no fallback left ends with a hint naming this command.
Its conversation is intact, so it can be continued on another model:

    rupu run <agent> --continue <agent_run_id> --model <model> [--provider <provider>]

The agent is told the earlier attempt stopped and why, and which model it is
continuing on. Only a `--model` or `--provider` given on the continue does
this: the ones a bare `--continue` restores do not. `--model` and `--provider` override the agent's own `model:` /
`provider:` for any `rupu run`, not only a continuation. A run that failed for
another reason (max turns, say) still can't be continued this way.
