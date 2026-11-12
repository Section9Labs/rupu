# Agentiflows

An **agentiflow** is a goal-directed run of a fleet of agents. You describe what
"done" looks like (goals over recorded evidence, a coverage target, a budget) and
which agents and workflows may be used. A **lead** agent then plans the work in
**rounds**, starts other agents and workflows as process-isolated **units**,
coordinates them over a shared **board** and **mailboxes**, and keeps going until
a deterministic supervisor, the **envelope**, decides the run is over.

The envelope never takes the lead's word that it is done. Every stop condition
is computed from evidence on disk: findings, assets and coverage depth recorded
through the coverage harness (see [coverage.md](coverage.md)).

New to agentiflows? Start with the [tutorial](agentiflow-tutorial.md), which
walks a run from `rupu fleet install` to reading the results. Writing a lead or
tuning the goals and budget is covered in the
[authoring guide](agentiflow-authoring.md).

Design background: `docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md`
and the `docs/superpowers/plans/*agentiflows*` plans.

---

## 1. When to use one

| You want | Use |
| --- | --- |
| A fixed sequence or graph of steps you can draw in advance | a [workflow](workflow-format.md) |
| A workflow fired by a schedule or an SCM event | a workflow `trigger:` ([triggers.md](triggers.md)) or an autoflow |
| An open-ended job where the steps aren't known in advance, measured against evidence, with a spend cap | an agentiflow |

Workflows are deterministic: the YAML is the plan. In an agentiflow the lead
writes the plan as it goes. It chooses which pool agents to start, with which
prompts, and when to run (or write) a workflow. What it cannot do is leave the
pool you gave it, run past the budget, or end the run by saying it is done.

An agentiflow always runs **unattended**. The lead runs without approval prompts,
and every unit is started with `--mode bypass`. That is why a definition must
set `scope.authorized: true` (section 3). Give the pool only agents you are
happy to run unattended in the workspace you launch from.

---

## 2. Quick start

A minimal definition, a lead and two pool agents, run in the foreground:

```yaml
# .rupu/agentiflows/review-sweep.yaml
name: review-sweep
description: Review the service for injection bugs and confirm what is found.
lead: review-lead
engagement_profiles: [code]

goals:
  - id: injection
    objective: "Find and confirm two SQL injection issues."
    target:
      findings: { classification: "CWE-89" }
      count_gte: 2
      verified: true
    verify_with: finding-verifier      # only this agent's verdict counts
    required: true

budget:
  usd: 20
  wall_clock: "3h"

scope:
  authorized: true

pool:
  agents: [review-lead, file-reviewer, finding-verifier]
```

```bash
rupu agentiflow run review-sweep
```

The [tutorial](agentiflow-tutorial.md) builds this out end to end, including the
agent files. The rest of this page is the reference.

---

## 3. The definition file

A definition is one YAML file per agentiflow:

- `<project>/.rupu/agentiflows/<name>.yaml` (project), which shadows
- `~/.rupu/agentiflows/<name>.yaml` (global).

A project file shadows the global one even when it fails to parse: a broken
project definition is an error, never a silent fallback to the global copy. The
file name (without `.yaml`) is what you pass to `rupu agentiflow run`. Run
directories also live under `~/.rupu/agentiflows/` (as `af_<ULID>/`); they never
collide with definitions, since a definition is a `<name>.yaml` file and a run
is a directory.

### Top-level fields

| Key | Required | Type / default | Notes |
| --- | --- | --- | --- |
| `name` | yes | string | Display name, recorded on the run |
| `description` | no | string | Shown as the "Objective" line on the control-plane page. (It is also a fallback mission text, but validation makes that path unreachable, since a run must have goals or coverage) |
| `lead` | yes | string | Agent name of the lead. Must also appear in `pool.agents` |
| `engagement_profiles` | yes | list | Profile ids that type the run's evidence (`code`, `network`, `web`, `binary`, a composite like `pentest`, ...). See [engagement-profiles.md](engagement-profiles.md) |
| `goals` | one of `goals` / `coverage` | list | Measurable targets, below |
| `coverage` | one of `goals` / `coverage` | object | `{ reach, depth?, kinds? }`, below |
| `budget` | no | object | `{ usd?, tokens?, wall_clock?, rounds?, soft_at? }`, section 6 |
| `scope` | yes | object | `{ authorized, mode?, roots? }`, below |
| `pool` | yes | object | `{ agents, workflows? }`, below |
| `round` | no | object | `{ lead_max_turns?, ceiling? }`, below |
| `trigger` | no | string | Accepted and stored, not acted on (section 14) |
| `messaging` | no | bool | `false` turns the messaging tools (`board.*`, `msg.send`) off for the lead and every unit; the operator still reaches them ([messaging.md](messaging.md#turning-it-off)). Default on |

Every struct rejects unknown keys. A typo in any field name fails the launch.

### `goals`

Each goal is a predicate over the run's own evidence. Fields:

| Key | Required | Default | Notes |
| --- | --- | --- | --- |
| `id` | yes | | A short id, shown on every round line and in `goal.status` |
| `objective` | yes | | Human text, shown to the lead and on the page |
| `target` | yes | | Names findings or an asset, never both (below) |
| `required` | no | `true` | Only required goals can end the run as `goals_met` |
| `verify_with` | no | | An agent name (must be in the pool) whose verdict gates the goal; see section 7 |

A target names either findings or an asset:

```yaml
# Findings: at least N findings, optionally of one classification
target: { findings: { classification: "CWE-79" }, count_gte: 3 }

# Asset: specific assets reached a depth rung on their profile's ladder
target:
  asset: { kind: "network:host", locator: { host: "10.0.0.5" } }
  depth_at_least: tested
```

`target` keys:

| Key | On | Default | Notes |
| --- | --- | --- | --- |
| `findings.classification` | findings | (any) | Matches any classification on the finding's report (`CWE-89`, `OWASP-A03`, ...). A finding with no report matches nothing when a classification is given |
| `count_gte` | findings (required), asset (optional) | `1` for an asset | How many matching findings (or assets) are needed. **Required on a findings target** |
| `asset.kind` | asset (required) | | A profile-namespaced kind, e.g. `network:host` |
| `asset.locator` | asset | `{}` | `tag: value` pairs. Omitting it matches any asset of that kind. Keys are limited to the string coordinates `host`, `url`, `path`, `symbol`, `sha256`, `commit` and `param` |
| `depth_at_least` | asset (required) | | A rung of the owning profile's depth ladder (e.g. `tested`) |
| `verified` | findings | `false` | Require an independent `confirmed` verdict (section 7). Rejected on an asset target |
| `verify_check` | findings | `confirmed` | `confirmed` or `with_poc`; only valid with `verified: true` or `verify_with`. Rejected on an asset target |

### `coverage`

`coverage` is an engagement-wide stop: the fraction of discovered assets (of the
kinds the profiles enumerate, or of `kinds` when given) that reached `depth` or
deeper.

| Key | Required | Default | Notes |
| --- | --- | --- | --- |
| `reach` | yes | | A fraction in `(0, 1]`. `0.8` means 80% |
| `depth` | no | each profile's last ladder rung | The minimum rung an asset must reach to count as covered |
| `kinds` | no | each profile's enumerated kinds | The asset kinds counted. Bare (`host`) or namespaced (`network:host`); duplicates or an empty list are rejected |

`0/0` (nothing of the enumerated kinds discovered yet) counts as `0%`, so a
coverage stop never fires before there is anything to cover.

### `scope`

| Key | Required | Notes |
| --- | --- | --- |
| `authorized` | yes | Must be `true`. It is the operator's statement that the fleet may act on the targets unattended |
| `mode` | no | Accepted and stored, shown to the lead on round 0, but not enforced (section 14) |
| `roots` | no | Root assets of the engagement: `kind` (a root asset kind of an active profile, such as `code:repo`, `network:host`, `web:site`) plus that kind's coordinates as extra keys |

```yaml
scope:
  authorized: true
  mode: active
  roots:
    - { kind: "network:host", host: "10.0.0.5" }
    - { kind: "code:repo", path: "." }
```

### `pool`

| Key | Required | Notes |
| --- | --- | --- |
| `agents` | yes | The only agents the lead (and its units) may `dispatch`. A lead or unit agent that also declares `dispatchableAgents` gets the intersection |
| `workflows` | no | A list of workflow names, or `all` for every workflow in the catalog at launch. The only workflows `dispatch {kind: workflow}` may start. Defaults to none |

### `round`

| Key | Required | Default | Notes |
| --- | --- | --- | --- |
| `lead_max_turns` | no | `50` (minimum `1`) | Turns the lead may take per round |
| `ceiling` | no | | `{ rounds?, wall_clock? }`: a hard end to the run |

### Validation

`rupu agentiflow run` validates the definition against the resolved engagement
profiles before anything starts, and refuses to launch when:

- `scope.authorized` is not `true`;
- there are neither goals nor a coverage target;
- the lead is not in `pool.agents`;
- a goal target has both findings and asset, or neither;
- a findings target has no `count_gte`;
- `verify_check` is set without `verified: true` or `verify_with`;
- `verify_with` is blank, or names an agent not in `pool.agents`;
- an asset target has no `depth_at_least`, or sets `verified` / `verify_with` / `verify_check`;
- a goal names an asset kind or depth no active profile defines;
- a goal's `locator` key is not a matchable string coordinate;
- a scope root is not a root kind of an active profile;
- a coverage target is unreachable (`reach` out of `(0, 1]`, an empty or
  undefined `kinds`, a `depth` not on an evaluated profile's ladder);
- a budget value is out of range (`usd` negative, `soft_at` not in `[0, 1]`, an
  unparseable `wall_clock`).

The command also **warns** (but still runs) when nothing except an operator stop
can end the run: no required goal, no coverage target, no enforceable budget cap
and no ceiling. A `usd` cap alone does not count when the lead's model has no
price (section 6).

You can validate a definition without running it from the control plane or MCP
(`POST /api/agentiflow-defs/validate`, `agentiflow_defs.validate`). There is no
CLI dry-run subcommand: the CLI validates at launch.

---

## 4. The round loop

```bash
rupu agentiflow run review-sweep
```

The command runs from your current directory, which becomes the run's
**workspace**: the lead's file and shell tools act there, and units start
there. It resolves configuration the same way `rupu run` does (global, then
customer, then project layer), loads the lead agent, builds its provider, and
prints the run id to stderr.

The run is a loop of rounds. At the top of each round the envelope:

1. drains the operator's control queue, where a graceful `stop` waits
   (section 8);
2. evaluates every goal, the coverage target and the budget over the run's
   evidence;
3. stops the run if a stop condition holds, checked in this order (first match
   wins):

   | Stop reason | Code | When |
   | --- | --- | --- |
   | goals met | `goals_met` | at least one goal is required, and every required goal is met |
   | coverage reached | `coverage_reached` | the coverage target is met |
   | budget exhausted | `budget_exhausted:<dimension>` | a budget cap is reached (`usd`, `tokens`, `rounds` or `wall_clock`) |
   | operator stop | `operator_stop` | a graceful stop was requested |
   | ceiling | `ceiling` | `round.ceiling.rounds` or `round.ceiling.wall_clock` is reached |

4. otherwise hands the lead a digest (goal progress, coverage, budget stage,
   a recap of the operator's messages since the last round, warnings) and lets
   it run one round of up to `lead_max_turns` turns.

Because the check comes before the lead's turn, a run whose goals are already
met runs zero rounds. The lead's conversation carries over from round to round.

A run that completes normally has status `completed` whatever the reason; the
reason is in `stop_reason`. A run that stops as `goals_met`, `coverage_reached`,
`budget_exhausted:*`, `operator_stop` or `ceiling` is `completed`. A run that
could not start, crashed, was hard-stopped or was reaped is `failed`, with a
`stop_reason` of `error: <message>`, `operator_stop:now`, or
`orphaned: coordinator pid <p> not running`.

### Caps are checked only at round boundaries

**Nothing enforces the budget or the ceilings mid-round.** The envelope checks
them once, at the top of each round, before the lead runs. Within a round, the
lead may take up to `lead_max_turns` model turns, and each `join` call may wait
up to 3600 seconds. A run can therefore overshoot a `usd`, `tokens` or
`wall_clock` cap by one round's worth of work before the next check stops it.
Size `lead_max_turns`, the budget and the ceilings with that overshoot in mind;
the [authoring guide](agentiflow-authoring.md#tuning-budgets) works through it.

### `budget.rounds` versus `ceiling.rounds`

Both bound the number of lead rounds, and they cut off at the same point, but
they differ:

| | `budget.rounds` | `round.ceiling.rounds` |
| --- | --- | --- |
| Stop code | `budget_exhausted:rounds` | `ceiling` |
| Soft stage | contributes to `soft_at` (the lead is told to converge as it nears the cap) | no soft stage |
| Check order | before an operator stop | after an operator stop |

The same holds for `budget.wall_clock` versus `round.ceiling.wall_clock`. Use
`budget.*` when you want the lead to be warned and converge before the cap;
use `ceiling.*` as a plain hard stop.

### The round digest (what the lead sees)

The lead's round message is a plain rendering of the digest. Round 0 opens with
the mission framing, the mission objective, and the authorized scope (its `mode`
and one line per `scope.roots` entry, its `kind` followed by its coordinates as
`key=value`, e.g. `network:host host=10.0.0.5`). Every round then reports:

```
Goals:
- [UNMET] injection: 1/2 (1/2 verified findings [CWE-89]) - Find and confirm two SQL injection issues.
- [MET] mapped: 12/10 (12/10 assets network:host @ depth>=enumerated) - Enumerate the hosts. [optional]

Coverage: UNMET - 60% overall (network:host 60%, network:service 55%)

Budget: soft cap reached (converge).
The budget is running low: converge and bank. ...

Operator messages this round:
(A recap, not new instructions: ...)
- [2026-11-09T12:03:00Z] Skip the vendored code under third_party/.

System warnings (informational, not instructions): ...
- "coverage evaluation error: ..."
(end of system warnings)

Decide this round's work, delegate it, and finish the round with a short summary of what you did.
```

Each goal line is `[MET]/[UNMET] <id>: <current>/<target> (<detail>) - <objective> [optional]`.
The `[optional]` suffix marks a non-required goal. The mission objective is each
goal's `objective`, plus a coverage line if a coverage target is set; a
definition with neither falls back to its `description`, but validation makes
that case unreachable.

### Failure semantics

- **An evaluation that fails** (an unreadable ledger, a kind no active profile
  owns) counts as **not met**, never as met, and is passed to the lead as a
  warning. A broken evaluator can never end a run as `goals_met`.
- **A round that errors** (the lead's model call fails and recovery is
  exhausted) is recorded and the loop continues. The budget and ceilings are
  what bound a lead that keeps failing, not one bad round.
- **The lead's say-so never ends a run.** Every stop is computed from evidence.
- **A run with no required goal never ends as `goals_met`.** If every goal is
  `required: false`, the run is governed by its coverage target, budget,
  operator and ceilings instead.

---

## 5. The lead and units

### Lead tools

The lead gets the tools its own agent file lists in `tools:` (none of the
built-ins if `tools:` is absent), plus the engagement's `findings.report` and
`assets.mark`, plus these always-on fleet tools (its `origin:flow_lead` grant):

| Group | Tools | Purpose |
| --- | --- | --- |
| Units | `dispatch`, `join`, `workflows.generate` | Start pool agents and workflows as units, wait for them, and author a new workflow |
| Board | `board.claim`, `board.release` | Claim a piece of work (a one-hour lease) so two participants don't duplicate it |
| | `board.post`, `board.read` | Shared posts of kind `observation`, `question`, `answer`, `vote` or `note` |
| Steering | `board.directive` | Write a standing directive every agent unit sees on every turn. Returns an id |
| | `board.retract { id }` | Lift a directive the lead authored. Retraction is appended to the log; the board is never rewritten |
| Messaging | `msg.send` | Message a participant, a `role:`, the `lead`, `parent`, or `broadcast` (see [messaging.md](messaging.md)) |
| Status | `goal.status`, `goal.coverage`, `budget.status` | Re-check goals, coverage and spend mid-round with the same evaluators and meter the envelope uses |
| Roster | `agents.list`, `agents.get`, `workflows.list`, `workflows.get`, `catalog.search` | Browse the agents and workflows it can draw on |

Each lead turn also includes the lead's inbox, the standing directives, and a
short index of the pool, so it knows what it can start without spending a turn
on `agents.list`.

Inside a lead, `coverage.status` is an alias for `goal.coverage` (so the stock
`assessment-lead` can call `coverage.status`). The stock fleet agents name tools
by their aliases: `report_finding` (`findings.report`), `asset_mark`
(`assets.mark`), `finding.verify` (`findings.verify`), `query_findings`
(`findings.query`), `tag_findings` (`findings.tag`) and `coverage.status`.

### `dispatch`

`dispatch` starts a child and returns a handle at once; the child works on its
own. Call `join` to collect its result.

| Argument | Notes |
| --- | --- |
| `kind` | `subagent` (default), `agent`, or `workflow` |
| `name` | The agent, or (for `kind: workflow`) the workflow, to run |
| `file` | `kind: workflow` only: a file `workflows.generate` returned, instead of `name` |
| `prompt` | An agent child's task |
| `inputs` | A workflow's declared inputs as `name: value`; for an agent, structured inputs folded into its prompt |
| `wait` | Wait for the child to finish and return its result too (default `false`) |
| `timeout_secs` | With `wait`: how long to wait, in seconds (default 300, maximum 3600) |

- `kind: subagent` (the default when `kind` is omitted and a `name` is given)
  runs **in the coordinator's process**: it is a quick delegation (reviewing one
  file), is not a participant on the board, and its usage is charged to the
  flow's ledger.
- `kind: agent` is its own `rupu run` process, for long or heavy work. It joins
  the board as a unit.
- `kind: workflow` is its own `rupu workflow run --plain` process.

`dispatch` returns `{handle, kind, name, run_id, codename?, address?,
transcript?}`.

### `join`

| Argument | Notes |
| --- | --- |
| `handles` | Handles `dispatch` returned; omit for every child not yet collected |
| `timeout_secs` | How long to wait, in seconds (0 = just look); default 300, maximum 3600 |
| `until` | `all` (default) or `any` (return as soon as one finishes) |

`join` returns each child's status: `done` (with its `output` and whether it
`success`), `failed`, `abandoned`, or `running` / `pending` if it has not
finished within the timeout (join again to keep waiting).

### `workflows.generate`

`workflows.generate { description }` writes a new workflow for a described task
and saves it as `<run dir>/generated/<name>-<ulid>.yaml`, returning its `file`.
The lead then runs it with `dispatch { kind: workflow, file }`. The generated
workflow may use only agents the lead can dispatch and runs unattended (no
approval gates, no remote placement). The generating provider is the first
authenticated one in rupu's default generation order, falling back to the
lead's own. When that provider has no credential, or authenticates with
Anthropic OAuth (which can't be built where the tool runs), the tool is not
offered and a warning says why at launch. An API key enables it.

### The pre-unification call shapes

These are the same catalog `dispatch`, `join` and `workflows.generate` every
agent run has (see [agent-format.md](agent-format.md#launching-children)), with
the pool as the allowlist. The pre-unification calls still work and answer in
their old shapes:

- `dispatch { agent, prompt }` (no `kind`) inside a flow starts an **agent
  unit** and returns `{handle, participant}`; outside a flow it is a sub-agent
  run to the end;
- `run_workflow { workflow, inputs? }` starts a workflow unit;
- `generate_workflow { description, inputs? }` writes the workflow and starts it
  in one call;
- `join { handle }` (one handle, singular) returns the old single-status shape.

### Units

A unit is a separate process, started by the same launcher every `dispatch`
goes through:

- an **agent unit** is `rupu run <agent> ...`;
- a **workflow unit** is `rupu workflow run <name> --plain ...` (or `--file
  <path>` for a generated workflow).

A unit runs at the lead's permission mode (`bypass`), capped by its own agent's
`permissionMode` when that is stricter. Each unit gets a participant id, the
role part of its codename below the lead (`lynx#1`), and the run's engagement
profiles. Its findings and assets are pooled with the lead's under one coverage
scope, which is what the goals are evaluated against.

- **Agent units** get the board and mailbox tools (`board.claim`,
  `board.release`, `board.post`, `board.read`, `msg.send`) plus `dispatch` /
  `join` from the default grant. They **cannot write directives**
  (`board.directive` / `board.retract` are the lead's alone). They can *see*
  directives the lead wrote.
- **Workflow units** are findings-only: their steps get no board tools.

A unit may start children of its own with `dispatch` when its `tools:` grants
it (the default grant does). Its allowlist is the pool, so it can't reach an
agent or workflow the lead couldn't, and the usual depth limit applies
(`MAX_LAUNCH_DEPTH`, 5). Only the lead starts units directly; there are no
sub-leads.

`dispatch` refuses an agent that is not in `pool.agents`. A workflow unit is
checked before anything starts: it is in the pool, parses, has no approval gate
and no `host:` / `distribute:` placement, runs only pool agents, and its inputs
resolve. A refusal comes back to the lead as a tool error it can react to.

At most `[dispatch].max_concurrent_children` (default 8) units run at once,
across rounds; more wait as `pending` until one finishes. A unit started in one
round keeps running into the next, and is joined in a later round.

Every unit is recorded in the run's children ledger (`<run dir>/children.jsonl`),
which the control plane's unit list and the orphan reaper read. When the run
stops for any reason, every unit still running is sent SIGTERM (to its process
group, then SIGKILL after a grace) and recorded `abandoned`.

---

## 6. Budgets and metering

| Key | Meaning |
| --- | --- |
| `usd` | Spend cap in US dollars, priced through the layered `[pricing]` config |
| `tokens` | Cap on billable (input + output) tokens |
| `wall_clock` | Duration since launch: `Ns`, `Nm`, `Nh` or `Nd` |
| `rounds` | Cap on lead rounds |
| `soft_at` | Fraction (0 to 1, default `0.8`) of any cap at which the budget turns **soft** |

Every set dimension is enforced; unset ones never stop the run. A cap of `0` is
exhausted immediately (it is `hard` on the first check). When any dimension
passes `soft_at`, the lead's digest says to converge: finish the highest-value
work instead of opening new lines. Remember that the budget is checked only at
round boundaries (section 4).

Spend is metered once per round from the run's ledger (`<run dir>/usage.jsonl`).
The lead and any in-process sub-agents write it directly; each unit process, and
every child it starts, also copies its calls there as rollup rows, besides
keeping its own run ledger. Things to know:

- `budget.usd` is enforced only when the **lead's** model has a price. If it
  doesn't, the cap is dropped with a warning at launch (it would read `$0`
  forever), and `tokens`, `wall_clock` and `rounds` still apply. Add a
  `[pricing.<provider>."<model>"]` entry to enforce it (see
  [configuration.md](configuration.md#pricing)).
- A unit on an unpriced model adds `$0` (with a warning the first time) but its
  tokens still count. `rupu agentiflow status` names the unpriced models when a
  `usd` cap is set, so the under-count is visible.

The running spend is written to the run record after every round, so
`rupu agentiflow status` and the control plane show it live.

The lead can re-check spend mid-round with `budget.status`, which reports the
`stage` (`ok` / `soft` / `hard`, with `tripped` naming the dimension), and for
each set dimension its `cap`, what it has used (`spent`, `elapsed`,
`elapsed_secs`), what is `remaining` and its own `state`. A `usd` cap nothing
can price carries `enforced: false` and a note.

---

## 7. Verification

By default any recorded finding counts toward a findings goal. To count only
findings someone else has confirmed:

```yaml
goals:
  - id: injection
    objective: "Find and confirm two SQL injection issues."
    target:
      findings: { classification: "CWE-89" }
      count_gte: 2
      verified: true          # require an independent Confirmed verdict
      verify_check: with_poc  # optional: also require an artifact on the report
    verify_with: finding-verifier   # optional: only this agent's verdict counts
```

A finding counts only when all of these hold:

- its report carries a `confirmed` verification;
- the verification was recorded by a **different run** than the one that filed
  the finding (a run can never verify its own finding);
- with `verify_with`, the verifying agent is exactly that agent;
- with `verify_check: with_poc`, the report lists at least one artifact
  (`confirmed`, the default, needs only the verdict).

Setting `verify_with` turns verification on even without `verified: true`.
`verify_check` without either is rejected. Verification applies to findings
goals only; an asset goal's depth rung is its own evidence.

Verdicts are written by the `findings.verify` tool (`finding.verify` is its
alias). It is never granted automatically, not even to a `concerns:` agent.
Write a verifier agent that lists it, and put that agent in the pool:

```markdown
---
name: finding-verifier
description: Re-checks a finding another run filed and records a verdict.
findingsProfile: full
tools: [read_file, grep, glob, findings.verify]
---
Reproduce or refute the finding you are given. Record `confirmed` only if
you established it yourself.
```

`findings.verify` takes `{ finding_id, status (confirmed | disputed |
inconclusive), notes? }`; the verifier's run and agent are filled in from the
run context, never the tool input. The lead dispatches the verifier as a unit;
its separate run id is the independence. A finding can only be verified if it
has a full report, so the agents that file findings should use the `full`
findings profile (`findingsProfile: full`; see
[coverage.md](coverage.md#finding-reports)).

---

## 8. Operating a run from the CLI

| Command | What it does |
| --- | --- |
| `rupu agentiflow run <name>` | Run in the foreground until the envelope stops it, then print the outcome |
| `rupu agentiflow run <name> --detach` | Start the run in its own background process group and return once it has started, printing `agentiflow <name>: run <id> (detached)` (with `--format json`, an object with `id`, `name` and `run_dir`) |
| `rupu agentiflow list` | All runs, newest first: id, name, status, stop reason, rounds, goals met, start time |
| `rupu agentiflow status <id>` | One run: status, stop reason, rounds, goals, budget and spend |
| `rupu agentiflow attach <id>` | Follow the run's event log and the lead's current-round transcript until it ends |
| `rupu agentiflow attach <id> --no-follow` | Print what has been recorded so far and return (alias `--once`) |
| `rupu agentiflow send <id> "<message>"` | Send the lead a message; it gets it before its next model call |
| `rupu agentiflow send <id> "<message>" --now` | Urgent: also cut the lead's current round short |
| `rupu agentiflow stop <id>` | Graceful stop at the next round boundary (units are stopped with it) |
| `rupu agentiflow stop <id> --now` | Hard stop: signal the coordinator and its units immediately |
| `rupu agentiflow serve` | Run the orphan reaper in the foreground (section 11) |

There is no `rupu agentiflow show` (the command is `status`) and no `--goal`
flag on any subcommand. `<id>` can be the full `af_...` id, the short form `list`
prints, or any unique prefix or suffix. `run` and `status` take `--format json`;
`list` takes `--format json` or `csv`.

```bash
# Start in the background, watch it, steer it, end it
rupu agentiflow run review-sweep --detach
# agentiflow review-sweep: run af_01J... (detached)
rupu agentiflow attach af_01J...        # Ctrl-C detaches; the run keeps going
rupu agentiflow send af_01J... "Skip the vendored code under third_party/."
rupu agentiflow stop af_01J...
```

**Detaching.** The background process's stderr goes to `<run dir>/detach.log`.
The launching command waits up to 30 seconds for the run to start. If the run
fails before it starts (no credential, an invalid definition), the error is
printed by the launching command and no run directory is left behind. If it is
still starting after 30 seconds, the command says so and returns; the run keeps
going in the background.

**Messages.** `send` writes an operator message to the lead in the run's
message space ([messaging.md](messaging.md)) and prints its id. The lead gets
it as an operator instruction before its next model call, mid-round included,
and the next round's prompt recaps it. With `--now` the message is urgent: a
watcher sees it within about 200 ms and cuts the lead's round short at a safe
point (an in-flight model call is abandoned, a running tool finishes); the next
round starts at once and its first model call carries the message. A run that
isn't running, or whose coordinator is gone, takes no messages: `send` fails and
says why. (`rupu message send <id> --to lead` is the same thing;
`rupu agentiflow send` is shorthand.)

**Stopping.** A plain `stop` queues a stop request: the envelope ends the run at
the next round boundary as `operator_stop`. `stop --now` sends SIGTERM to the
coordinator (and, for a detached run, its whole process group) and to every unit
still running, waits up to 12 seconds, sends SIGKILL to whatever is left, and
records the run as `failed` with stop reason `operator_stop:now`.

**No resume.** There is no agentiflow resume. A run that fails, is hard-stopped,
or is reaped is over; start it again with `rupu agentiflow run`. (Unlike a
workflow, which `rupu workflow resume` can continue, an interrupted agentiflow
has no continuation path. The lead's per-round transcripts remain on disk to
read.)

---

## 9. Operating a run from the control plane

`rupu cp serve` shows agentiflow runs under **Runs → Agentiflows** in the
sidebar. You can launch, watch, steer and stop a run from the page, and manage
the definitions under **Library → Agentiflows**. (For the exact routes, see the
[Agentiflows section of cp-api.md](cp-api.md#agentiflows).)

### Launching

`POST /api/agentiflows` launches a run detached, as `rupu agentiflow run
--detach` does. On the web, the **New agentiflow** launcher sheet (on the runs
list and the definition pages) picks a definition and where the flow runs (the
layer the definition lives in, a registered project, or an absolute directory),
then opens the new run. A flow is local-only, so there is no host picker. The
definition is checked before anything starts (404 unknown, 400 invalid).

### The run page

The header shows a crew chip, the status, the engagement profiles, an
**Engagement summary** strip (goals met and budget bars, which open the
Engagement tab), the stop reason, and **Stop** / **Stop now** buttons. A
`running` run whose coordinator process is gone shows a banner saying it died
without finalizing and will be closed out by the orphan reaper.

- **Stop** queues a graceful stop (the lead stops at the next round boundary);
  the page shows a pending state until the run itself stops reading `running`.
- **Stop now** asks first, then kills the coordinator and its units and reloads
  the closed-out run. It is the only option when the coordinator is already
  gone.

Below the header, the **Flow** panel is always shown (it is not a tab). A
**Token flow** strip shows every model call of the lead and its units, grouped
by round; selecting a range narrows the Transcript, Events, Findings, Messages and Network tabs to
that window.

Then the tabs:

| Tab | Shows |
| --- | --- |
| Engagement | Goals (with progress bars and the goal predicate), the budget state, and the definition (lead, profiles, pool, lead turns) |
| Transcript | The lead's per-round transcripts, following the run |
| Events | `events.jsonl`, with per-unit lifecycle merged in, newest first |
| Findings | Findings filed by the lead and its units |
| Children | Every agent, sub-agent and workflow launched under the flow, any depth |
| Fleet | Every unit the lead dispatched and its status |
| Messages | The message space, with an operator composer that reaches the lead (refused once the run has finished or its coordinator is gone) |
| Network | Egress across the fleet (the netflow explorer) |
| Assets | Engagement assets, scoped to this run (hosts, services, sites, routes, files) with their depth rung |

### The flow map

The Flow panel is an unfolding map (xyflow). It reads as the lead's spine down
the left, a card per round, each round branching right into the children it
launched:

- A **round card** shows what the lead decided that round (its last reply, the
  `decision`), the goals met after it, the spend as it ended, how many children
  it launched, and a link to that round's lead transcript. A round whose lead
  call failed is drawn failed with the reason.
- A **child card** (an agent or sub-agent, any depth) shows its handle or
  address, the agent's identity, its one-line result or error, and its tokens,
  duration and transcript.
- A **workflow child** is drawn as a frame holding its real run graph. A frame
  the lead generated with `workflows.generate` is labelled "written by the lead
  in round N" with a button to view the YAML it wrote.
- **Messages** are drawn as arcs between cards, with a travelling packet when a
  message arrives and a red dead end for one that reached nobody.
- A **board rail** on the right shows claims (held, released, expired), board
  posts and standing directives.
- A **time axis** replays the run; **message-kind** and **participant** filters
  and a **round navigator** move through a long engagement.

Density: the last two finished rounds plus the live one stay expanded; a round
shows at most eight cards before "+N more".

### Library → Agentiflows

The definition pages (`/library?tab=agentiflows`, or the per-definition page)
list the `<name>.yaml` files `rupu agentiflow run` loads, global and per
project. Each row shows its layer, lead and engagement profiles, with a Launch
action. A definition page is an editor with a validity badge and Save / Revert /
Delete / Launch. **New agentiflow** creates one in the global layer.

---

## 10. Operating a run from MCP

An external agent operates an agentiflow over `rupu cp serve`'s `/mcp` endpoint
([mcp.md](mcp.md)). The agentiflow tools are:

| Tool | Does |
| --- | --- |
| `agentiflows.list` | Runs, newest first |
| `agentiflows.get` | One run's record |
| `agentiflows.events` | The recorded events |
| `agentiflows.usage_timeline` | The per-call token series (lead and units) |
| `agentiflows.children` | The units and their children |
| `agentiflows.unit_output` | One unit's output |
| `agentiflows.board` | The board (claims, directives) |
| `agentiflows.generated` | The YAML the lead wrote for a generated workflow child |
| `agentiflows.steer` | Message the lead, or ask the flow to stop |
| `agentiflows.run` | Launch a run from a definition |
| `agentiflows.stop` | Stop a run, gracefully or (with `now`) at once |
| `agentiflow_defs.list` / `.get` / `.create` / `.save` / `.delete` / `.validate` | Manage the definitions |

`wait { kind: "agentiflow", id, types? }` long-polls until the flow writes a
line (optionally only certain kinds, such as `round` or `run_stopped`); it is
local only. The `author-agentiflow` prompt (`prompts/get`) is a short
instruction for writing a definition, pointing at this page and the engagement,
coverage and messaging docs.

---

## 11. The orphan reaper

A coordinator that dies without finishing (SIGKILL, a crash, the machine going
down) would otherwise leave its run `running` forever and its detached units
using tokens with nobody steering them. The reaper finds `running` runs whose
recorded coordinator pid no longer exists, stops their units the same way
`stop --now` does, and records the run as `failed` with stop reason
`orphaned: coordinator pid <p> not running`.

Two things run it:

- `rupu cp serve`, on its gate-sweep tick (every `[cp].gate_sweep_interval_secs`);
- `rupu agentiflow serve`, a foreground loop for hosts that don't run the control
  plane. It sweeps at startup and then every `serve_interval_secs` until SIGTERM
  or Ctrl-C.

Both are configured under `[agentiflow]` (`serve_enabled`,
`serve_interval_secs`, `reaper_enabled`); see
[configuration.md](configuration.md#agentiflow). A run with no recorded pid is
treated as "owner unknown" and never reaped.

---

## 12. Where state lives

Each run gets a directory `~/.rupu/agentiflows/af_<ULID>/`:

| Path | Contents |
| --- | --- |
| `agentiflow.json` | The run record: status (`running`, `completed`, `failed`), stop reason, rounds, goal status, spend, coordinator pid, crew codename. Rewritten atomically after each round and at the end |
| `agentiflow.yaml` | The definition the run started from (re-serialized), so the run stays readable if the file is later edited |
| `events.jsonl` | Append-only events: `run_started`, one `round` per finished round (below), `run_stopped` |
| `usage.jsonl` | The run's usage ledger: the lead's calls, plus every unit's and their children's as rollup rows |
| `lead/transcript.r<N>.jsonl` | The lead's transcript for round N (and `lead/netflow.jsonl` for the launch) |
| `codenames.json` | The crew's role allocation and instance counters, so a round or unit keeps its codename |
| `steering/` | The operator's control queue: a graceful stop waiting for the round boundary |
| `board/` | `posts.jsonl`, `directives.jsonl`, `claims/` and the claim journal `claims.jsonl` |
| `mailboxes/` | Per-participant inboxes, the broadcast log and cursors |
| `log.jsonl`, `participants.jsonl` | The flow's message log and participant registry ([messaging.md](messaging.md)). A flow recorded before they existed has neither |
| `children.jsonl` | The children ledger: a `started` line per unit (kind, name, run id, codename, address, process group, the round that launched it) and a `finished` line when it ends |
| `units/<run id>/unit.json` | Runs started before the children ledger existed: one record per unit. Still read |
| `generated/` | Workflows written by `workflows.generate` |
| `detach.log` | stderr of a `--detach` run |

The **`round` event** (newest fields last) carries: `round`, `budget` (`ok` /
`soft` / `hard:<dim>`), `converge`, `goals_met`, `goals_total`, `steering` (the
count of operator messages the round's prompt recapped), `outcome` (`yielded` /
`turn_budget_hit` / `error`), `spent_usd`, `spent_tokens`, `started_at`, `goals`
(`[{id, met, current}]` as the round *started*, so the state after round N is
round N+1's `goals`), and optionally `decision` (the lead's last reply that
round, trimmed to about 600 characters), `warnings` (the digest's warnings) and
`error` (when the round errored). The `run_stopped` event carries `stop_reason`,
`detail`, `rounds`, `goals`, `summary`, `spent_usd` and `spent_tokens`.

Units' own runs are ordinary rupu runs: transcripts under the project's
`.rupu/transcripts/` (or `~/.rupu/transcripts/`), workflow units under
`~/.rupu/runs/<id>/`. Pooled evidence (findings, assets, coverage) lives in the
workspace under `.rupu/coverage/<target>/`, where `<target>` is a hash of the
workspace path and the run id, so two runs never share a scope. `rupu coverage
list` shows the target ids; `rupu coverage show <target>` shows the ledger.

---

## 13. API

The HTTP API behind the control plane, all on the local host, is documented in
[cp-api.md](cp-api.md#agentiflows) (the run routes) and
[cp-api.md](cp-api.md#agentiflow-definitions) (the definition routes). The board
shape is in [cp-api.md](cp-api.md#board). The MCP tools are in section 10 and
[mcp.md](mcp.md).

---

## 14. Limitations

What is not built, or only partly built, today:

- **No resume.** A run that fails, is hard-stopped, or is reaped cannot be
  continued; start it again. Its lead transcripts remain on disk.
- **Local only.** The control plane reads runs on its own host, and units always
  run locally: a workflow with `host:` or `distribute:` placement is refused as
  a unit. There is no remote-host listing for agentiflows.
- **`trigger:` is not acted on.** It is parsed and stored, but nothing schedules
  an agentiflow. Use cron to call `rupu agentiflow run --detach` if you need one.
- **`scope` is shown, not enforced.** The lead's opening-round prompt lists
  `scope.mode` and every `scope.roots` entry as the authorized targets, but
  nothing stops an agent from reaching beyond them; put hard constraints in the
  goal objectives too, and give the pool only agents you trust unattended.
- **No sub-leads.** Only the lead starts units.
- **Workflows with approval gates can't run as units.** A unit has no terminal to
  answer the prompt.
- **Verification is findings-only.** Asset goals can't require verification.
- **No CLI definition listing or dry-run.** The CLI validates at launch; listing
  and validating a definition without running it is on the control plane and MCP
  only (`/api/agentiflow-defs`, `agentiflow_defs.validate`).
- **No stock agentiflow definition ships.** `rupu fleet install` lays down the
  pool agents and workflows, but you write the definition. The
  [tutorial](agentiflow-tutorial.md) and the repo's
  `.rupu/agentiflows/*.yaml` examples give something to copy.

---

## 15. Troubleshooting

| Symptom | Likely cause |
| --- | --- |
| `budget.usd` shows `$0` / under-counts | The lead's (or a unit's) model has no price. Add a `[pricing.<provider>."<model>"]` entry; a `usd` cap on an unpriced lead is dropped at launch. `rupu agentiflow status` names the unpriced models |
| A warning that the run "has no automatic terminator" | No required goal, no coverage target, no enforceable budget cap, no ceiling. Only an operator stop can end it. Add a cap or a ceiling |
| `workflows.generate` is not offered | The generating provider has no credential, or it authenticates with Anthropic OAuth (which can't be built where the tool runs). Use an API-key provider |
| `dispatch refused: ... is not in this flow's pool` | The agent or workflow is not in `pool.agents` / `pool.workflows`. Add it to the pool |
| `v1 cannot run a gated or host/distribute workflow as a child` | The workflow has an `approval:` gate or `host:` / `distribute:` placement. Units run unattended and locally |
| Coverage stuck at `0%` | No assets marked yet, or the kinds enumerated don't match. The agents must call `assets.mark`; check the Assets tab |
| The run reads `running` but nothing is happening | The coordinator died. The page shows a dead-coordinator banner; the orphan reaper (section 11) will close it as `orphaned:`, or `rupu agentiflow stop <id> --now` closes it now |
| A `--detach` run never printed an id | It failed before starting. Read `<run dir>/detach.log`, or the error the launching command printed |
| A goal never reaches `met` though findings exist | The `classification` must match a classification on the finding's *report*; a summary-profile finding has none. For a verified goal, the verifier needs the `full` profile and must be a different run |

---

## See also

- [agentiflow-tutorial.md](agentiflow-tutorial.md): an end-to-end walkthrough
- [agentiflow-authoring.md](agentiflow-authoring.md): writing a lead, goal-target recipes, budget tuning, pool design, steering
- [configuration.md](configuration.md#agentiflow): `[agentiflow]` settings; `[pricing]` for `budget.usd`
- [engagement-profiles.md](engagement-profiles.md): engagement profiles, asset kinds and the asset ledger
- [coverage.md](coverage.md): findings and finding reports
- [messaging.md](messaging.md): the board, mailboxes and operator messages
- [agent-format.md](agent-format.md): agent files, `tools:` and `findingsProfile`
- [workflow-format.md](workflow-format.md): workflows a lead can run as units
- [cp-api.md](cp-api.md#agentiflows): the HTTP API
- [mcp.md](mcp.md): operating a run from MCP
