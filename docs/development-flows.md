# Development Flows with rupu

> See also: [using-rupu.md](using-rupu.md) · [workflow-authoring.md](workflow-authoring.md) · [examples/README.md](../examples/README.md)

---

## Goal

These flows show how to use `rupu` for normal software development, from small local tasks to disciplined multi-phase delivery.

The examples here are grounded in the current workflow engine rather than an imaginary future one.

---

## Simple flows

### 1. Review local changes

Use a read-heavy agent against the current checkout.

```sh
rupu run review-diff "check the current diff for bugs and missing tests"
```

Use this for:

- pre-commit review
- branch self-review
- quick validation before opening a PR

### 2. Fix a failing test

```sh
rupu run fix-bug "cargo test parser::tests::rejects_bad_token fails"
```

Good pattern:

- reproduce
- diagnose
- apply the minimal fix
- rerun focused validation

### 3. Add missing coverage

```sh
rupu run add-tests "cover parser::parse_config error cases"
```

Good pattern:

- inspect code and existing tests
- add focused tests only
- keep production edits out unless a real bug blocks correct tests

### 4. Fan out review across a file list

```sh
rupu workflow run review-changed-files --input files=$'src/lib.rs\nsrc/main.rs'
```

Use `for_each:` when the same reviewer should check many independent items.

---

## Standard issue flow

A practical repo-scale flow is:

1. turn the issue into a spec
2. turn the spec into a phased plan
3. execute one phase per PR
4. run an automated review panel on each PR
5. require human merge between phases

That is the model to prefer for substantial work.

---

## Example: issue to spec and plan

Run from the repo checkout:

```sh
rupu workflow run issue-to-spec-and-plan github:your-org/your-repo/issues/42
```

What this should do:

- read the issue in context of the codebase
- create `docs/specs/issue-42.md`
- create `docs/plans/issue-42.md`
- comment back with the proposed phases

This gives the project a stable artifact before code changes begin.

---

## Example: one implementation phase

```sh
rupu workflow run phase-delivery-cycle github:your-org/your-repo/issues/42 --input phase=phase-1
```

What this should do:

- read the plan for the named phase
- implement only that phase
- open a draft PR
- run security, performance, and maintainability reviewers in a panel
- iterate with a fixer agent until findings clear or the loop limit is hit
- pause for human approval before declaring the phase ready for merge

This is the right level of automation for real code work.

---

## Sophisticated phased delivery

If the desired process is:

- pick up a GitHub issue
- understand the ask
- generate a spec
- generate an implementation plan with phases
- work one phase at a time
- create a PR for each phase
- review every PR with a specialist panel
- iterate until findings are cleared
- merge and continue until the issue is fully solved

Then implement it as this sequence:

### Flow A: issue intake

Run:

```sh
rupu workflow run issue-to-spec-and-plan github:your-org/your-repo/issues/42
```

Output:

- stable spec document
- stable phase plan document

### Flow B: phase execution

Run once per phase:

```sh
rupu workflow run phase-delivery-cycle github:your-org/your-repo/issues/42 --input phase=phase-1
```

Output:

- code changes for one phase
- draft PR
- automated panel review
- approval pause before human review / merge

### Flow C: human merge

After the workflow pauses and the PR is satisfactory:

```sh
rupu workflow approve <run-id>
```

Then perform the actual merge through your normal GitHub or GitLab review process.

### Flow D: next phase

After merge, rerun the same phase workflow with the next phase id:

```sh
rupu workflow run phase-delivery-cycle github:your-org/your-repo/issues/42 --input phase=phase-2
```

Repeat until the issue's planned phases are exhausted.

---

## Why this decomposition is better than one giant autonomous workflow

Because it gives you:

- stable artifacts between stages
- one PR per reviewable unit of change
- explicit human control over merge boundaries
- simpler recovery when one phase goes wrong
- better transcripts and auditability

It also matches how real engineering teams already work.

---

## Suggested agent roles for a mature project

A practical baseline set is:

- `issue-understander`
- `spec-writer`
- `phase-planner`
- `repo-investigator`
- `repo-implementer`
- `pr-author`
- `security-reviewer`
- `performance-reviewer`
- `maintainability-reviewer`
- `finding-fixer`
- `issue-commenter`

That is enough to support disciplined end-to-end delivery without pretending the runtime should do everything inside one agent.

---

## When to add event triggers

Add triggers after the manual flow is working.

Good candidates:

- issue labeled `triage` → run issue intake
- PR opened → run automated review panel
- nightly cron → run dependency or security audits

Do not start with triggers if the underlying manual workflow is still unstable.

---

## Where to look next

- [examples/README.md](../examples/README.md) for copyable agent and workflow files
- [workflow-authoring.md](workflow-authoring.md) for design guidance
- [using-rupu.md](using-rupu.md) for operational commands
