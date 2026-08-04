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
- call an MCP connector tool directly, no agent, with `action:`
- route to different downstream steps with `branch:`
- pause for human approval with `approval:`
- run out of declaration order as an explicit graph with `next:`/`depends_on:`/`split:`/`join:`/`loops:`
- place a step (or a `for_each:` fan-out) on a remote host with `host:`/`distribute:`
- start manually, on cron, or from an event trigger
- carry contract-validated outputs for downstream automation
- opt into persistent autonomous reconciliation with `autoflow:`

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
| `contracts` | object | no | `{}` | Named structured outputs validated against JSON Schema |
| `autoflow` | object | no | none | Autonomous ownership metadata for `rupu autoflow ...` |
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

## `autoflow:` block

Use `autoflow:` when the same workflow file should also be runnable through:

```sh
rupu autoflow run <name> <issue-ref>
rupu autoflow tick
rupu autoflow serve --repo github:your-org/your-repo
```

Example:

```yaml
autoflow:
  enabled: true
  entity: issue
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["autoflow"]
    labels_any: ["bug", "urgent"]
    labels_none: ["blocked"]
    limit: 100
  wake_on:
    - github.issue.opened
    - issue.queue_entered
    - github.pr.opened
  reconcile_every: "10m"
  claim:
    key: issue
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
```

Fields:

| Key | Type | Required | Default | Notes |
| --- | --- | --- | --- | --- |
| `enabled` | bool | no | `false` | Workflow appears under `rupu autoflow list` only when true |
| `entity` | `issue`\|`pull_request` | no | `issue` | Which kind of tracker/SCM entity this autoflow owns |
| `source` | string | no | none | Free-form tracker-native source tag, e.g. `linear:<team-id>` or `jira:<host>/<project>`. Disambiguates which bound tracker/repo an autoflow belongs to when more than one is configured; purely informational, not a filter |
| `priority` | integer | no | `0` | Higher wins when multiple autoflows match the same issue |
| `selector.states` | array<`open`\|`closed`> | no | `[]` | Empty means any issue state |
| `selector.labels_all` | array<string> | no | `[]` | Every listed label must be present |
| `selector.labels_any` | array<string> | no | `[]` | At least one listed label must be present |
| `selector.labels_none` | array<string> | no | `[]` | None of the listed labels may be present |
| `selector.limit` | integer | no | none | Candidate cap per reconciliation cycle |
| `selector.draft` | `include`\|`exclude`\|`only` | no | none | `entity: pull_request` only; rejected on `entity: issue`. Unset behaves like `include` (both draft and ready-for-review PRs match) |
| `selector.base` | string | no | none | `entity: pull_request` only; rejected on `entity: issue`. Restricts matches to PRs targeting this base branch (e.g. `main`) |
| `selector.authors` | array<string> | no | `[]` | Explicit allowlist of author logins. **Empty is no restriction** — see the safety note below |
| `selector.authors_from` | `collaborators`\|`org_members` | no | none | Broader author-scope check, resolved against the SCM at tick time. **Unset is no restriction** — see the safety note below |
| `selector.on_skip` | `skip`\|`label_needs_human` | no | `skip` | What to do when an event is otherwise eligible but excluded by the author allowlist |
| `wake_on` | array<string> | no | `[]` | Canonical or semantic event ids used as wake hints |
| `reconcile_every` | duration | no | none | Re-run cadence like `10m`, `2h`, `1d` |
| `claim.key` | `issue`\|`pr_head_sha` | no | `issue` | Claim granularity |
| `claim.ttl` | duration | no | none | Lease duration for persistent issue ownership |
| `workspace.strategy` | `worktree`\|`in_place` | no | `worktree` | How repo files are materialized |
| `workspace.branch` | string | no | generated | Strict-rendered branch template |
| `outcome.output` | string | yes for autoflows | none | Name of the declared workflow output to consume |

Notes:

- `autoflow:` does not replace `trigger:`. `trigger:` still describes one-shot starts; `autoflow:` describes persistent ownership over time.
- Workflow files remain usable with `rupu workflow run`; autoflow semantics activate only under `rupu autoflow run`, `rupu autoflow tick`, or `rupu autoflow serve`.
- Autoflow execution accepts only `bypass` or `readonly` permission modes. `ask` and any other value are rejected.
- `autoflow` template rendering is strict. Missing variables are a protocol error in autonomous mode.
- `wake_on` accepts the same canonical ids and semantic aliases documented in `docs/triggers.md`, for example `github.issue.labeled`, `issue.queue_entered`, or `pr.review_activity`.
- `wake_on` becomes actionable only when the autoflow runtime can see matching SCM events, typically via `[triggers].poll_sources` for the repo or via webhook wake hints recorded by `rupu webhook serve`.
- Most laptop users should start with polling plus `rupu autoflow tick` or `rupu autoflow serve`; public webhook exposure is optional and better suited to always-on or tunneled machines.
- Matching precedence is deterministic: higher `priority` wins, then workflow name.
- On later ticks, an idle lower-priority claim can be released and replaced by a higher-priority winner. Active or approval-paused claims are not stolen automatically.
- Operator-facing commands surface that decision: `rupu autoflow status` lists contested issues, and `rupu autoflow claims` shows the selected priority plus losing contenders.
- The same workflow file is portable across local scheduled mode, always-on local worker mode, and future cloud relay mode. The runtime contract is the persisted `RunEnvelope`, `WakeRecord`, `ArtifactManifest`, and `WorkerRecord` documented in `docs/using-rupu.md`.

Author restriction (`selector.authors` / `selector.authors_from`):

- **With neither `authors` nor `authors_from` set, any author who opens a matching issue or PR can trigger the autoflow.** Autoflows commonly run at `permission_mode: bypass` (see above), so an unrestricted selector means any outside contributor's issue/PR can start a bypass-mode agent run against your repo. This is the current default and it is intentional to preserve — tightening it would silently stop existing autoflows from firing on issues/PRs opened by non-collaborators. Set one of the two fields explicitly for any unattended autoflow.
- Precedence between the two fields is not a simple AND/OR of independent checks — it's evaluated in this order:
  1. If `authors` is non-empty and contains the event's author login, the author is allowed, full stop — `authors_from` is not consulted.
  2. Otherwise, if `authors_from` is set, the author is allowed iff the SCM connector reports them as satisfying that scope (a repo collaborator, or an org member) — regardless of whether `authors` also failed to match.
  3. Otherwise, if both `authors` is empty and `authors_from` is unset, there is no restriction at all: allowed.
  4. Otherwise (`authors` is non-empty with no match, and `authors_from` is unset), the author is denied.
- When an author is denied, `selector.on_skip` decides what happens: `skip` (the default) silently drops the event; `label_needs_human` also applies a label so a human notices the otherwise-eligible issue/PR was excluded.
- Recommended for any unattended autoflow: set `selector.authors_from: collaborators` (or `org_members` for a broader org-wide allowlist) rather than relying on the default.

Deployment guidance:

- use `rupu autoflow tick` when you want an OS scheduler to own cadence
- use `rupu autoflow serve` when one local worker should stay online and consume due wakes continuously
- use `rupu webhook serve` only when the machine is intentionally reachable or tunneled; laptop users should usually start with polling instead

Typical pattern:

- a high-priority controller autoflow like `issue-supervisor-dispatch`
- one or more lower-priority child autoflows such as a phase-delivery workflow
- explicit `dispatch` objects in the output contract when one workflow should hand off to another

---

## `contracts:` block

Use `contracts:` to name machine-readable workflow outputs and validate them against schemas stored under:

```text
<project>/.rupu/contracts/
~/.rupu/contracts/
```

Project-local contracts shadow global contracts by name.

Example:

```yaml
contracts:
  outputs:
    result:
      from_step: handoff
      format: json
      schema: autoflow_outcome_v1
```

Fields:

| Key | Type | Required | Notes |
| --- | --- | --- | --- |
| `outputs.<name>.from_step` | string | yes | Step id whose final output is the canonical value |
| `outputs.<name>.format` | `json`\|`yaml` | yes | Serialization expected from the step output |
| `outputs.<name>.schema` | string | yes | Contract name resolved to `.json` schema file |

Why this matters:

- autoflows need structured outcomes instead of free-form prose
- controller workflows need durable child-dispatch payloads
- later workflows can depend on stable artifacts like phase plans or review packets

Common shipped schemas:

- `autoflow_outcome_v1`
- `workflow_dispatch_v1`
- `phase_plan_v1`
- `review_packet_v1`

---

## Step fields

Every step has an `id` and exactly one execution shape:

- linear step
- `for_each:` fan-out step
- `parallel:` multi-agent fan-out step
- `panel:` review step
- `action:` connector step (no agent — calls an MCP tool directly)
- `branch:` routing step
- an orchestration node (`split:` or `join:` — pure routing, no work of its own)
- a gate node (a standalone `approval:` block with none of the above)

Common fields:

| Key | Type | Applies to | Notes |
| --- | --- | --- | --- |
| `id` | string | all steps | Unique within the workflow |
| `actions` | array<string> | agent steps (`step`/`for_each`) | Narrows this step's connector (MCP) tool calls — see below |
| `when` | string | all steps | Minijinja expression reduced to truthy / falsy |
| `continue_on_error` | bool | all steps | Tolerates failure and continues |
| `max_parallel` | integer | `for_each`, `parallel`, `panel` | Concurrency cap, must be at least 1 |
| `approval` | object | all steps | Human pause before the step dispatches |
| `contract` | object | linear steps | Optional documentation for a structured step output |

### `actions`

`actions:` **does narrow tool access** — but only the connector (MCP catalog)
portion of it. Builtin tools (`bash`, `read_file`, `write_file`, `edit_file`,
`grep`, `glob`, `ast_grep`, `dispatch_agent`, `dispatch_agents_parallel`) are
never touched by this field; they're governed entirely by the agent's own
`tools:` and the run mode (`ask`/`bypass`/`readonly`).

Each entry must name a tool in the MCP catalog (`scm.*`, `issues.*`,
`github.*`, `gitlab.*` — see `GET /api/tools`); an unknown entry is a
parse-time error.

Semantics:

- **Empty (or absent) `actions:` means unrestricted** — the step runs with
  the agent's full tool grant, unchanged. This is the compatibility default;
  every workflow written before this field was enforced already relies on
  it.
- **A non-empty `actions:` narrows only the connector subset** of the
  agent's grant to the intersection of the agent's tools and this list.
  Builtins pass through untouched. A step can only ever shrink the
  connector set — naming a catalog tool the agent doesn't grant does
  **not** add it (no escalation).
- **Not supported on a remote step** (`host:` / `distribute:`): a non-empty
  `actions:` there is rejected at parse time, because the roster never
  reaches the remote dispatch path today (it would otherwise be a silent
  no-op).

```yaml
# agent grants: [issues.list, issues.get, issues.comment, issues.create, issues.update_state]
steps:
  - id: triage
    agent: issue-reporter
    actions: [issues.list, issues.get]   # narrowed to read-only for THIS step
  - id: report
    agent: issue-reporter
    actions: []                          # unrestricted — full agent grant
```

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

### Permission modes in a workflow

`rupu workflow run` accepts `--mode ask | bypass | readonly`. **`ask` and
`bypass` behave identically for workflow agent steps**, and `ask` is the
default when `--mode` is omitted — so a workflow run without an explicit mode
grants its steps full tool access.

That is deliberate. The agent runtime's interactive `ask` decider blocks on
stdin waiting for a human, and a workflow step has no operator present at the
tool layer, so a genuinely-prompting `ask` would hang every unattended run.
Rather than silently doing something different from what the name suggests,
`rupu workflow run` prints a warning when no mode was given:

```
warning: --mode not set; workflow steps run at `bypass`
         (there is no operator to answer `ask` mid-run).
         Pass --mode readonly to deny bash/write_file/edit_file.
```

| Mode | Workflow agent steps | Action steps |
| --- | --- | --- |
| `bypass` | all tools allowed | all catalog tools allowed |
| `ask` | **same as `bypass`** — no prompt is possible | same as `bypass` |
| `readonly` | `bash`, `write_file`, `edit_file` denied | Write-classified tools refused |

**If you want a workflow restricted, pass `--mode readonly` explicitly** —
`ask` will not do it. For human-in-the-loop control over *what a workflow
does*, use a gate node (below) rather than the permission mode: gates pause the
run at a point you choose, where an operator can actually answer.

### Gate nodes (standalone `approval:` steps)

A step that carries **only** an `approval:` block — no `agent:`, `action:`,
`for_each:`, `parallel:`, `panel:` or `branch:` — is a **gate node**: a
first-class pause in the graph rather than a pause attached to some other
step's work.

```yaml
- id: sign-off
  approval:
    required: true
    prompt: "Ship {{ inputs.tag }} to production?"
    timeout_seconds: 3600
    on_timeout: reject        # approve | reject | fail (default: fail)
    auto_approve: "{{ inputs.trusted }}"
    on_reject:                # steps run when the gate is rejected
      - id: rollback
        action: scm.prs.comment
        with: { platform: github, owner: acme, repo: widget, number: 41, body: "rolled back" }
    notify:                   # fired best-effort as the gate parks
      - action: issues.comment
        with: { project: "acme/widget", number: 41, body: "awaiting sign-off" }
```

| Field | Values | Notes |
| --- | --- | --- |
| `on_timeout` | `approve` \| `reject` \| `fail` | What to do when `timeout_seconds` elapses. Defaults to `fail`. |
| `auto_approve` | expression | When truthy, the gate resolves without pausing. `notify:` hooks do **not** fire on auto-approve. |
| `on_reject` | list of steps | A cleanup chain run when the gate is rejected — by an operator, or by an `on_timeout: reject`. |
| `notify` | list of `action:` hooks | Fired best-effort just before the gate parks. A notify failure never blocks the pause. |

A gate's decision is available downstream as `steps.<gate-id>.decision`
(already parsed — no `fromjson` needed).

#### Unattended timeout routing requires `rupu cp serve`

**`timeout_seconds` and `on_timeout` are only acted on by a running
`rupu cp serve`**, whose background gate sweep
(`[cp].gate_sweep_enabled`, default on, `[cp].gate_sweep_interval_secs`,
default 60) is what expires overdue gates and executes their routing.

If you never start `cp serve`, or you set `[cp].gate_sweep_enabled = false`:

- an `on_timeout: approve` gate **stays parked** rather than resuming — resolve
  it with `rupu workflow approve <run-id>`;
- an `on_timeout: reject` gate likewise stays parked — `rupu workflow reject
  <run-id> --reason "..."` resolves it and runs the `on_reject` chain;
- an `on_reject` chain triggered from the **CP web UI, the desktop app, or
  `rupu workflow cancel`** is recorded as pending and executed by the sweep, so
  it too waits until `cp serve` runs.

Listing runs (`rupu workflow runs`) deliberately does **not** resolve gates or
execute cleanup chains — a read command must have no external side effects. The
CLI `approve`/`reject` commands and the sweep are the only things that do.

### `contract`

Use `contract:` on a step when humans and prompts should see the expected output shape directly on the step:

```yaml
- id: handoff
  agent: writer
  actions: []
  contract:
    emits: autoflow_outcome_v1
    format: json
  prompt: |
    Return only valid JSON for `autoflow_outcome_v1`.
```

Fields:

| Key | Type | Required | Notes |
| --- | --- | --- | --- |
| `emits` | string | yes | Contract name the step is expected to emit |
| `format` | `json`\|`yaml` | yes | Serialization the step should return |

Important:

- workflow-level `contracts.outputs.*` remains the runtime authority
- step-level `contract:` is authoring metadata
- if the step metadata disagrees with the workflow output declaration, the workflow is invalid

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

## `action:` connector steps

Use `action:` when a step should call an MCP catalog tool directly — no agent, no LLM turn at all.

```yaml
steps:
  - id: fetch
    action: issues.get
    with: { project: "acme/widget", number: 41 }

  - id: triage
    agent: triager
    prompt: |
      Issue title: {{ (steps.fetch.output | fromjson).title }}
      Labels: {{ (steps.fetch.output | fromjson)['labels'] | join(', ') }}
```

Fields:

| Key | Type | Required | Notes |
| --- | --- | --- | --- |
| `action` | string | yes | Tool name from the MCP catalog (`scm.*`, `issues.*`, `github.*`, `gitlab.*` — see `GET /api/tools`) |
| `with` | map | no | Parameters passed to the tool call; string values may be minijinja templates |

Rules, all enforced at parse time:

- `action:` must name a real tool in the catalog. An unknown name is a parse error.
- `with:`'s keys are validated against the tool's JSON Schema: an unknown key or a missing required key both fail parsing before the run ever starts.
- `action:` is mutually exclusive with `agent:`/`prompt:`/`for_each:`/`parallel:`/`panel:`/`branch:`.
- an `action:` step must not also carry a **non-empty** `actions:` allowlist. `actions:` narrows an *agent's* connector grant; an action step's tool call is already fully explicit, so there's nothing left to narrow — a non-empty `actions:` here is rejected as a parse error. An empty (or absent) `actions:` is legal, if redundant.

Typed `with:` values are coerced against the tool's declared schema type:

- a plain literal against a schema-declared `integer`/`number`/`boolean` field (e.g. `number: "7"` with stray quotes) is rejected at **parse** time — this is almost always an authoring typo;
- a **whole-leaf** template (the entire string is one `{{ ... }}` expression, nothing else) is rendered, then parsed against the declared type; a render that doesn't fit the type is a runtime error;
- a **partial** template — a `{{ ... }}` expression mixed with other text, e.g. `"issue-{{ n }}"` against a numeric field — is a runtime error rather than something the pipeline guesses at: there is no single number to extract from `"issue-42"`, so use a whole-leaf template (`"{{ n }}"`) for a typed parameter instead.

Published output:

- `steps.<id>.output` — the tool's return value, serialized as a JSON string
- `steps.<id>.success` — whether the dispatcher call succeeded

Because `output` is a JSON *string*, pull fields out of it with the `fromjson` filter documented under "Template filters" further down — `{{ (steps.fetch.output | fromjson).title }}`, not `{{ steps.fetch.output.title }}`.

---

## `run:` deterministic command steps

Use `run:` when a step must produce an **exact** value — scoring, validation,
report rendering, a preflight check. No language model is involved: the runner
executes a declared command and binds its output.

```yaml
- id: score
  run:
    cmd: python3
    args: ["tools/score_fixture.py", "{{ item.fixture }}", "{{ item.candidate }}"]
    cwd: "{{ inputs.cybermark_root }}"
    env: { CYBERBENCH_RELEASE_KEY: "{{ inputs.release_key }}" }
    parse: json            # json | lines | raw   (default: raw)
    timeout_seconds: 300
    allow_exit_codes: [0]  # default [0]; anything else fails the step
```

### There is no shell

`cmd` and `args` are handed to the OS as an **argv vector**. There are no pipes,
no redirection, no globbing, and no word splitting. Each `args` entry renders
independently and becomes exactly one argument, so a rendered value containing
`;`, `&&`, or `$(...)` arrives as literal text rather than executing.

If you genuinely need shell features, invoke a shell explicitly
(`cmd: sh`, `args: ["-c", "..."]`) — that is your decision to make, visible in
the workflow file, rather than something the engine does to every step behind
your back.

### Output bindings

| Binding | Contents |
|---|---|
| `steps.<id>.output` | stdout as a **string** (all `parse:` modes) |
| `steps.<id>.json` | stdout **parsed** per `parse:` — indexable |
| `steps.<id>.stdout` / `.stderr` | raw streams |
| `steps.<id>.exit_code` | the process exit code |
| `steps.<id>.duration_ms` | wall-clock duration |
| `steps.<id>.success` | exit code was in `allow_exit_codes` |

Index structured output through `json`, not `output`:

```yaml
- id: report
  run:
    cmd: echo
    args: ["scored {{ steps.score.json.score }} / 100"]
```

`output` stays a string for every step kind in the engine, so a step that starts
emitting JSON never changes what `{{ steps.<id>.output }}` renders elsewhere.

Under `parse: json`, stdout that is not valid JSON **fails the step**. It does
not fall back to the raw string — binding garbage as "the output" would let a
broken tool produce a plausible-looking but meaningless downstream result.

### Fan-out

`run:` composes with `for_each:` and `max_parallel:`, which is how you score N
items concurrently:

```yaml
- id: score
  for_each: "{{ steps.plan.json.jobs }}"
  max_parallel: 8
  continue_on_error: true
  run:
    cmd: python3
    args: ["score_job.py", "--job-id", "{{ item.job_id }}"]
    parse: json
```

Per-unit results land in `steps.<id>.results[*]` in **declared** order regardless
of finish order. With `continue_on_error: true`, a failing unit is recorded with
`success=false` and the remaining units still dispatch — one bad item never costs
you the other 199. Without it, any failing unit fails the step.

`distribute:` is **not** supported on a `run:` step and is rejected at parse time
rather than ignored: `run:` executes on the coordinator, and silently dropping a
`distribute:` would let you believe work was spread across a fleet when it never
left the local host.

### Enabling it

`run:` executes commands, so it is opt-in per workspace:

```toml
[workflow]
run_step_enabled = true
# Optional. Empty (the default) permits any executable.
# Matched on basename, so "/bin/bash" and "bash" gate alike.
run_step_allowlist = ["python3", "make"]
```

A workflow containing a `run:` step **fails** when the toggle is off — it does
not skip the step. A benchmark that quietly omitted its scoring step would report
a plausible-looking but meaningless number.

Permission modes:

| Mode | Behaviour |
|---|---|
| `readonly` | `run:` steps are refused |
| `ask` | allowed — see below |
| `bypass` | allowed |

`ask` allows `run:` steps for the same reason it allows agent writes in a
workflow: there is no operator present mid-run to answer a prompt, so a
genuinely-prompting `ask` would hang every scheduled run. That gap is announced
by the same warning agent steps print. The workspace opt-in above is the real
control, and `bypass` cannot override it.

### Mutual exclusivity

`run:` cannot be combined with `agent`/`prompt`, `parallel:`, `panel:`,
`branch:`, `action:`, `split:`, or `join:`. It *is* compatible with `for_each:`,
`when:`, and `continue_on_error:`.

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

## `branch:` steps

Use `branch:` to route to different downstream steps depending on a condition, instead of running every subsequent step unconditionally.

```yaml
steps:
  - id: tests
    agent: tester
    prompt: "run tests"
  - id: check
    branch:
      condition: "{{ steps.tests.success }}"
      then: [deploy]
      else: [notify_failure]
  - id: deploy
    agent: deployer
    prompt: "Deploy the build."
  - id: notify_failure
    agent: reporter
    prompt: "Tests failed: {{ steps.tests.output }}"
```

Fields:

| Key | Type | Required | Notes |
| --- | --- | --- | --- |
| `condition` | string | yes | minijinja expression, reduced to truthy/falsy the same way as `when:` |
| `then` | array<string> | no | Step ids to dispatch when `condition:` is truthy |
| `else` | array<string> | no | Step ids to dispatch when `condition:` is falsy |

Rules, all enforced at parse time:

- `branch:` is mutually exclusive with `agent:`/`prompt:`/`for_each:`/`parallel:`/`panel:`/`action:`.
- `when:` is **not allowed** on a branch step. The runner evaluates `when:` before the branch block, so a falsy `when:` on a branch step would skip only the branch step itself — its condition never evaluates, so *neither* arm is added to the run's skip-set, and both arms would run. Use the branch `condition:` itself to gate routing instead.
- every id in `then:`/`else:` must name a real step, and that step must run strictly *after* the branch step (a forward reference) — routing backwards isn't a supported loop construct.
- the same step id can't appear in both `then:` and `else:`.
- a branch step can't target itself.

### The transitive-arm rule

This is the part that's easy to get wrong in a way that produces a **silently wrong result** rather than a parse error, so read it carefully.

`then:`/`else:` must each list the **complete, transitive** set of step ids that arm covers — including the arm-target steps of any branch step nested inside it. The runner does not compute reachability at run time: when it takes (say) the `then:` arm, it skips *exactly* the ids literally written in `else:` — nothing more, nothing less.

A branch nested inside a not-taken arm is therefore itself skipped, which means it never runs and never gets a chance to add its *own* sub-arms to the skip-set. If its sub-arm steps weren't already listed in the outer arm's list, they run anyway — unskipped — even though the branch that would have chosen between them never fired.

```yaml
# WRONG — parses fine, but silently over-runs on one path.
steps:
  - id: outer
    branch:
      condition: "{{ inputs.deploy }}"
      then: [ship]
      else: [inner]              # missing inner's own arm targets
  - id: inner
    branch:
      condition: "{{ inputs.staging }}"
      then: [inner_yes]
      else: [inner_no]
  - id: inner_yes
    agent: deployer
    prompt: "deploy to staging"
  - id: inner_no
    agent: deployer
    prompt: "deploy to prod"
  - id: ship
    agent: deployer
    prompt: "ship normally"
```

If `inputs.deploy` is truthy, `outer` takes `then: [ship]`, so the runner's skip-set is exactly `else`'s literal list — `[inner]`. `inner` is skipped, as intended. But `inner_yes` and `inner_no` are **not** in that skip-set, so **both** of them still run, even though the branch that was supposed to choose between them never executed. The fix is to list everything `inner` would have contributed, transitively, in `outer`'s `else:`:

```yaml
# CORRECT — outer's else: arm is the complete transitive set.
  - id: outer
    branch:
      condition: "{{ inputs.deploy }}"
      then: [ship]
      else: [inner, inner_yes, inner_no]
```

---

## Non-linear orchestration

By default, steps run in declaration order (a "chain" workflow). A workflow becomes an explicit **graph** — scheduled by node readiness instead of list order — as soon as any step uses `next:`, `depends_on:`, `split:`, or `join:`, or the workflow declares a `loops:` block.

### `next:` / `depends_on:`

```yaml
steps:
  - id: a
    agent: worker
    prompt: p
    next: [b]
  - id: b
    agent: worker
    prompt: p
    depends_on: [a]
```

- `next:` lists this step's successor id(s); `depends_on:` lists this step's predecessor id(s) — the symmetric inverse of `next:`. Step `a`'s `next: [b]` and step `b`'s `depends_on: [a]` describe the identical edge; author with whichever reads better, or mix both in the same workflow.
- every edge target must name a real step id, and a step can't edge to itself.
- the full edge set (`next`/`split`/branch-arm/`depends_on` control edges, unioned with the data edges inferred from `{{ steps.X.* }}` template references) must be acyclic; a cycle is a parse-time error.

### `split:` / `join:`

```yaml
steps:
  - id: fanout
    split: [a, b, c]
  - id: a
    agent: worker
    prompt: "do a"
    next: [gathered]
  - id: b
    agent: worker
    prompt: "do b"
    next: [gathered]
  - id: c
    agent: worker
    prompt: "do c"
    next: [gathered]
  - id: gathered
    join: { wait: { count: 2 } }
  - id: after
    agent: worker
    prompt: "n={{ steps.gathered.results | length }}"
    depends_on: [gathered]
```

- `split:` fans out into N independent concurrent tracks, named by step id. A `split:` node carries no `agent:`/`action:`/etc. of its own — it's pure routing.
- `join:` is a barrier: it waits for its inbound paths per `wait:`, then exposes its gathered results as `steps.<id>.results`. `wait:` accepts `all` (the default), `any`, or `{ count: N }`.
- a `join:` needs at least one inbound edge — a join nothing points at would never fire, silently stranding it and everything downstream, so it's rejected at parse time — and a `{ count: N }` can't exceed the number of inbound paths that actually feed it.
- reconvergence doesn't require an explicit `join:` node: two branches can both `next:`/`depends_on:` the same ordinary step, and it naturally waits for every inbound path before running (implicit "wait: all"). Reach for an explicit `join:` node when you want `any`/`count` semantics, or a dedicated barrier with no work of its own.
- neither `split:` nor `join:` may carry `agent:`/`action:`/`for_each:`/`parallel:`/`panel:`/`branch:`/`approval:` — they are pure orchestration nodes.

### `loops:`

A workflow-level `loops:` map declares named, bounded subgraph loops: a subset of existing step ids that re-run together, in sequence, until a condition holds or an iteration cap is hit.

```yaml
name: has-loop
steps:
  - id: gen
    agent: writer
    prompt: "produce a draft"
  - id: test
    agent: reviewer
    prompt: "review the draft"
    depends_on: [gen]
loops:
  refine:
    nodes: [gen, test]
    until: "{{ steps.test.output }}"
    max_iterations: 3
    on_max: fail
```

Fields:

| Key | Type | Required | Default | Notes |
| --- | --- | --- | --- | --- |
| `nodes` | array<string> | yes | — | Must name real, existing steps; at least 2; a step belongs to at most one loop |
| `until` | string | yes | — | minijinja expression (same engine as `when:`), evaluated after each iteration; truthy converges the loop |
| `max_iterations` | integer | yes | — | Must be at least 1 |
| `on_max` | `fail`\|`proceed` | no | `fail` | What happens when the cap is hit without `until` ever holding: `fail` fails the run; `proceed` continues downstream with the last iteration's outputs and `converged: false` |

Per-loop progress is available as `{{ loops.<name>.iteration }}` / `{{ loops.<name>.converged }}` — to the loop's own members mid-iteration, to `until`'s own evaluation, and to every step downstream of the loop once it finishes.

### `max_concurrency:`

A workflow-level integer cap on how many nodes the scheduler runs concurrently across the **whole graph** — distinct from a `for_each:`/`parallel:`/`panel:` step's own `max_parallel:`, which caps only that one step's internal fan-out.

```yaml
name: max-concurrency-example
max_concurrency: 4
steps:
  - id: fanout
    split: [a, b]
  - id: a
    agent: worker
    prompt: p
  - id: b
    agent: worker
    prompt: p
```

Must be at least 1 when set. Omitted (the default) is unbounded: every node the graph's dependency structure makes ready runs, including a `split:`'s full fan-out, with no artificial cap. Only consulted for a non-linear workflow (one using any of the constructs above) — ignored by a plain chain workflow.

---

## Remote placement

By default every step's agent runs locally, in the same process as the rest of the workflow. `host:` and `distribute:` place a step's work on a different host in your fleet instead.

### `host:` — a single placed step

```yaml
name: host-example
steps:
  - id: edit
    agent: editor
    prompt: "edit foo.txt"
    host: worker-1
    workspace: sync
  - id: verify
    agent: verifier
    prompt: "check: {{ steps.edit.output }}"
```

- valid only on a linear step (`agent:` + `prompt:`) — not on `for_each:`/`parallel:`/`panel:`/`branch:`/`action:`/an approval gate.
- the whole step's agent runs on the named host via the fleet's `UnitDispatcher` port, and its output feeds downstream steps exactly as a local step's would.
- must be a non-empty string.

### `distribute:` — spreading a `for_each:` fan-out across hosts

```yaml
name: distribute-example
steps:
  - id: edit
    agent: editor
    for_each: "x\ny\nz"
    prompt: "edit {{ item }}.txt"
    max_parallel: 3
    workspace: sync
    distribute:
      hosts: [w1, w2]
```

- only valid on a `for_each:` step; `hosts:` must be non-empty.
- items are spread round-robin across the listed hosts instead of all running locally.

### `workspace:`

`workspace: sync` makes the coordinator's workspace available on the remote host and brings file changes back afterward. The default — `workspace:` omitted, or `workspace: none` — keeps the step self-contained: the remote step sees only its rendered prompt plus prior steps' string outputs, no files.

`workspace:` is only meaningful on a remote step (one with `host:` or `distribute:`); setting `sync` on a purely local step is rejected at parse time as author confusion. A workflow-level `defaults.workspace:` sets the fallback used by every remote step that doesn't set its own.

### `actions:` is not supported on a remote step

A non-empty `actions:` allowlist on a `host:`/`distribute:` step is rejected at parse time (`WorkflowParseError::ActionsUnsupportedOnRemoteStep`): the tool roster never reaches the remote dispatch payload today, so a narrowed list there would otherwise be a silent no-op — the remote agent would run with its *full* tool grant while the workflow (and anything reading it) showed the step as narrowed. An empty (or absent) `actions:` stays legal on a remote step.

---

## Template context

Workflow templates use minijinja. Missing variables render as empty strings.

### Template functions

| Function | Returns | Notes |
| --- | --- | --- |
| `read_file('<path>')` | File contents as a string | Path is resolved against the run's working directory. Errors loudly (the run fails) if the file is missing. |

### Template filters

Beyond minijinja's builtins:

| Filter | Returns | Notes |
| --- | --- | --- |
| `fromjson` | The parsed JSON value | The inverse of minijinja's builtin `tojson`. Fails the run if the input is not valid JSON — it never silently yields an empty value. |

`fromjson` is what makes an **`action:` step's output usable**. An action step's
`output` is a JSON *string*, so `{{ steps.<id>.output }}` interpolates the raw
text and cannot be indexed. Pipe it through `fromjson` first:

```yaml
steps:
  - id: fetch
    action: issues.get
    with: { project: "acme/widget", number: 41 }

  - id: triage
    agent: triager
    prompt: |
      Issue title: {{ (steps.fetch.output | fromjson).title }}
      Labels: {{ (steps.fetch.output | fromjson)['labels'] | join(', ') }}
```

Values come back **typed**, so comparisons and arithmetic work:

```yaml
    when: "{{ (steps.fetch.output | fromjson).comments > 5 }}"
```

Approval-gate steps do not need this — a gate's decision is pre-parsed for you
as `steps.<gate-id>.decision`.

`read_file` lets control flow be driven by a **file a prior step wrote** instead of by an agent's chat output, which is far more deterministic. The canonical use is sourcing a `for_each:` list from a file:

```yaml
steps:
  - id: plan
    agent: planner
    actions: []
    prompt: |
      Decide the work items and write them as a JSON array to reports/items.json.
  - id: work
    agent: worker
    actions: []
    # Reads the file the planner wrote — the agent's final message is irrelevant.
    for_each: "{{ read_file('reports/items.json') }}"
    prompt: |
      Process item {{ item }} ({{ loop.index }} / {{ loop.length }}).
```

Because `for_each:` JSON-parses a value that starts with `[`, have the upstream step write a clean JSON array file; the downstream `for_each` then never depends on the agent being terse in chat. The same function works in `prompt:` and `when:` templates.

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

If the workflow is triggered from an event source, the event payload is available under `event.*`. A handful of fields are always present regardless of vendor — `event.id` (the matched event id), `event.vendor`, `event.repo.full_name` (for repo-scoped sources) — but the vendor-native payload for the specific event (e.g. GitHub's own `pull_request`/`issue` object) is nested one level down, under `event.payload.*`, not at the top level:

```yaml
when: "{{ event.payload.pull_request.merged }}"
```

A top-level `event.pull_request.*` (no `payload.` segment) does not exist for a GitHub-sourced event — it renders as an empty string under minijinja's default undefined handling, so a `when:` built on that path is silently, permanently falsy rather than failing loudly.

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
- an `action:` naming an unknown tool, or `with:` failing the tool's schema
- a `branch:` target that doesn't exist, isn't forward, or appears in both arms
- a `join:` with no inbound edges, or a `wait: { count: N }` exceeding its inbound path count
- a cycle anywhere in the workflow's dependency graph

Common design mistakes:

- leaving a shipped `actions:` list incomplete — it now really narrows the connector subset, so a partial list silently drops the tools you forgot
- writing a `branch:` arm that isn't the complete transitive set (see [The transitive-arm rule](#the-transitive-arm-rule)) — this parses fine and fails silently at run time, not at parse time
- making reviewers write-capable
- building one giant workflow instead of using smaller workflows per phase
- relying on fragile free-form prose when a downstream step needs structured output

---

## Practical guidance

- Use [workflow-authoring.md](workflow-authoring.md) when designing new workflows.
- Use [examples/README.md](../examples/README.md) for complete copyable workflows.
- Use [development-flows.md](development-flows.md) for recommended real-world compositions.
