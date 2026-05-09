# Using rupu

> See also: [agent-format.md](agent-format.md) · [workflow-format.md](workflow-format.md) · [development-flows.md](development-flows.md) · [examples/README.md](../examples/README.md)

---

## What rupu is for

`rupu` is a local-first CLI for running coding agents and orchestrating them as workflows.

Use it when you want:

- checked-in agent prompts under version control
- repeatable multi-step engineering flows
- SCM / issue integration through one tool surface
- transcripts and run history for auditability
- human approval gates at meaningful boundaries

---

## Project layout

Inside a repo, `rupu` looks for:

```text
<repo>/.rupu/
  agents/
  contracts/
  workflows/
  config.toml
```

Global state lives under:

```text
~/.rupu/
  agents/
  contracts/
  workflows/
  config.toml
  auth.json
  repos/
  autoflows/
  transcripts/
  runs/
```

Project-local agents and workflows shadow global ones with the same `name:`.

---

## First-time setup

### 1. Bootstrap a repo

```sh
rupu init --with-samples --git
```

This creates `.rupu/agents/`, `.rupu/workflows/`, `.rupu/config.toml`, and the sample set also seeds `.rupu/contracts/`.

### 2. Authenticate at least one model provider

```sh
rupu auth login --provider anthropic --mode sso
```

Other common variants:

```sh
rupu auth login --provider openai --mode api-key --key sk-...
rupu auth login --provider gemini --mode api-key --key ...
rupu auth login --provider copilot --mode sso
```

Check status:

```sh
rupu auth status
```

### 3. Configure SCM defaults if you will use PR or issue tools

Add this to `~/.rupu/config.toml` or `<repo>/.rupu/config.toml`:

```toml
[scm.default]
platform = "github"
owner = "your-org"
repo = "your-repo"

[issues.default]
tracker = "github"
project = "your-org/your-repo"
```

### 4. Attach the repo if you want autonomous issue ownership

Autoflows need a repo-to-local-checkout binding:

```sh
rupu repos attach github:your-org/your-repo .
```

Manual local-checkout commands also auto-track the current repo when `origin` is parseable:

- `rupu run ...`
- `rupu workflow run ...`
- `rupu issues ...`

Use `rupu repos attach` or `rupu repos prefer` when you want to seed the binding explicitly or switch the preferred checkout.

Optional autonomous defaults:

```toml
[autoflow]
enabled = true
repo = "github:your-org/your-repo"
permission_mode = "bypass"
strict_templates = true

[triggers]
poll_sources = ["github:your-org/your-repo"]
```

Use `[triggers].poll_sources` when you want `autoflow.wake_on` to react before the next `reconcile_every` deadline.

---

## Day-to-day commands

### Run a single agent

```sh
rupu run review-diff "check staged changes for bugs and missing tests"
```

### Run an agent against a PR target

```sh
rupu run scm-pr-review github:your-org/your-repo#42
```

### Run a workflow with inputs

```sh
rupu workflow run review-changed-files --input files=$'src/lib.rs\nsrc/main.rs'
```

### Run an issue-target workflow

```sh
rupu workflow run issue-to-spec-and-plan github:your-org/your-repo/issues/42
```

### Run an issue-target workflow that also needs inputs

```sh
rupu workflow run phase-delivery-cycle github:your-org/your-repo/issues/42 --input phase=phase-1
```

Important:

- `rupu issues run` is a convenience wrapper for issue-target workflows
- it does not expose extra `--input` flags
- when you need both an issue target and additional inputs, use `rupu workflow run`

### Re-attach to a run

```sh
rupu watch run_01J...
```

Replay a finished run:

```sh
rupu watch run_01J... --replay --pace=20
```

### Inspect workflow run history

```sh
rupu workflow runs
rupu workflow show-run run_01J...
```

### Approve or reject a paused workflow

```sh
rupu workflow approve run_01J...
rupu workflow reject run_01J... --reason "not ready"
```

### Browse issues

```sh
rupu issues list --repo github:your-org/your-repo
rupu issues show github:your-org/your-repo/issues/42
```

### Inspect and run autoflows

```sh
rupu autoflow list
rupu autoflow list --repo github:your-org/your-repo
rupu autoflow show issue-supervisor-dispatch
rupu autoflow show issue-supervisor-dispatch --repo github:your-org/your-repo
rupu autoflow run issue-supervisor-dispatch github:your-org/your-repo/issues/42
rupu autoflow tick
rupu autoflow status
rupu autoflow status --repo github:your-org/your-repo
rupu autoflow claims
rupu autoflow claims --repo github:your-org/your-repo
```

`rupu autoflow list`, `show`, `status`, and `claims` inspect tracked repos, not just the current working directory. Use `rupu repos attach` first if you want to inspect autoflows from outside a checkout, and pass `--repo` when you want to narrow output to one tracked repo.

---

## Targets and execution context

`rupu run` and `rupu workflow run` accept an optional target positional.

Common forms:

- `github:owner/repo`
- `github:owner/repo#42`
- `github:owner/repo/issues/123`
- `gitlab:group/project`
- `gitlab:group/project!7`
- `gitlab:group/project/issues/9`

Behavior:

- repo and PR / MR targets clone to a temp workspace for the run
- issue targets do not clone; the workflow runs in the current checkout and receives `{{ issue.* }}` metadata
- autoflow issue cycles prefer persistent worktrees under `~/.rupu/autoflows/worktrees/`

Practical implication:

- if an issue-target workflow needs to read or modify repo files, run it from the correct local checkout

---

## Recommended usage patterns

### Local coding assistant mode

Use checked-in project agents such as:

- `fix-bug`
- `add-tests`
- `review-diff`
- `scaffold`

Examples:

```sh
rupu run fix-bug "cargo test parser::tests::rejects_bad_token fails"
rupu run add-tests "cover parser::parse_config edge cases"
rupu run scaffold "add an IssueSummary struct in crates/rupu-scm/src/types.rs"
```

### Workflow mode

Use workflows when work crosses boundaries:

- multiple specialists
- approvals
- phase planning
- issue intake
- PR review loops

Examples are in [development-flows.md](development-flows.md) and [examples/README.md](../examples/README.md).

### Autoflow mode

Use autoflows when the same workflow should keep owning an issue over time.

Typical pattern:

- attach the repo once with `rupu repos attach`
- declare `autoflow:` and `contracts:` in the workflow YAML
- store schemas under `.rupu/contracts/`
- run `rupu autoflow tick` from a scheduler
- set `[autoflow].cleanup_after` if you want completed claims and their worktrees pruned automatically

Operational cleanup:

- `rupu autoflow release <issue-ref>` now removes the persisted claim and its managed worktree immediately
- terminal `complete` and `released` claims are pruned on later `rupu autoflow tick` runs once `cleanup_after` has elapsed

Examples:

```sh
rupu autoflow show issue-supervisor-dispatch
rupu autoflow show issue-supervisor-dispatch --repo github:your-org/your-repo
rupu autoflow run issue-supervisor-dispatch github:your-org/your-repo/issues/42
rupu autoflow tick
```

Operational visibility:

- `rupu autoflow status` shows contested issues when more than one autoflow matches the same issue
- `rupu autoflow claims` shows the selected workflow priority and the losing contenders, for example `*issue-supervisor-dispatch[100], phase-ready-autoflow[50]`

Two useful shapes:

- controller autoflow: selects the next workflow and emits `dispatch`
- direct autoflow: owns one issue phase directly and returns `await_human` or `complete`

See [workflow-format.md](workflow-format.md) for the `autoflow:` and `contracts:` schema and [examples/README.md](../examples/README.md) for copyable controller and direct examples.

---

## Working with SCM and issues

### Review a PR

```sh
rupu run scm-pr-review github:your-org/your-repo#42
```

### Use the unified SCM / issue tool surface inside your own agents

Add tools such as:

```yaml
tools:
  - scm.prs.get
  - scm.prs.diff
  - scm.prs.create
  - issues.get
  - issues.comment
```

Or allow whole namespaces:

```yaml
tools: [scm.*, issues.*]
```

Keep reviewer agents read-only even if their `tools:` list names writable SCM tools; `permissionMode: readonly` will block writes.

---

## Triggers and long-running automation

### Cron-driven workflows

List what would be eligible:

```sh
rupu cron list
```

Tick cron and event pollers:

```sh
rupu cron tick
```

Typical crontab split:

```text
* * * * *   rupu cron tick --skip-events
*/5 * * * * rupu cron tick --only-events
```

### Webhook-driven workflows

```sh
RUPU_GITHUB_WEBHOOK_SECRET=... rupu webhook serve --addr 0.0.0.0:8080
```

Use webhook mode when you want lower latency or event types that polling does not expose.

---

## MCP mode

Expose `rupu`'s SCM / issue tool surface to another MCP client:

```sh
rupu mcp serve --transport stdio
```

Use this when you want Claude Desktop, Cursor, or another MCP host to operate on the same unified repo / issue abstraction.

---

## Practical safety rules

- treat checked-in agents like code
- keep reviewer agents read-only
- keep implementers narrow and scoped
- use `ask` mode when editing locally
- add approval gates before risky side effects
- prefer one phase per PR for larger efforts

---

## A good adoption path for a team

1. check in a few narrow project agents
2. use them manually with `rupu run`
3. promote repeated multi-step work into workflows
4. add issue-target workflows
5. add panel review and approval gates
6. attach the repo and add autoflows only after manual workflows are solid
7. add cron or webhook wakeups only after the autonomous loop is stable

That path keeps the system observable and avoids automating a weak process.
