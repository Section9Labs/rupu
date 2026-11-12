# Agentiflow tutorial

This walks one agentiflow from an empty machine to reading the results: install
the fleet, write a lead and two pool agents, write the definition, run it, watch
it, steer it, and read the findings. Then it does the same with the **stock
network-assessment fleet**, which ships a ready-made lead.

For the reference behind each step, see [agentiflows.md](agentiflows.md); for the
authoring patterns (goal recipes, budget tuning, pool design), see
[agentiflow-authoring.md](agentiflow-authoring.md).

All the targets here are invented. Point the fleet only at things you are
authorized to act on.

---

## Part 1: a code-review sweep from scratch

The goal: review a small service for injection bugs, have a second agent confirm
each one, and stop once two are confirmed (or the budget runs out).

### 1. Install the fleet (optional for this part)

`rupu fleet install` lays down the stock security-assessment agents and
workflows into `~/.rupu`:

```bash
rupu fleet install
# add --project to install into the current project's .rupu/ instead
# add --force to overwrite files you have edited
```

The stock fleet ships a `code-auditor` agent (and the generic
`assessment-lead`, used in Part 2), but no code-review *lead* and no agentiflow
definition, so this part writes its own. Files that already exist are kept unless you pass `--force`.

### 2. Write the pool agents

An agentiflow needs a lead plus the specialists it dispatches. Put these in the
project you want to review, under `.rupu/agents/`.

A reviewer that files findings (`.rupu/agents/file-reviewer.md`):

```markdown
---
name: file-reviewer
description: Reviews a set of files for real injection bugs and records each as a finding.
permissionMode: bypass
maxTurns: 60
findingsProfile: full
tools: [read_file, grep, glob, findings.report]
---
You review the files you are given for real, exploitable injection bugs
(SQL, command, template, path). For each genuine issue, call `findings.report`
with a clear title, the `path` (e.g. `src/db.py:42`), a stated `root_cause`,
and a `classification` (a CWE id such as CWE-89 for SQL injection). Pin every
finding to a file and line. No style nits, no guesses: a real bug, or nothing.
```

A verifier that confirms them (`.rupu/agents/finding-verifier.md`):

```markdown
---
name: finding-verifier
description: Re-checks a finding another run filed and records an independent verdict.
permissionMode: bypass
maxTurns: 60
findingsProfile: full
tools: [read_file, grep, glob, findings.verify]
---
You are handed a finding another agent filed. Open the cited file and line,
establish for yourself whether it is a real, exploitable issue, and record a
verdict with `findings.verify`: `confirmed` only if you reproduced or
independently established it, `disputed` if it is wrong, `inconclusive` if you
could not decide. You are the quality gate between "flagged" and "confirmed".
```

The lead (`.rupu/agents/review-lead.md`). Its `tools:` is exactly what it gets
on top of the fleet tools, so give it just the read tools it needs to orient:

```markdown
---
name: review-lead
description: Lead of a goal-directed code-review fleet.
permissionMode: bypass
maxTurns: 120
tools: [read_file, grep, glob]
---
You are the LEAD of a code-review fleet. Drive the goals below to completion:
find injection bugs and have them independently confirmed. You orchestrate; you
do not review the whole codebase yourself.

You hold `dispatch` (start a pool agent on a focused task, get a handle back),
`join` (collect a dispatched agent's result), the board and mailbox tools, and
the status tools `goal.status` / `budget.status`. Start by calling
`agents.list` so you know your pool.

Work in rounds:

1. **Round 1 — map and fan out.** Spend a few quick `grep` / `glob` / `read_file`
   calls to see the service's modules. Then dispatch `file-reviewer` at two or
   three tightly-scoped sets of files in parallel, each with a specific worklist.
   `join` them and read what they found.
2. **Following rounds — verify.** For each candidate finding, dispatch
   `finding-verifier` to confirm it independently. `join` and read the verdicts.
   Check `goal.status`: the goal counts only confirmed findings.
3. **Keep going** until the goal is met and the budget is healthy
   (`budget.status`), then finish. Record more than the floor if you find more.

Finish each round with a short summary of what you did.
```

Validate the agent files before you run:

```bash
rupu agent validate --all
```

### 3. Write the definition

Put it at `.rupu/agentiflows/review-sweep.yaml`:

```yaml
name: review-sweep
description: Review the billing service for injection bugs and confirm what is found.
lead: review-lead
engagement_profiles: [code]

goals:
  - id: injection
    objective: "Find and confirm two injection issues (CWE-89 or CWE-78)."
    target:
      findings: { classification: "CWE-89" }
      count_gte: 2
      verified: true
    verify_with: finding-verifier
    required: true

budget:
  usd: 15
  tokens: 4000000
  wall_clock: "2h"
  soft_at: 0.8

scope:
  authorized: true
  roots:
    - { kind: "code:repo", path: "." }

pool:
  agents: [review-lead, file-reviewer, finding-verifier]

round:
  lead_max_turns: 40
  ceiling: { rounds: 20, wall_clock: "3h" }
```

Notes on this definition:

- `engagement_profiles: [code]` types the evidence. The `code` profile defines
  `code:repo` and `code:file` assets and a `unreviewed → reviewed` depth ladder.
- The goal is **verified**: a finding counts only after `finding-verifier`
  (a different run) records a `confirmed` verdict. `verify_with` must name a
  pool agent, and the reviewers use `findingsProfile: full` so their findings
  can be verified.
- `budget.usd` is enforced only if the lead's model is priced; `tokens` and
  `wall_clock` always apply. See [budget tuning](agentiflow-authoring.md#tuning-budgets).
- The `ceiling` is a hard backstop in case the goal is never met.

There is no CLI dry run: `rupu agentiflow run` validates the definition as it
launches, and a definition that fails validation is refused before any model
call. To check a definition without launching anything, use the control plane
or MCP with `cp serve` running: `POST /api/agentiflow-defs/validate` with body
`{"raw": "<the file's contents>"}` (plus `scope_id` to resolve a project's own
profiles) answers `{ok: true, name, lead, engagement_profiles}` or a 400 naming
the first problem; the MCP tool is `agentiflow_defs.validate`.

```bash
python3 -c 'import json,sys; print(json.dumps({"raw": sys.stdin.read()}))' \
  < .rupu/agentiflows/review-sweep.yaml \
  | curl -s -X POST -H "Authorization: Bearer $RUPU_CP_TOKEN" \
      -H 'content-type: application/json' --data-binary @- \
      http://127.0.0.1:7878/api/agentiflow-defs/validate
```

### 4. Run it

From the root of the service you want to review (that directory is the run's
workspace):

```bash
rupu agentiflow run review-sweep --detach
# agentiflow review-sweep: run af_01J2ABC... (detached)
```

Drop `--detach` to run it in the foreground and see the outcome printed at the
end. Detached, it runs in its own process group; follow it next.

### 5. Watch it

```bash
rupu agentiflow attach af_01J2ABC...
```

`attach` prints the event log (each round as it lands) and the lead's transcript
for the round in progress, following it until the run ends. Ctrl-C detaches; the
run keeps going. For a one-shot snapshot instead of following, use
`rupu agentiflow attach af_01J2ABC... --no-follow`.

For a structured view:

```bash
rupu agentiflow status af_01J2ABC...
# status, rounds, each goal's current/target, budget state and spend
rupu agentiflow list
# every run, newest first
```

In the control plane (`rupu cp serve`), open **Runs → Agentiflows** and click
the run. The **Flow** map draws the lead's rounds as a spine with each round's
reviewers and verifiers branching off it; the **Engagement** tab shows the goal
progress bars; the **Token flow** strip shows per-call spend by round.

### 6. Steer it

Send the lead an operator instruction mid-run:

```bash
rupu agentiflow send af_01J2ABC... "Skip the vendored code under third_party/."
```

The lead gets it before its next model call, and the next round recaps it. Add
`--now` to cut the current round short so the lead turns to it at once. On the
web, the **Messages** tab has a composer that does the same.

### 7. Read the results

When the goal is met (two confirmed findings), the envelope stops the run as
`goals_met`. Read what it found:

```bash
rupu findings list run:af_01J2ABC...
# the findings the lead and its units recorded under this run
rupu findings export --run af_01J2ABC... --to md -o review.md
```

Or open the run's **Findings** tab in the control plane. The raw record is on
disk: `~/.rupu/agentiflows/af_01J2ABC.../events.jsonl` has the per-round
`round` events (goals met, spend, the lead's decision), and the pooled evidence
is under the workspace's `.rupu/coverage/`.

To end a run early:

```bash
rupu agentiflow stop af_01J2ABC...        # graceful, at the next round boundary
rupu agentiflow stop af_01J2ABC... --now  # hard, signal everything now
```

---

## Part 2: a network assessment with the stock fleet

The stock fleet ships `assessment-lead`, a generic lead, plus `recon`,
`service-analyst` and `exploit-verifier` and a `network-assessment` workflow, so
you only write the definition. Reuse them for an authorized network engagement.

### 1. Confirm the fleet is installed

```bash
rupu fleet install
rupu agent list | grep -E 'assessment-lead|recon|service-analyst|exploit-verifier'
```

### 2. Write the definition

`~/.rupu/agentiflows/net-sweep.yaml` (global, so it runs from any directory):

```yaml
name: net-sweep
description: Enumerate and test the lab subnet, confirming the real exposures.
lead: assessment-lead
engagement_profiles: [network]

goals:
  - id: tested-hosts
    objective: "Test the two authorized hosts to the `tested` depth."
    target:
      asset: { kind: "network:host" }
      depth_at_least: tested
    count_gte: 2
    required: true
  - id: confirmed
    objective: "Confirm at least one real exposure."
    target:
      findings: {}
      count_gte: 1
      verified: true
    verify_with: exploit-verifier
    required: true

coverage:
  reach: 0.8          # 80% of discovered services...
  depth: tested       # ...tested or deeper
  kinds: [network:service]

budget:
  usd: 40
  wall_clock: "4h"
  soft_at: 0.75

scope:
  authorized: true
  mode: active
  roots:
    - { kind: "network:host", host: "10.20.0.11" }
    - { kind: "network:host", host: "10.20.0.12" }

pool:
  agents: [assessment-lead, recon, service-analyst, exploit-verifier]
  workflows: [network-assessment]

round:
  lead_max_turns: 60
  ceiling: { rounds: 30, wall_clock: "6h" }
```

Notes:

- `engagement_profiles: [network]` defines `network:host` and `network:service`
  assets and a `discovered → enumerated → tested → exploited` depth ladder. The
  goals' kinds and depths are validated against it.
- The first goal is an **asset goal**: at least two `network:host` assets at
  `tested` depth or deeper. Omitting the `locator` matches any host; the agents
  record depth with `assets.mark`.
- The second goal is a **verified findings goal**, confirmed by
  `exploit-verifier` (a pool agent).
- `coverage` adds an engagement-wide stop: 80% of discovered services tested.
  Either the goals or the coverage stop can end the run.
- `pool.workflows: [network-assessment]` lets the lead dispatch the stock
  recon → analyze → verify workflow as a unit (`dispatch { kind: "workflow",
  name: "network-assessment" }`), instead of, or alongside, dispatching the
  agents one by one.
- The `scope.roots` are the authorized hosts, shown to the lead on round 0.

### 3. Price the lead's model (so `budget.usd` is enforced)

`budget.usd` is enforced only when the lead's model has a price. The stock
`assessment-lead` uses `*` tools and whatever provider your config defaults to;
add a price if the built-in table doesn't cover it:

```toml
# ~/.rupu/config.toml
[pricing.anthropic."claude-sonnet-4-6"]
input_per_mtok = 3.0
output_per_mtok = 15.0
```

See [configuration.md](configuration.md#pricing). Without a price, the `usd` cap
is dropped at launch (with a warning) and `tokens` / `wall_clock` still apply.

### 4. Run, watch, and read

```bash
rupu agentiflow run net-sweep --detach
rupu agentiflow attach <id>
rupu agentiflow status <id>
```

The lead will call `agents.list` and `workflows.list`, dispatch `recon` to
enumerate the authorized hosts (recording each with `assets.mark`), hand the
enumerated services to `service-analyst`, and verify the claimed exposures with
`exploit-verifier`. Watch the **Assets** tab fill in and the coverage fraction
climb. When both goals are met (two hosts tested, one exposure confirmed), or
80% of services are tested, the run stops.

```bash
rupu findings list run:<id>
rupu findings export --run <id> --to html -o net-sweep.html
```

---

## Where to go next

- [agentiflow-authoring.md](agentiflow-authoring.md): goal-target recipes,
  budget tuning, verifier setup, steering, and a pool-design checklist.
- [agentiflows.md](agentiflows.md): the full definition reference, the round
  loop, and the control-plane and MCP surfaces.
- The repo ships two example definitions to read:
  `.rupu/agentiflows/rupu-self-review.yaml` and
  `.rupu/agentiflows/dispatch-kinds-flow.yaml`.
