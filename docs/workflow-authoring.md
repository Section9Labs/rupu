# Writing Good Workflows

> See also: [workflow-format.md](workflow-format.md) · [agent-authoring.md](agent-authoring.md) · [development-flows.md](development-flows.md)

---

## What a good workflow does

A good workflow makes the process explicit.

It should answer four questions:

1. What enters the workflow?
2. Which specialist owns each stage?
3. Where are the review and approval boundaries?
4. What output or side effect should exist at the end?

If the workflow cannot answer those questions, it is not ready to automate.

---

## Choose the right step shape

### Linear step

Use when one output naturally feeds the next step.

Examples:

- investigate → implement
- understand issue → write spec → write plan
- prepare release notes → deploy → announce

### `for_each:`

Use when one agent should process many items independently.

Examples:

- review a set of files
- generate test cases for a list of endpoints
- summarize several modules one by one

### `parallel:`

Use when different specialists should look at the same subject.

Examples:

- security + performance + maintainability reviews over one diff
- frontend + backend + infra review over one design proposal

### `panel:`

Use when you want a structured review surface plus an optional fix loop.

Examples:

- PR review before human approval
- design review before implementation starts
- release review before rollout

---

## Design rules

### 1. Keep each step accountable

Each step should have one clear owner:

- `issue-understander`
- `spec-writer`
- `phase-planner`
- `repo-implementer`
- `pr-author`
- `security-reviewer`

If multiple responsibilities are hidden inside one step, review becomes difficult.

### 2. Use inputs for external parameters

Prefer:

```yaml
inputs:
  phase:
    type: string
    required: true
```

Over burying operational parameters inside a free-form prompt.

### 3. Make downstream dependencies explicit

If a later step depends on a prior step, shape the earlier output so the later step can consume it reliably.

Examples:

- first line must be `PR: github:owner/repo#123`
- write spec to `docs/specs/issue-42.md`
- output numbered phases with stable names

### 4. Separate implementers from reviewers

A durable pattern is:

1. investigator or planner
2. implementer
3. panel reviewers
4. fixer agent for reviewer findings
5. human approval

That pattern is easier to reason about than one self-reviewing agent.

### 5. Put approvals at ownership boundaries

Good approval points:

- before deploy
- before a status comment that declares something ready for merge
- before a destructive production action
- before phase transitions when humans still own the merge decision

### 6. Keep workflows phase-sized

For large project work, prefer multiple workflows over one endless autonomous loop.

A reliable decomposition is:

- issue intake workflow
- spec + phase plan workflow
- one workflow per implementation phase
- optional event-triggered follow-up workflows

That is easier to observe, approve, and recover.

### 7. Put autonomous policy in `autoflow:`, not in ad-hoc prose

If a workflow should own an issue over time, declare that explicitly:

```yaml
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["autoflow"]
  reconcile_every: "10m"
```

Use `autoflow:` for:

- ownership and matching
- worktree strategy
- lease duration
- wake hints
- which structured output the runtime should consume

Do not bury those rules in a prompt and expect the runtime to infer them later.

### 8. Put workflow handoffs behind contracts

If one workflow hands off to another, declare the output contract and keep the schema in `.rupu/contracts/`.

Good pattern:

```yaml
contracts:
  outputs:
    result:
      from_step: handoff
      format: json
      schema: autoflow_outcome_v1
```

That gives you:

- machine-readable validation
- durable child-dispatch payloads
- reviewable repo-local protocol files

Without that, you are asking the runtime to parse prose. That does not scale.

---

## Recommended structure for real code work

### Simple bugfix

- investigate
- implement
- summarize verification

### Normal issue delivery

- understand issue
- write spec
- write phase plan
- implement one phase
- open draft PR
- run review panel
- fix findings
- pause for human review

### High-discipline delivery

- issue intake
- spec generation
- phased plan generation
- one PR per phase
- automated panel review for every PR
- human merge between phases
- rerun the phase workflow for the next phase

### Autonomous issue ownership

- controller autoflow selects candidate issues
- controller emits `dispatch` when a child workflow should run
- child workflow owns one implementation phase or review cycle
- child workflow emits a structured handoff back to the controller
- `rupu autoflow tick` reconciles the repo repeatedly

This is the right model when you want autonomy without inventing a second orchestration language.

This is the practical way to get "agentic orchestration" without pretending the system has an infinite autonomous planning loop.

---

## Anti-patterns

| Anti-pattern | Why it is weak | Better design |
| --- | --- | --- |
| One workflow tries to solve the entire issue forever | Hard to pause, audit, and recover | Split by phase |
| Reviewers can edit code directly | Review signal becomes untrustworthy | Use a separate fixer agent |
| No stable artifacts | Later steps depend on vague prose | Write spec / plan files or emit structured output |
| No explicit human gate | Risky actions happen too easily | Add `approval:` where ownership changes |
| Huge prompts with hidden assumptions | Maintenance cost grows quickly | Keep prompts narrow and named |
| Using `actions:` as a tool policy | It does not do that | Put tool limits in the agent file |
| Parsing workflow handoffs from prose | Breaks under real automation | Declare `contracts:` and validate them |
| Child autoflows with no owner precedence | Matching becomes non-deterministic | Use `autoflow.priority` intentionally |

---

## Workflow authoring checklist

Before you commit a workflow, verify that:

- every referenced agent exists
- reviewers are read-only
- implementers have a clear validation bar
- approvals sit at meaningful risk boundaries
- `for_each`, `parallel`, and `panel` are used only where they help
- outputs consumed by later steps are shaped intentionally
- large efforts are split into reviewable phases
- autoflow ownership is explicit when the workflow is meant for `rupu autoflow`
- matching precedence is deliberate when more than one autoflow can select the same issue
- every autonomous handoff has a declared contract schema under `.rupu/contracts/`

---

## Autoflow authoring patterns

### Controller autoflow

Use when the repo needs one top-level policy owner that decides what to do next.

Recommended shape:

- high `autoflow.priority`
- broad issue selector such as `labels_all: ["autoflow"]`
- optional `labels_any` / `labels_none` when the controller should narrow by readiness or exclude blocked work
- one step that emits `autoflow_outcome_v1`
- `dispatch` to `issue-to-spec-and-plan` or `phase-delivery-cycle`

### Direct phase autoflow

Use when phase execution should take over immediately once the issue hits a specific state.

Recommended shape:

- lower priority than the controller by default
- label-scoped selector such as `labels_all: ["autoflow", "phase:phase-1"]`
- use `labels_any` for alternate readiness labels and `labels_none` to exclude pause labels such as `blocked`
- final step emits `autoflow_outcome_v1`
- handoff can dispatch back to the controller after the phase is ready

If you want the direct phase workflow to win instead of the controller, raise its `autoflow.priority`.

### Repo binding and scheduling

Autoflows do not discover repos from your shell `cwd`. They need an explicit repo binding:

```sh
rupu repos attach github:your-org/your-repo .
```

Then reconcile with:

```sh
rupu autoflow list
rupu autoflow tick
```

Schedule `rupu autoflow tick` outside the workflow YAML:

- macOS: `launchd`
- Linux: `systemd --user` timer or cron
- Windows: Task Scheduler

---

## Practical recommendation for sophisticated delivery

If the desired process is:

- pick up a GitHub issue
- understand the ask
- generate a spec
- produce a phased plan
- implement one phase per PR
- review each PR with a panel
- iterate until findings are cleared
- merge and continue to the next phase

Then model it as a workflow family, not one monolith:

1. `issue-to-spec-and-plan`
2. `phase-delivery-cycle`
3. human merge
4. rerun `phase-delivery-cycle` for the next phase

That is the most honest and controllable way to represent phased project work in current `rupu`.
