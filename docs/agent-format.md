# Agent File Format Reference

> Part of the rupu reference docs: [spec.md](spec.md) · **agent-format.md** ·
> [workflow-format.md](workflow-format.md) · [transcript-schema.md](transcript-schema.md)

---

## Overview

An agent file is a Markdown document with YAML frontmatter. The frontmatter configures how the
agent runs (provider, model, tools, permission mode, turn limit). The body — everything after the
closing `---` — is used verbatim as the LLM system prompt.

---

## File location

```
<dir>/agents/<name>.md
```

Where `<dir>` is one of:

- `~/.rupu/` — global agent library; available in every workspace.
- `<project>/.rupu/` — project-local agents; override globals with the same `name`.

Resolution: when two files share the same `name:` value, the project-local file wins entirely
(no frontmatter merging). `rupu agent list` labels each entry `(global)` or `(project)`.

---

## Frontmatter fields

The frontmatter is parsed with `#[serde(deny_unknown_fields)]`. Unknown keys — including
misspellings like `permision_mode` or `max_turns` — are rejected at parse time with a clear
error message.

### `name`

| Attribute | Value    |
|-----------|----------|
| Type      | string   |
| Required  | yes      |

The agent's unique name within its scope (global or project). Used in:

- `rupu run <name> [prompt]`
- `rupu agent show <name>`
- `agent:` field in workflow step definitions

Names are case-sensitive. Convention: lowercase, hyphen-separated (`fix-bug`, `add-tests`).

---

### `description`

| Attribute | Value    |
|-----------|----------|
| Type      | string   |
| Required  | no       |

One-line human-readable description. Displayed by `rupu agent list` and `rupu agent show`.

---

### `provider`

| Attribute | Value       |
|-----------|-------------|
| Type      | string      |
| Required  | no          |
| Default   | `anthropic` |

LLM provider to use for this agent. Currently only `anthropic` is fully wired in v0. Other
values (`openai`, `copilot`, `local`) are accepted at parse time but return a
`"not wired in v0"` error at runtime.

Mode resolution (highest precedence first): CLI flag → agent frontmatter → project config →
global config → `anthropic`.

---

### `model`

| Attribute | Value                |
|-----------|----------------------|
| Type      | string               |
| Required  | no                   |
| Default   | `claude-sonnet-4-6`  |

Model identifier passed to the provider. Must be a model the chosen provider supports. rupu does
not validate the model string at parse time; invalid values surface as provider errors at
runtime.

---

### `tools`

| Attribute | Value                                                   |
|-----------|---------------------------------------------------------|
| Type      | array\<string\>                                         |
| Required  | no                                                      |
| Default   | all six: `[bash, read_file, write_file, edit_file, grep, glob]` |

Subset of the six v0 tools that this agent is allowed to call. Tools omitted from this list are
not registered with the provider for this agent; the LLM never sees them as options.

Available tool names: `bash`, `read_file`, `write_file`, `edit_file`, `grep`, `glob`.

To create a read-only agent at the tool level (regardless of `permissionMode`), omit the
write tools: `tools: [bash, read_file, grep, glob]`.

---

### `maxTurns`

| Attribute | Value  |
|-----------|--------|
| Type      | u32    |
| Required  | no     |
| Default   | `50`   |

Maximum number of agent turns before the run is aborted with `run_complete { status: "error",
error: "max_turns_reached" }`. One turn is one round-trip to the LLM (user message →
assistant response). A single turn may contain multiple tool calls.

---

### `permissionMode`

| Attribute | Value  |
|-----------|--------|
| Type      | string |
| Required  | no     |
| Default   | `ask`  |

Controls how the agent may affect the filesystem and shell. One of:

| Value      | Effect                                                           |
|------------|------------------------------------------------------------------|
| `ask`      | Prompt the user before `bash`, `write_file`, `edit_file`         |
| `bypass`   | Execute all tools without prompting                              |
| `readonly` | Allow `read_file`, `grep`, `glob`; deny `bash`, `write_file`, `edit_file` |

Non-TTY + `ask` aborts the run before the first tool call. Silently degrading to `bypass` in
CI is how accidents happen; rupu refuses to do it.

Denied tool calls return a `tool_result` with `error: "permission_denied"`. The agent sees this
and can adapt (e.g., switch to investigation-only behavior).

---

## Body (system prompt)

Everything after the second `---` delimiter is the system prompt. It is passed to the LLM as
the `system` message before the user turn. Standard Markdown is fine; rupu passes the raw text
without rendering it.

There is no length limit enforced by rupu, but very large system prompts consume context window.

---

## Worked examples

### Minimal agent (name + body only)

```markdown
---
name: summarize-diff
---

You are a code reviewer. When given a git diff, produce a concise summary
of what changed and why it matters. Use bullet points. Be direct.
```

All fields are at their defaults: provider `anthropic`, model `claude-sonnet-4-6`, all six tools
available, `maxTurns` 50, `permissionMode` `ask`.

---

### Full agent (all fields set)

```markdown
---
name: fix-bug
description: Investigate a failing test and propose a minimal fix.
provider: anthropic
model: claude-sonnet-4-6
tools: [bash, read_file, write_file, edit_file, grep, glob]
maxTurns: 30
permissionMode: ask
---

You are a careful senior engineer. When given a failing test or bug
report, you:
1. Reproduce the failure with `cargo test -- --nocapture` (or the
   appropriate command).
2. Read the relevant source until you understand the failure.
3. Propose the *minimal* edit that fixes it.
4. Verify the test passes.
5. Stop. Do not refactor surrounding code or fix unrelated lints.
```

This is the `fix-bug` agent shipped at `<repo>/.rupu/agents/fix-bug.md` and usable as a
template for custom agents.

---

## Schema validation

The frontmatter deserializer is configured with `#[serde(deny_unknown_fields)]`. Any key that
is not in the list above — including common misspellings — produces a parse error:

```
error: unknown field `permision_mode`, expected one of `name`, `description`,
       `provider`, `model`, `tools`, `maxTurns`, `permissionMode`
```

This fires at `rupu run` / `rupu agent show` load time, not at the start of a tool call deep
inside a run. Fix the frontmatter and re-run.
