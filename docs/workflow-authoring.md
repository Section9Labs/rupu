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
