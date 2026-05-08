# Workflow File Format Reference

> See also: [workflow-authoring.md](workflow-authoring.md) · [agent-format.md](agent-format.md) · [triggers.md](triggers.md) · [using-rupu.md](using-rupu.md)

---

## Overview

A workflow is a YAML file that orchestrates one or more agent runs.

A workflow can:

- run linear steps in order
- fan out one agent across many items with `for_each:`
- fan out many specialist agents with `parallel:`
- run structured review panels with `panel:`
- pause for human approval with `approval:`
- start manually, on cron, or from an event trigger

Step prompts are rendered with minijinja templates against workflow inputs, prior step outputs, and optional issue / event context.

---

## File location and resolution

```text
<dir>/workflows/<name>.yaml
```

`<dir>` is one of:

- `~/.rupu` for global workflows
- `<project>/.rupu` for project-local workflows

Resolution rules match agents:

- project-local workflows shadow global workflows by `name:`
- shadowing is whole-file; no merging is performed
- `rupu workflow list` shows scope

---

## Top-level fields

| Key | Type | Required | Default | Notes |
| --- | --- | --- | --- | --- |
| `name` | string | yes | — | Workflow identifier |
| `description` | string | no | none | Human-readable summary |
| `trigger` | object | no | `manual` | Manual, cron, or event trigger |
| `inputs` | map | no | `{}` | Typed runtime inputs |
| `defaults` | object | no | `{}` | Workflow-wide defaults |
| `notifyIssue` | bool | no | `false` | Auto-comment only when the run target is an issue |
| `steps` | array<Step> | yes | — | Ordered step list |

An empty `steps:` array is invalid.

---

## Trigger block

```yaml
trigger:
  on: manual | cron | event
  cron: "0 4 * * *"
  event: github.issue.opened
  filter: "{{ event.repo.full_name == 'Section9Labs/rupu' }}"
```

Rules:

- `on` defaults to `manual`
- `cron:` is required only for `on: cron`
- `event:` is required only for `on: event`
- `filter:` is allowed only for `on: event`
- extraneous cross-fields are rejected at parse time

Notes:

- `cron:` must be a 5-field expression
- `event:` accepts the event vocabulary documented in [triggers.md](triggers.md)
- event matching also supports glob-style patterns such as `github.issue.*` or `*`

---

## Inputs

```yaml
inputs:
  phase:
    type: string
    required: true
  retries:
    type: int
    default: 3
  strict:
    type: bool
    default: true
```

Input fields:

| Key | Type | Required | Default | Notes |
| --- | --- | --- | --- | --- |
| `type` | `string` \| `int` \| `bool` | no | `string` | Declared input type |
| `required` | bool | no | `false` | Must be supplied unless `default` exists |
| `default` | scalar | no | none | Must match the declared type |
| `enum` | array<string> | no | `[]` | Allowed stringified values |

At runtime:

- manual workflows accept inputs via `rupu workflow run <name> --input key=value`
- if a workflow also takes an issue target and additional inputs, use `rupu workflow run <name> <issue-ref> --input ...`
- `rupu issues run` is only a convenience wrapper; it does not expose extra `--input` flags

---

## Workflow defaults

Currently supported:

```yaml
defaults:
  continue_on_error: true
```

If a step does not set `continue_on_error`, it inherits the workflow default.

---

## Step fields

Every step has an `id` and exactly one execution shape:

- linear step
- `for_each:` fan-out step
- `parallel:` multi-agent fan-out step
- `panel:` review step

Common fields:

| Key | Type | Applies to | Notes |
| --- | --- | --- | --- |
| `id` | string | all steps | Unique within the workflow |
| `actions` | array<string> | all steps | Action-protocol allowlist, not a tool allowlist |
| `when` | string | all steps | Minijinja expression reduced to truthy / falsy |
| `continue_on_error` | bool | all steps | Tolerates failure and continues |
| `max_parallel` | integer | `for_each`, `parallel`, `panel` | Concurrency cap, must be at least 1 |
| `approval` | object | all steps | Human pause before the step dispatches |

### `actions`

`actions:` is frequently misunderstood.

It does **not** control tool access. Tool access belongs in each agent's `tools:` list.

`actions:` only allowlists action-protocol events emitted from agent output and recorded in the transcript. If you are not intentionally using the action protocol, set:

```yaml
actions: []
```

That is the recommended default today.

### `when`

`when:` is rendered as a template and then reduced to a boolean.

Falsy values are:

- empty string
- `false`
- `0`
- `no`
- `off`

Everything else is truthy.

Examples:

```yaml
when: "{{ steps.review.success }}"
when: "{{ 'bug' in issue.labels }}"
when: "{{ steps.panel.max_severity == 'critical' }}"
```

### `approval`

```yaml
approval:
  required: true
  prompt: |
    About to deploy {{ inputs.tag }}.
    Approve?
  timeout_seconds: 3600
```

Behavior:

- approval is checked after `when:`
- if approval is required, the run pauses before the step dispatches
- resume with `rupu workflow approve <run-id>`
- reject with `rupu workflow reject <run-id> --reason "..."`
- timeouts are enforced lazily on the next run-store interaction

---

## Linear steps

A linear step is the basic shape:

```yaml
- id: summarize
  agent: writer
  actions: []
  prompt: |
    Summarize the previous step.
```

Required fields:

- `id`
- `agent`
- `prompt`

---

## `for_each:` fan-out steps

Use `for_each:` when one agent should process many independent items.

```yaml
- id: review_each
  agent: code-reviewer
  actions: []
  for_each: "{{ inputs.files }}"
  max_parallel: 4
  prompt: |
    Review file {{ item }} ({{ loop.index }} / {{ loop.length }}).
```

Behavior:

- `for_each:` renders to a list of items
- if the rendered text starts with `[`, `rupu` parses it as a JSON / YAML array
- otherwise, `rupu` treats each non-empty line as one item

Per-item template variables:

- `{{ item }}`
- `{{ loop.index }}`
- `{{ loop.index0 }}`
- `{{ loop.length }}`
- `{{ loop.first }}`
- `{{ loop.last }}`

Published outputs:

- `steps.<id>.output` → JSON array string of per-item outputs
- `steps.<id>.results` → list of per-item output strings
- `steps.<id>.success` → `true` only if every item succeeded

---

## `parallel:` multi-agent fan-out steps

Use `parallel:` when different specialists should review or process the same subject.

```yaml
- id: review
  actions: []
  parallel:
    - id: security
      agent: security-reviewer
      prompt: "Review for security issues: {{ inputs.diff }}"
    - id: perf
      agent: performance-reviewer
      prompt: "Review for performance issues: {{ inputs.diff }}"
  max_parallel: 2
```

Rules:

- a `parallel:` step must not also set top-level `agent:` or `prompt:`
- each sub-step must have its own `id`, `agent`, and `prompt`
- sub-step ids must be unique within that parent step

Published outputs:

- `steps.<id>.results` → list of sub-step outputs in declaration order
- `steps.<id>.sub_results.<sub_id>.output` → named output
- `steps.<id>.sub_results.<sub_id>.success` → named success flag
- `steps.<id>.success` → `true` only if every sub-step succeeded

---

## `panel:` review steps

Use `panel:` when several reviewer agents should produce structured findings.

```yaml
- id: panel_review
  actions: []
  panel:
    panelists:
      - security-reviewer
      - performance-reviewer
      - maintainability-reviewer
    subject: "{{ inputs.diff }}"
    max_parallel: 3
```

Panel fields:

| Key | Type | Required | Notes |
| --- | --- | --- | --- |
| `panelists` | array<string> | yes | At least one agent |
| `subject` | string | yes | Rendered once before the first panel pass |
| `prompt` | string | no | Optional per-panelist prompt template |
| `max_parallel` | integer | no | Defaults to 1 |
| `gate` | object | no | Optional review/fix loop |

Important runtime contract:

- each panelist's final assistant message must contain a parseable JSON object with a `findings` array
- surrounding prose is tolerated, but `rupu` extracts the first parseable object with `findings`

Expected findings shape:

```json
{
  "findings": [
    {
      "severity": "low|medium|high|critical",
      "title": "Short title",
      "body": "One sentence detail"
    }
  ]
}
```

Published outputs:

- `steps.<id>.findings` → aggregated findings list with `source`, `severity`, `title`, `body`
- `steps.<id>.max_severity` → highest severity or empty string
- `steps.<id>.iterations` → number of panel passes executed
- `steps.<id>.resolved` → whether the gate cleared
- `steps.<id>.output` → JSON array string of findings

### `panel.prompt`

If `panel.prompt` is set, it is rendered for each panelist. The current subject is injected as `{{ inputs.subject }}` inside that prompt template.

If `panel.prompt` is omitted, the rendered subject text itself is sent to each panelist as the user message.

### `panel.gate`

```yaml
gate:
  until_no_findings_at_severity_or_above: high
  fix_with: finding-fixer
  max_iterations: 4
```

Gate fields:

| Key | Type | Required | Notes |
| --- | --- | --- | --- |
| `until_no_findings_at_severity_or_above` | severity | yes | `low`, `medium`, `high`, `critical` |
| `fix_with` | string | yes | Agent used to address findings between passes |
| `max_iterations` | integer | yes | Must be at least 1 |

Gate behavior:

1. run the panel against the current subject
2. if the highest finding severity is below the threshold, continue
3. otherwise run the fixer agent
4. the fixer receives the original subject plus the findings JSON
5. the fixer's final assistant text becomes the revised subject for the next panel pass
6. stop when the gate clears or `max_iterations` is reached

That means fixer agents should preserve the important context in the revised subject they emit.

---

## Template context

Workflow templates use minijinja. Missing variables render as empty strings.

### Always available

| Variable | Meaning |
| --- | --- |
| `inputs.<key>` | Runtime input values |
| `steps.<step_id>.output` | Final output string from an earlier step |
| `steps.<step_id>.success` | Whether that step completed successfully |
| `steps.<step_id>.skipped` | Whether that step was skipped by `when:` |

### Fan-out outputs

| Variable | Meaning |
| --- | --- |
| `steps.<step_id>.results` | Per-item or per-sub-step outputs |
| `steps.<step_id>.sub_results.<sub_id>.output` | Named output from `parallel:` |
| `steps.<step_id>.sub_results.<sub_id>.success` | Named success from `parallel:` |

### Panel outputs

| Variable | Meaning |
| --- | --- |
| `steps.<step_id>.findings` | Aggregated findings list |
| `steps.<step_id>.max_severity` | Highest severity as a string |
| `steps.<step_id>.iterations` | Panel pass count |
| `steps.<step_id>.resolved` | Whether the gate cleared |

### Issue-target workflows

If the workflow is invoked with an issue target, these are available:

- `issue.number`
- `issue.title`
- `issue.body`
- `issue.labels`
- `issue.author`
- `issue.state`
- `issue.r.project`

Example invocation:

```sh
rupu workflow run issue-to-spec-and-plan github:owner/repo/issues/42
```

### Event-triggered workflows

If the workflow is triggered from an event source, the event payload is available under `event.*`.

Example:

```yaml
when: "{{ event.pull_request.merged }}"
```

See [triggers.md](triggers.md) for the event vocabulary and common payload shapes.

---

## Worked examples

### Minimal linear workflow

```yaml
name: summarize-change
steps:
  - id: summarize
    agent: writer
    actions: []
    prompt: |
      Summarize the change in one paragraph.
```

### `for_each:` file review

```yaml
name: review-changed-files
inputs:
  files:
    type: string
    required: true
steps:
  - id: review_each
    agent: code-reviewer
    actions: []
    for_each: "{{ inputs.files }}"
    max_parallel: 4
    prompt: |
      Review file {{ item }}.
```

### Panel with fix loop

```yaml
name: review-with-fixer
inputs:
  diff:
    type: string
    required: true
steps:
  - id: review
    actions: []
    panel:
      panelists: [security-reviewer, performance-reviewer, maintainability-reviewer]
      subject: "{{ inputs.diff }}"
      max_parallel: 3
      gate:
        until_no_findings_at_severity_or_above: high
        fix_with: finding-fixer
        max_iterations: 3
```

---

## Validation and common failures

Common parse-time failures:

- duplicate step ids
- `parallel:` combined with top-level `agent:` / `prompt:`
- missing `agent:` or `prompt:` on linear steps
- empty `panelists:` list
- invalid `max_parallel` or `max_iterations`
- invalid input defaults or enum defaults
- extraneous fields inside `trigger:`

Common design mistakes:

- using `actions:` as if it were a tool allowlist
- making reviewers write-capable
- building one giant workflow instead of using smaller workflows per phase
- relying on fragile free-form prose when a downstream step needs structured output

---

## Practical guidance

- Use [workflow-authoring.md](workflow-authoring.md) when designing new workflows.
- Use [examples/README.md](../examples/README.md) for complete copyable workflows.
- Use [development-flows.md](development-flows.md) for recommended real-world compositions.
