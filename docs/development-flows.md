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

## Autonomous issue ownership

When the manual phase flow is stable, the same repo can move to an autoflow pattern:

1. attach the repo to a local checkout
2. run a controller autoflow against labeled issues
3. dispatch one child workflow at a time
4. keep one persistent worktree per issue
5. reconcile on a scheduler until the issue is done

### Example: attach and inspect

```sh
rupu repos attach github:your-org/your-repo .
rupu autoflow list
rupu autoflow show issue-supervisor-dispatch
```

### Example: controller autoflow

```sh
rupu autoflow run issue-supervisor-dispatch github:your-org/your-repo/issues/42
```

Expected behavior:

- if the issue already has a live or blocked claim, the manual run fails fast instead of stealing ownership
- if the spec or plan is missing, dispatch `issue-to-spec-and-plan`
- if phase `phase-1` is ready, dispatch `phase-delivery-cycle`
- if a PR is waiting on human merge, return `await_external`
- if all planned phases are done, return `complete`

### Example: scheduled reconciliation

```sh
rupu autoflow tick
```

What the tick should do:

- discover autoflow-enabled workflows
- match candidate issues by selector and `autoflow.priority`
- acquire or refresh the issue claim
- create or reuse the persistent worktree
- run one autonomous cycle
- validate the structured contract output
- persist the next status or dispatch for the next tick

### Controller vs direct child workflows

Two patterns are valid:

- **Controller first**: `issue-supervisor-dispatch` has higher `autoflow.priority` and decides what child workflow should run next.
- **Direct phase owner**: a workflow like `phase-ready-autoflow` matches the issue directly and owns the current phase without a separate controller.

Use the controller pattern for larger repos. It keeps repo-specific decision logic in one place.

### Background scheduling

Recommended model: use `rupu autoflow tick` when you want stateless scheduled reconciliation, or `rupu autoflow serve` when you want one always-on local worker that keeps consuming due wakes with lower latency.

Pick the deployment mode that matches the machine:

- **Laptop / workstation**: poll issues and events locally, then schedule `rupu autoflow tick` or leave `rupu autoflow serve` running in one terminal/session.
- **Dedicated worker box**: run `rupu autoflow serve` continuously and pair it with `rupu webhook serve` if the box is reachable from the internet.
- **Tunneled workstation**: advanced only; place `rupu webhook serve` behind Tailscale, Cloudflare Tunnel, or ngrok if you want webhook latency without a public VM.

- macOS `launchd`: run every 5 or 10 minutes
- Linux `systemd --user` timer or cron
- Windows Task Scheduler

Example crontab:

```text
*/10 * * * * cd /path/to/repo && rupu autoflow tick
```

Example `launchd` program arguments:

```text
/bin/zsh -lc 'cd /path/to/repo && rupu autoflow tick'
```

Example `systemd --user` service command:

```text
WorkingDirectory=/path/to/repo
ExecStart=/usr/bin/env rupu autoflow tick
```

Example long-running `systemd --user` worker command:

```text
WorkingDirectory=/path/to/repo
ExecStart=/usr/bin/env rupu autoflow serve --repo github:your-org/your-repo --worker build-box-01
```

Example Windows Task Scheduler program/script:

```text
Program/script: rupu
Arguments: autoflow tick
Start in: C:\\path\\to\\repo
```

Example direct long-running local worker:

```sh
rupu autoflow serve --repo github:your-org/your-repo --worker laptop-01
```

Use event or webhook wakeups only after the autonomous loop is stable on periodic ticks. Webhook mode still feeds back into the same autoflow runtime; it shortens wakeup latency but does not replace the periodic reconciliation loop. If you run `rupu webhook serve` from outside a repo checkout, attach the repo first so tracked repo workflows remain visible to the receiver.

### Operator recovery workflow

When an autonomous issue looks stuck, keep the operator loop narrow and deterministic:

```sh
rupu autoflow status --repo github:your-org/your-repo
rupu autoflow wakes --repo github:your-org/your-repo
rupu autoflow explain github:your-org/your-repo/issues/42
rupu autoflow doctor --repo github:your-org/your-repo
rupu autoflow repair github:your-org/your-repo/issues/42
rupu autoflow requeue github:your-org/your-repo/issues/42 --event github.issue.reopened --not-before 5m
```

Use the tools in that order:

- `status` tells you whether the repo is generally healthy or blocked on a few issues
- `wakes` shows whether the issue is actually queued for another pass
- `explain` gives the exact claim, run, wake, and dispatch context for one issue
- `doctor` surfaces state mismatches before you mutate anything
- `repair` applies bounded fixes such as rebuilding a missing worktree or dropping a broken queued wake
- `requeue` asks for one more pass without waiting for the next natural wake source

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

Add triggers after the manual or autoflow path is working.

Good candidates:

- issue labeled `triage` → run issue intake
- PR opened → run automated review panel
- issue labeled `autoflow` → let `rupu autoflow tick` pick it up
- nightly cron → run dependency or security audits

Do not start with triggers if the underlying manual workflow is still unstable.

---

## Where to look next

- [examples/README.md](../examples/README.md) for copyable agent and workflow files
- [workflow-authoring.md](workflow-authoring.md) for design guidance
- [using-rupu.md](using-rupu.md) for operational commands
