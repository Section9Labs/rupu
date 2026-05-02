# Workflow File Format Reference

> Part of the rupu reference docs: [spec.md](spec.md) · [agent-format.md](agent-format.md) ·
> **workflow-format.md** · [transcript-schema.md](transcript-schema.md)

---

## Overview

A workflow file is a YAML document that defines a named sequence of agent steps. The orchestrator
renders each step's `prompt` as a minijinja template (with access to CLI inputs and prior step
outputs), runs the specified agent, and chains the results forward. v0 runs steps linearly, one
at a time, in declaration order.

---

## File location

```
<dir>/workflows/<name>.yaml
```

Where `<dir>` is one of:

- `~/.rupu/` — global workflow library; available in every workspace.
- `<project>/.rupu/` — project-local workflows; override globals with the same `name`.

Resolution rules are identical to agents: project file wins by `name`, no merging.

---

## Top-level fields

### `name`

| Attribute | Value  |
|-----------|--------|
| Type      | string |
| Required  | yes    |

The workflow's unique name within its scope. Used in `rupu workflow run <name>` and
`rupu workflow show <name>`. Convention: lowercase, hyphen-separated.

---

### `description`

| Attribute | Value  |
|-----------|--------|
| Type      | string |
| Required  | no     |

One-line description. Displayed by `rupu workflow list` and `rupu workflow show`.

---

### `steps`

| Attribute | Value                         |
|-----------|-------------------------------|
| Type      | array\<Step\>                 |
| Required  | yes                           |
| Minimum   | 1 step                        |

An ordered list of steps executed sequentially. An empty `steps: []` array is a parse error.

---

## Step fields

### `id`

| Attribute | Value  |
|-----------|--------|
| Type      | string |
| Required  | yes    |

Unique identifier for this step within the workflow. Referenced by later steps via
`steps.<id>.output` in prompt templates. Convention: lowercase, hyphen-separated.

Must be unique within the workflow; duplicate `id` values produce a parse error.

---

### `agent`

| Attribute | Value  |
|-----------|--------|
| Type      | string |
| Required  | yes    |

Name of the agent to run for this step. Resolved using the same project/global lookup as
`rupu run <name>`. The agent file must exist at workflow-run time; a missing agent produces a
user error before the first step executes.

Multiple steps in the same workflow may reference the same agent.

---

### `actions`

| Attribute | Value           |
|-----------|-----------------|
| Type      | array\<string\> |
| Required  | yes             |

Allowlist of action-protocol verbs that the agent may emit in this step. The orchestrator checks
each `action_emitted` event against this list and records `allowed: true|false` in the
transcript.

In v0, no action actually executes effects — the allowlist is validated and logged, but nothing
happens. Slice B wires real effects (open PR, post comment, create branch) behind the same
contract.

Use an empty array `actions: []` to permit no actions (the agent can still use tools freely;
actions are a separate protocol layer on top of tool calls).

---

### `prompt`

| Attribute | Value  |
|-----------|--------|
| Type      | string |
| Required  | yes    |

The prompt passed to the agent as its user turn. Supports minijinja template syntax.

#### Template variables

| Variable                   | Description                                              |
|----------------------------|----------------------------------------------------------|
| `inputs.<key>`             | Value passed via `rupu workflow run --input KEY=VALUE`   |
| `steps.<step_id>.output`   | Final assistant text from the named earlier step         |

Template rendering runs at step-start time, not at workflow-parse time. Forward references
(referencing a step that has not yet run) produce a runtime error.

---

## v0 limitations and future-reserved keys

The following keys are parsed and **rejected** in v0 with a clear error message indicating they
are deferred to Slice B. This prevents silent behavior changes when Slice B ships:

| Reserved key | Planned use                                   |
|--------------|-----------------------------------------------|
| `parallel`   | Fan-out: run multiple steps concurrently      |
| `when`       | Conditional step execution                    |
| `gates`      | Human-approval gates between steps            |

Example error:
```
error: field `parallel` is reserved for Slice B and not supported in v0
```

---

## Action protocol

Each step has an `actions:` allowlist. The orchestrator enforces this allowlist against every
`action_emitted` event produced during the step.

In the transcript, each `action_emitted` event records:

- `kind` — the action verb (e.g., `propose_edit`, `log_finding`, `open_pr`)
- `payload` — the action data
- `allowed` — whether the action was on the step's allowlist
- `applied` — whether the effect was executed (always `false` in v0)
- `reason` — optional explanation (e.g., `"not on allowlist"`, `"not wired in v0"`)

This schema is stable from v0 so Slice B can wire effects without changing the event format.

---

## Worked example

The `investigate-then-fix` workflow, shipped at `<repo>/.rupu/workflows/investigate-then-fix.yaml`:

```yaml
name: investigate-then-fix
description: Two-step bug fix — investigate, then propose minimal edit.
steps:
  - id: investigate
    agent: fix-bug
    actions: []
    prompt: |
      Investigate the bug described by:
      {{ inputs.prompt }}

      Stop without making edits. Report the root cause as text.

  - id: propose
    agent: fix-bug
    actions: []
    prompt: |
      Based on this investigation:
      {{ steps.investigate.output }}
      Propose and apply the minimal fix.
```

**Running it:**

```sh
rupu workflow run investigate-then-fix --input prompt="cargo test fails with index out of bounds in parser.rs:142"
```

1. The orchestrator renders the `investigate` step's prompt with `inputs.prompt`.
2. The `fix-bug` agent runs, producing a final assistant text (the investigation report).
3. The orchestrator renders the `propose` step's prompt, substituting
   `{{ steps.investigate.output }}` with the investigation text.
4. The `fix-bug` agent runs again, this time proposing and applying the fix.
5. `run_complete` is emitted with cumulative token counts.

---

## Engine behavior summary

1. Parse YAML; reject unknown keys and reserved keys (`parallel`, `when`, `gates`).
2. Verify all `agent:` values exist before executing any step.
3. For each step in declaration order:
   a. Render `prompt:` template.
   b. Build a provider client for the step's agent spec.
   c. Run the agent loop; stream events to the transcript.
   d. Check each `action_emitted` against `actions:` allowlist; record result.
   e. On agent failure or abort, abort the workflow immediately (no subsequent steps run).
4. Emit `run_complete` with `status: ok` (or `error` / `aborted`).
