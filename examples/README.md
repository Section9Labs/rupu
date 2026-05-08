# Example agents and workflows

These files are fuller, project-oriented examples for teams that want more than the small starter set installed by `rupu init --with-samples`.

They are meant to be copied into a repo's `.rupu/` directory and adapted.

---

## Copy into a project

```sh
mkdir -p .rupu/agents .rupu/contracts .rupu/workflows
cp examples/agents/*.md .rupu/agents/
cp examples/contracts/*.json .rupu/contracts/
cp examples/workflows/*.yaml .rupu/workflows/
```

Then edit names, providers, models, and tool lists to match your repo and operating standards.

---

## Included agents

| Agent | Purpose |
| --- | --- |
| `repo-investigator` | Diagnose an issue or planned phase without editing |
| `repo-implementer` | Make the minimal scoped code change and validate it |
| `code-reviewer` | Produce concise textual findings for a file, diff, or PR |
| `issue-understander` | Turn an issue into a precise technical problem statement |
| `spec-writer` | Write a spec document into the repo |
| `phase-planner` | Turn a spec into reviewable phases |
| `pr-author` | Open a draft PR from the current branch |
| `issue-commenter` | Post one focused issue comment |
| `writer` | Summarize or aggregate text without side effects |
| `security-reviewer` | Structured security panel reviewer |
| `performance-reviewer` | Structured performance panel reviewer |
| `maintainability-reviewer` | Structured maintainability panel reviewer |
| `finding-fixer` | Address panel findings and emit a revised review subject |

---

## Included workflows

| Workflow | Purpose |
| --- | --- |
| `quick-bugfix` | Simple investigate → implement flow |
| `review-changed-files` | `for_each:` fan-out over a file list |
| `code-review-panel` | Standalone specialist review panel over one diff or subject |
| `issue-to-spec-and-plan` | Turn an issue into a spec and phased plan |
| `phase-delivery-cycle` | Implement one phase, open a PR, run a panel, and pause for approval |
| `issue-supervisor-dispatch` | Controller autoflow that selects the next workflow for an issue |
| `phase-ready-autoflow` | Direct autonomous phase owner without a separate controller |

---

## Included contracts

| Contract | Purpose |
| --- | --- |
| `autoflow_outcome_v1` | Persistent autonomous handoff result |
| `workflow_dispatch_v1` | Child-workflow dispatch payload |
| `phase_plan_v1` | Stable phased implementation plan artifact |
| `review_packet_v1` | Structured PR validation and review summary |

---

## Required setup

### LLM auth

Authenticate at least one provider used by the example agents:

```sh
rupu auth login --provider anthropic --mode sso
```

### SCM / issue defaults

If you will use the PR / issue workflows, configure defaults in `~/.rupu/config.toml` or `<repo>/.rupu/config.toml`:

```toml
[scm.default]
platform = "github"
owner = "your-org"
repo = "your-repo"

[issues.default]
tracker = "github"
project = "your-org/your-repo"
```

### Run from the repo checkout for issue-target workflows

`issue-to-spec-and-plan` and `phase-delivery-cycle` expect to read and write files in the local repository. Run them from the correct checkout.

### Attach the repo for autoflows

```sh
rupu repos attach github:your-org/your-repo .
```

Use this before `rupu autoflow tick` so autonomous issue ownership can resolve the correct local checkout.

---

## Example commands

### Simple bugfix

```sh
rupu workflow run quick-bugfix --input prompt="cargo test parser::tests::rejects_bad_token fails"
```

### File-by-file review

```sh
rupu workflow run review-changed-files --input files=$'src/lib.rs\nsrc/main.rs'
```

### Review panel over a diff

```sh
rupu workflow run code-review-panel --input diff="$(git diff HEAD)"
```

### Issue to spec and phase plan

```sh
rupu workflow run issue-to-spec-and-plan github:your-org/your-repo/issues/42
```

### One planned delivery phase

```sh
rupu workflow run phase-delivery-cycle github:your-org/your-repo/issues/42 --input phase=phase-1
```

### Controller autoflow for issue ownership

```sh
rupu autoflow run issue-supervisor-dispatch github:your-org/your-repo/issues/42
```

### Inspect and reconcile all autoflows

```sh
rupu autoflow list
rupu autoflow tick
```

---

## Notes

- `rupu issues run` is convenient for issue-target workflows with no extra inputs.
- `phase-delivery-cycle` needs `--input phase=...`, so run it with `rupu workflow run`, not `rupu issues run`.
- These examples assume a disciplined model: one phase per PR, automated review panel per PR, human merge between phases.
- `issue-supervisor-dispatch` is the controller example. `phase-ready-autoflow` is the direct-owner example.
- If more than one autoflow can match the same issue, set `autoflow.priority` intentionally so ownership is deterministic.
