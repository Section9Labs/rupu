# rupu Autoflows — Design

**Date:** 2026-05-08
**Status:** Design (pre-plan)
**Companion docs:** [Slice A design](./2026-05-01-rupu-slice-a-design.md), [Slice B-2 design](./2026-05-03-rupu-slice-b2-scm-design.md), [Workflow triggers design](./2026-05-07-rupu-workflow-triggers-design.md)

---

## 1. What this is

This design adds **autonomous, persistent workflow ownership** to `rupu` without introducing a second orchestration language.

The key decision:

- `workflow` remains the unit of execution
- `autoflow` becomes the persistent, lifecycle-oriented runner for workflows
- the **same YAML file** can run in manual mode or autonomous mode

An autoflow is therefore:

- not a new file type
- not a second step DSL
- not a replacement for workflows

It is a workflow plus an optional top-level `autoflow:` block, executed by a new top-level CLI surface: `rupu autoflow ...`.

## 2. Why this is needed

`rupu` already has:

- workflow step composition
- issue / SCM integration
- run persistence
- approval gates
- cron and event triggers

But those answer only:

- **how does one workflow run execute?**
- **when should one workflow run start?**

They do **not** answer:

- which local checkout owns a repo when the user is not currently in it?
- which issue is already claimed?
- how do we revisit an issue after a PR merge or a backoff timer?
- how do we persist per-issue progress across many workflow runs?
- how do we safely keep one autonomous branch/worktree per issue?

That missing layer is what autoflows add.

## 3. Core design decisions

### 3.1 One YAML language

There is **one** workflow YAML language. Autoflow metadata extends it.

### 3.2 Separate top-level CLI surface

Autonomy should feel like a first-class operating mode, not an obscure workflow flag. The CLI should therefore be:

- `rupu workflow ...` for direct execution
- `rupu autoflow ...` for persistent autonomous execution

This is a product distinction, not a language split.

### 3.3 Thin runtime, workflow-centric logic

The autoflow runtime should own only:

- candidate discovery
- claims
- repo → local path resolution
- worktree lifecycle
- scheduling / retries / reconciliation
- structured outcome handling

Repo-specific reasoning and delivery logic should remain in workflows.

### 3.4 No daemon requirement in v1

The first implementation should be **idempotent tick-based**, not daemon-first.

- `rupu autoflow tick` is the primary runtime
- OS schedulers invoke it periodically
- a future long-lived `rupu autoflow serve` can be added later if needed

### 3.5 Contracts are explicit and versioned

Workflow-to-workflow and workflow-to-runtime communication must be machine-readable and validated. Do not parse prose.

Contracts should live in:

```text
.rupu/contracts/
```

and be referenced from workflow YAML.

### 3.6 Reuse the existing event ingress

Autoflows must reuse the same event-ingestion infrastructure already designed for workflow triggers:

- `[triggers].poll_sources`
- `EventConnector`
- `cron-state/event-cursors`
- `rupu webhook serve`

Autoflows do not get a second polling configuration or a second cursor store.

## 4. Goals

1. Let the same workflow file run manually or autonomously.
2. Keep workflows as the only user-facing orchestration language.
3. Add persistent issue ownership across many workflow runs.
4. Support repo-local supervision policy in `<repo>/.rupu/config.toml`.
5. Add durable per-issue worktrees instead of temp clones.
6. Make autonomous chaining depend on explicit structured contracts.
7. Work cleanly on macOS, Linux, and Windows.

## 5. Non-goals

- A second orchestration DSL
- A visual GUI / SaaS control plane
- Replacing `trigger:` with autoflows
- Full general-purpose workflow-to-workflow DAG scheduling
- Hidden always-on background agents in v1
- Parsing arbitrary prose as inter-workflow protocol

## 6. Relationship to existing concepts

| Concept | Purpose |
|---|---|
| `rupu workflow run` | Execute one workflow now |
| `trigger:` | Decide when a one-shot workflow run may fire |
| `rupu cron tick` / `rupu webhook serve` | Feed event/cron-triggered workflow runs |
| `rupu autoflow tick` | Reconcile and advance persistent autonomous ownership |

### 6.1 Triggers are not removed

Existing workflow triggers remain valid for one-shot workflows.

Autoflows solve a different problem:

- triggers = run initiation
- autoflows = persistent lifecycle ownership

### 6.2 Why not reuse `trigger:` as autoflow wakeup

Directly reusing `trigger:` for autoflows creates an ambiguity:

- for workflows, `trigger:` means **dispatch this workflow now**
- for autoflows, the correct behavior is usually **mark this item dirty and reconcile it**

These are different semantics. To keep the system legible, autoflows get their own `wake_on:` concept while reusing the same event vocabulary.

### 6.3 Event ingress is still shared

Autoflows do **not** introduce a second event-ingestion subsystem.

The same event ids and the same `[triggers].poll_sources` gating continue to apply. Autoflows consume those events differently: they treat them as reconciliation hints rather than direct one-shot dispatch commands.

## 7. CLI surface

New top-level command family:

```text
rupu autoflow list
rupu autoflow show <name>
rupu autoflow run <name> <target>
rupu autoflow tick
rupu autoflow status
rupu autoflow claims
rupu autoflow release <issue-ref>
```

### 7.1 Command meanings

| Command | Purpose |
|---|---|
| `rupu autoflow list` | Show workflow files that declare `autoflow.enabled = true` |
| `rupu autoflow show <name>` | Print the workflow file and resolved autoflow metadata |
| `rupu autoflow run <name> <target>` | Execute one autonomous cycle for one entity, without scheduler discovery |
| `rupu autoflow tick` | Scan all enabled autoflows, reconcile eligible entities, and advance work |
| `rupu autoflow status` | Summarize active / waiting / retrying / complete autonomous entities |
| `rupu autoflow claims` | Inspect the claim store directly |
| `rupu autoflow release <issue-ref>` | Force-release a stuck claim |

### 7.2 Relationship to `workflow run`

`rupu workflow run <name> ...` continues to work for autoflow-enabled workflow files.

When invoked through `workflow run`:

- the `steps:` execute normally
- `autoflow:` metadata is ignored except for contract validation and any explicit inputs the author chooses to pass

When invoked through `autoflow ...`:

- the runtime activates claim / worktree / outcome / retry behavior

This gives one file two operating modes.

For v1, `target` is expected to be an issue ref when `entity: issue`:

```text
github:owner/repo/issues/42
gitlab:group/project/issues/9
```

`rupu autoflow run` must hard-error on any other target shape in v1. It must not inherit the current one-shot workflow behavior that silently falls back to `cwd` when the target cannot be parsed.

## 8. Discovery model

`rupu autoflow tick` cannot rely on the current `cwd`-rooted project discovery model alone.

It must discover autoflow definitions from:

1. `~/.rupu/workflows/`
2. each preferred repo checkout recorded in the global repo registry

This is a new lookup path. It is required so autonomous management can continue when the operator is not currently inside the repo directory.

## 9. Workflow YAML extension

Autoflows extend the existing workflow schema with two new optional top-level blocks:

- `autoflow:`
- `contracts:`

### 9.1 `autoflow:` block

```yaml
autoflow:
  enabled: true
  entity: issue
  priority: 100

  selector:
    states: ["open"]
    labels_all: ["autoflow"]
    limit: 100

  wake_on:
    - github.issue.opened
    - github.issue.labeled
    - github.pr.merged

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

Duration-like fields in v1 use a compact relative-duration grammar:

- `10m`
- `3h`
- `7d`

Supported units: `s`, `m`, `h`, `d`.

Only the following autoflow fields are template-rendered in v1:

- `workspace.branch`

Autoflow template rendering must use strict undefined handling. Missing variables are a protocol error in autoflow mode.

### 9.2 Field meanings

| Field | Meaning |
|---|---|
| `enabled` | Marks this workflow as autonomously runnable |
| `entity` | Entity type the autoflow owns; v1 supports `issue` |
| `priority` | Match precedence when multiple autoflows select the same issue; higher wins, default `0` |
| `selector` | Candidate filter over issues |
| `wake_on` | Event ids that should mark candidate items dirty for reconciliation |
| `reconcile_every` | Maximum time between reconciliations for an owned entity |
| `claim` | Claiming policy |
| `workspace` | Persistent checkout/worktree policy |
| `outcome` | Which declared workflow output the runtime should consume |

### 9.3 v1 selector model

The selector surface is intentionally conservative in v1.

Portable fields:

- `states`
- `labels_all`
- `limit`

Deferred until the issue connector contract grows:

- `labels_any`
- `labels_none`
- `query`

### 9.4 v1 entity model

v1 supports:

- `entity: issue`

Future entities may include:

- `pr`
- `repo`
- `queue_item`

### 9.5 Multiple matches and precedence

It must be legal for more than one autoflow to match the same issue. V1 resolves that deterministically:

1. evaluate every autoflow whose `entity` and `selector` match
2. choose the candidate with the highest `priority`
3. on equal priority, choose the workflow whose `name` sorts first lexicographically
4. only the winning autoflow may create or retain the active claim for that issue

Default priority is `0`.

This keeps controller-style and direct phase workflows composable without hidden first-match behavior.

### 9.6 Priority examples

**Example 1 — controller beats direct phase**

```yaml
name: issue-supervisor-dispatch

autoflow:
  enabled: true
  entity: issue
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["autoflow"]
```

```yaml
name: phase-delivery-cycle

autoflow:
  enabled: true
  entity: issue
  priority: 50
  selector:
    states: ["open"]
    labels_all: ["autoflow", "phase:phase-1"]
```

If an issue has both labels, both autoflows match. `issue-supervisor-dispatch` wins because `100 > 50`.

**Example 2 — direct phase takes over by design**

```yaml
name: phase-delivery-cycle

autoflow:
  enabled: true
  entity: issue
  priority: 200
  selector:
    states: ["open"]
    labels_all: ["autoflow", "phase:phase-1"]
```

Now the direct phase autoflow wins because `200 > 100`. This is appropriate when a repo wants to bypass a controller once a planning label is present.

**Example 3 — tie-break**

If two autoflows both have `priority: 100`, the one whose workflow name sorts first wins. That rule must be documented in CLI status output so operators can understand why one autoflow owns an issue.

## 10. Contracts

Contracts define machine-readable output schemas between:

- a step and the runtime
- a workflow and another workflow
- an autoflow controller and a child workflow

### 10.1 Workflow-level declaration

```yaml
contracts:
  outputs:
    result:
      from_step: finalize
      format: json
      schema: autoflow_outcome_v1
```

This says:

- the workflow's canonical output is named `result`
- it comes from step `finalize`
- it must be JSON
- it must validate against `autoflow_outcome_v1`

`contracts.outputs.*` is the single source of truth for:

- `from_step`
- `format`
- `schema`

`autoflow.outcome.output` only names which declared workflow output the autoflow runtime should consume.

### 10.2 Step-level declaration

```yaml
steps:
  - id: finalize
    agent: issue-commenter
    contract:
      emits: autoflow_outcome_v1
      format: json
    prompt: |
      Return only valid JSON for autoflow_outcome_v1.
```

Step-level `contract:` is optional authoring metadata. It exists to make the step's output expectations explicit to humans and prompts.

Runtime authority still lives at the workflow level in `contracts.outputs.*`. If step-level metadata disagrees with the workflow-level declaration, the workflow-level declaration wins and validation should fail loudly.

### 10.3 Contract storage

Contracts live in:

```text
<repo>/.rupu/contracts/
~/.rupu/contracts/
```

Resolution mirrors agents/workflows:

1. project contract wins by name
2. global contract is fallback

### 10.4 Contract format

v1 contract documents should be JSON Schema files validated with the workspace's existing JSON Schema stack (`jsonschema` / `schemars` already ship in the workspace dependency set):

```text
.rupu/contracts/autoflow_outcome_v1.json
.rupu/contracts/workflow_dispatch_v1.json
.rupu/contracts/phase_plan_v1.json
.rupu/contracts/review_packet_v1.json
```

Using JSON Schema avoids inventing a custom validation language.

### 10.5 Why contracts are workflow-centric

Agents are reusable. The same agent may emit different structures in different workflows.

Therefore:

- agents may suggest formats
- workflows declare the real contract
- runtime validates the declared contract

## 11. Canonical contracts for v1

### 11.1 `autoflow_outcome_v1`

Used by the autoflow runtime to decide what to do next.

Example:

```json
{
  "status": "await_human",
  "summary": "Draft PR opened and panel findings addressed",
  "pr_url": "https://github.com/org/repo/pull/123",
  "next_phase": "phase-2",
  "dispatch": {
    "workflow": "phase-delivery-cycle",
    "target": "github:org/repo/issues/42",
    "inputs": {
      "phase": "phase-2"
    }
  }
}
```

Suggested statuses:

- `continue`
- `await_human`
- `await_external`
- `retry`
- `blocked`
- `complete`

### 11.2 `workflow_dispatch_v1`

Used when one workflow explicitly decides which other workflow should run next.

Example:

```json
{
  "workflow": "phase-delivery-cycle",
  "target": "github:org/repo/issues/42",
  "inputs": {
    "phase": "phase-1"
  },
  "summary": "Spec exists and phase 1 is ready"
}
```

### 11.3 `phase_plan_v1`

Used between intake/planning and phase-delivery workflows.

### 11.4 `review_packet_v1`

Used for PR review / human handoff summaries.

## 12. Schema delta

This design requires explicit schema extension in the existing Rust config/workflow types.

At minimum, the implementation must extend:

- `crates/rupu-orchestrator/src/workflow.rs`
  - `Workflow`
  - `Step`
  - new supporting structs for `Autoflow`, `Contracts`, and step-level `Contract`
- `crates/rupu-config/src/config.rs`
  - `Config`
  - new `AutoflowConfig`

And the public reference docs must be updated alongside implementation:

- `docs/workflow-format.md`
- `docs/workflow-authoring.md`
- `docs/using-rupu.md`

This matters because current `deny_unknown_fields` behavior would reject every new autoflow/contract field until the schema is extended.

## 13. Repo-local config

Autoflows need repo-wide operational defaults that are awkward to repeat in every workflow.

Add a top-level `[autoflow]` config section in the existing merged config model at `~/.rupu/config.toml` and `<repo>/.rupu/config.toml`.

Example:

```toml
[autoflow]
enabled = true
repo = "github:Section9Labs/rupu"
checkout = "worktree"
worktree_root = "~/.rupu/autoflows/worktrees"
permission_mode = "bypass"
strict_templates = true
reconcile_every = "10m"
max_active = 2
claim_ttl = "3h"
cleanup_after = "7d"
```

### 13.1 Config responsibility split

Use workflow YAML for:

- issue selection
- autonomous behavior
- repo process logic
- structured outputs
- branch templating
- logical retry cadence
- logical claim TTL

Use config for:

- local machine paths
- execution defaults
- repo binding / local checkout defaults
- scheduler-safe defaults
- local concurrency caps
- cleanup policies

Machine-local path values such as `worktree_root` belong in config only; they should not be declared in workflow YAML.

### 13.2 Precedence

For autonomous execution settings, precedence should be:

1. explicit CLI override
2. workflow `autoflow.*` logical settings
3. `[autoflow]` in project config
4. `[autoflow]` in global config
5. runtime default

### 13.3 Layering

The existing config merge rules already fit this model:

- tables merge recursively
- arrays replace
- project config overrides global

That behavior is already defined in `crates/rupu-config/src/layer.rs`.

## 14. Global runtime state

Autoflows need new global state in `~/.rupu/`:

```text
~/.rupu/
  config.toml
  workspaces/
  runs/
  repos/                         # NEW repo-ref -> local path mapping
  autoflows/
    claims/                      # NEW one file per owned entity
    worktrees/                   # NEW persistent per-entity worktrees
    logs/                        # NEW supervisor logs
```

### 14.1 Repo registry

Current workspace records are keyed by path. Autoflows need the reverse mapping:

- repo ref → preferred local path

Example:

```toml
repo_ref = "github:Section9Labs/rupu"
preferred_path = "/Users/matt/Code/Oracle/rupu"
known_paths = ["/Users/matt/Code/Oracle/rupu"]
origin_urls = [
  "git@github.com:Section9Labs/rupu.git",
  "https://github.com/Section9Labs/rupu.git"
]
default_branch = "main"
last_seen_at = "2026-05-08T18:00:00Z"
```

This is machine-local state, not repo-committed state.

### 14.2 Claim store

Example claim file:

```toml
issue_ref = "github:Section9Labs/rupu/issues/100"
repo_ref = "github:Section9Labs/rupu"
workflow = "issue-supervisor-dispatch"
status = "await_human"
worktree_path = "/Users/matt/.rupu/autoflows/worktrees/github--Section9Labs--rupu/issue-100"
branch = "rupu/issue-100"
last_run_id = "run_01J..."
last_error = ""
next_retry_at = "2026-05-08T20:15:00Z"
claim_owner = "host:user:pid"
lease_expires_at = "2026-05-08T23:00:00Z"
pending_dispatch_workflow = "phase-delivery-cycle"
updated_at = "2026-05-08T20:00:00Z"
```

Claims should be lock-backed to prevent duplicate ownership.

### 14.3 Claim locking and leases

V1 must define exact stale-claim behavior. Minimum rules:

1. each claim has an owner id and lease expiry
2. each active autoflow cycle holds an exclusive lock file and renews the lease while running
3. a second process may steal only expired claims whose active lock is absent
4. approval-paused and externally-waiting claims remain owned via the lease, but do not hold a long-lived active-cycle lock
5. `claim.ttl` remains part of the autoflow contract in v1; it governs lease duration, not just retry policy
6. release is explicit on terminal completion or manual operator action

This behavior must be specified precisely in the implementation plan; "lock-backed" alone is not sufficient for cross-platform reliability.

## 15. Persistent workspaces

Autoflows should not use the current repo/PR temp clone behavior used by direct workflow runs.

Instead, they should prefer:

- `workspace.strategy = worktree`

Default layout:

```text
~/.rupu/autoflows/worktrees/github--owner--repo/issue-42/
```

Recommended branch naming:

- `rupu/issue-42`
- or `rupu/issue-42/phase-1` for phase-isolated repos

### 15.1 Why worktrees

- does not mutate the user's main working tree
- supports one issue = one durable branch
- allows resume and retry
- makes cleanup explicit
- allows concurrent autonomous issue ownership

## 16. Runtime architecture

```
event pollers / webhook hints / scheduler
                 │
                 ▼
          rupu autoflow tick
                 │
       ┌─────────┼─────────┐
       │         │         │
       ▼         ▼         ▼
 candidate   claim store   repo registry
 discovery      │              │
       │         └──────┬───────┘
       ▼                ▼
  worktree manager ─▶ workflow dispatcher
                            │
                            ▼
                    existing run_workflow
                            │
                            ▼
                        RunStore
                            │
                            ▼
                     outcome validator
                            │
                            ▼
                     claim/state update
```

### 16.1 New internal entrypoint

Autoflows should not route through the current `workflow run` CLI wrapper.

They need a dedicated internal runtime entrypoint that accepts explicit:

- `project_root`
- `workspace_path`
- `workspace_id`
- `event`
- `issue`
- deterministic `run_id` when applicable

The current wrapper is optimized for interactive `cwd`-relative execution and is not safe for background autonomous issue ownership.

### 16.2 Architecture rule

The autoflow runtime must remain a thin caller over the existing workflow engine. It must not become a second step runner.

## 17. Lifecycle state model

Autoflow claim state and workflow run state are related but not identical.

The claim store needs its own lifecycle:

- `eligible`
- `claimed`
- `running`
- `await_human`
- `await_external`
- `retry_backoff`
- `blocked`
- `complete`
- `released`

The last workflow run still records normal `RunStatus`:

- `pending`
- `running`
- `completed`
- `failed`
- `awaiting_approval`
- `rejected`

Autoflow reconciliation must interpret both.

### 17.1 Approval interaction

When a workflow run pauses at a normal step `approval:` gate:

- run status becomes `awaiting_approval`
- claim lifecycle becomes `await_human`

This mapping must be explicit so operator UX and retries stay coherent.

## 18. Tick algorithm

For each enabled autoflow:

1. Resolve repo-local config and autoflow metadata.
2. Resolve repo binding from the repo registry.
3. Discover candidate issues from the issue tracker.
4. Filter by `selector`.
5. Merge with existing claim state.
6. For each eligible issue:
   1. acquire claim lock
   2. resolve or create worktree
   3. decide whether issue is due:
      - first seen
      - wake event seen
      - reconcile interval elapsed
      - retry backoff elapsed
   4. run the workflow in autoflow mode
   5. read the declared structured outcome
   6. validate contract
   7. update claim state
   8. persist any requested child dispatch onto the claim state for the next reconciliation cycle
7. Release or retain claim according to outcome state.

The entire algorithm must be idempotent. Running two ticks close together must not duplicate ownership or dispatch.

## 19. Execution semantics

### 19.1 `rupu autoflow run <name> <target>`

Runs one autoflow cycle for one entity:

- useful for debugging
- bypasses discovery
- still uses claim / worktree / contract semantics

### 19.2 `rupu autoflow tick`

Runs discovery + reconciliation across all autoflows once, then exits.

This is the primary v1 surface.

Child workflow dispatch requested by an autoflow outcome is not executed inline in the same tick. It is persisted onto claim state and picked up on the next reconciliation cycle. This keeps dispatch idempotent and makes crash recovery easier to reason about.

### 19.3 Future `rupu autoflow serve`

Possible future command:

```text
rupu autoflow serve
```

This would be a long-lived foreground reconciler. It is explicitly deferred; v1 should rely on `tick`.

## 20. Cross-platform background operation

### 20.1 macOS

Use `launchd` LaunchAgents to run:

```text
rupu autoflow tick
```

on a fixed interval.

Optional:

- separate LaunchAgent for `rupu webhook serve`

### 20.2 Linux

Preferred:

- `systemd --user` timer + service

Fallback:

- cron

### 20.3 Windows

Preferred:

- Task Scheduler

Run:

```text
rupu autoflow tick
```

every N minutes.

### 20.4 Why no daemon first

Tick-based execution is:

- easier to recover
- easier to reason about
- simpler to deploy cross-platform
- compatible with the existing CLI model

## 21. Safety and unattended behavior

Autoflows are unattended by design. Therefore:

- `permission_mode = ask` must be rejected
- `strict_templates = true` should be the default
- contract validation failures should fail the cycle loudly
- one active claim per issue must be enforced
- one autoflow run should not overwrite another issue's worktree

Autoflow outcomes must validate successfully against their declared JSON Schema contract before the runtime treats them as authoritative. Invalid structured output is a protocol failure.

The current template system renders missing variables as empty strings. Autoflows should offer a strict mode so unattended failures do not silently degrade behavior.

Similarly, contract validation failures should be terminal for the cycle by default. A malformed structured result is a runtime protocol failure, not just a weak warning.

## 22. Example controller autoflow

```yaml
name: issue-supervisor-dispatch
description: Decide what workflow should run next for an issue.

autoflow:
  enabled: true
  entity: issue
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["autoflow"]
    limit: 100
  wake_on:
    - github.issue.opened
    - github.issue.labeled
    - github.pr.merged
  reconcile_every: "10m"
  claim:
    key: issue
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result

contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1

steps:
  - id: decide
    agent: issue-understander
    contract:
      emits: autoflow_outcome_v1
      format: json
    prompt: |
      Decide what should happen next for issue #{{ issue.number }}.

      Return only valid JSON for autoflow_outcome_v1.
```

This pattern keeps repo-specific process logic inside the workflow while the runtime handles persistence.

### 22.1 Example direct phase autoflow

```yaml
name: phase-delivery-cycle
description: Deliver one planned phase when the issue is phase-ready.

autoflow:
  enabled: true
  entity: issue
  priority: 50
  selector:
    states: ["open"]
    labels_all: ["autoflow", "phase:phase-1"]
    limit: 100
  wake_on:
    - github.issue.labeled
    - github.pr.merged
  reconcile_every: "10m"
  claim:
    key: issue
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result

contracts:
  outputs:
    result:
      from_step: finalize
      format: json
      schema: autoflow_outcome_v1
```

This pattern is appropriate when the repo wants a workflow to advance phase work directly once a planning label or state is present.

## 23. Acceptance criteria

This design is correct if:

1. users author one YAML language, not two
2. the autoflow runtime reuses the existing workflow engine
3. repo-local config can govern autonomous behavior
4. one issue can be owned across many workflow runs without temp-clone loss
5. cross-workflow communication is machine-readable and validated
6. the first implementation works on macOS, Linux, and Windows with no mandatory daemon
7. the architecture does not create a second step DSL or a second run engine
8. multiple matching autoflows resolve ownership deterministically via `priority`, then workflow name

## 24. Open follow-on items

- exact Rust config structs for `[autoflow]`
- exact claim-lock format and locking strategy
- exact `autoflow_outcome_v1` schema contents
- exact repo registry command surface under `rupu repos` (`attach`, `prefer`, `tracked`, `forget`)
- optional `autoflow serve`

Those belong in the implementation plan, not this design.
