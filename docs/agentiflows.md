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
set `scope.authorized: true` (section 2). Give the pool only agents you are
happy to run unattended in the workspace you launch from.

---

## 2. The definition file

A definition is one YAML file per agentiflow:

- `<project>/.rupu/agentiflows/<name>.yaml` (project), which shadows
- `~/.rupu/agentiflows/<name>.yaml` (global).

A project file shadows the global one even when it fails to parse: a broken
project definition is an error, never a silent fallback to the global copy. The
file name (without `.yaml`) is what you pass to `rupu agentiflow run`. Run
directories also live under `~/.rupu/agentiflows/` (as `af_<ULID>/`); they never
collide with definitions.

A minimal example: a code review that stops once two injection findings have
been confirmed by an independent verifier and 80% of the repository's files have
been reviewed, or when the budget runs out.

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

coverage:
  reach: 0.8          # 80% of discovered files...
  depth: reviewed     # ...at the `reviewed` rung or deeper
  kinds: [file]

budget:
  usd: 20
  tokens: 4000000
  wall_clock: "3h"
  soft_at: 0.8        # past 80% of any cap, the lead is told to converge

scope:
  authorized: true
  roots:
    - kind: code:repo
      path: "."

pool:
  agents: [review-lead, file-reviewer, finding-verifier]
  workflows: [lint-and-test]

round:
  lead_max_turns: 40
  ceiling: { rounds: 30, wall_clock: "4h" }
```

### Fields

| Key | Required | Notes |
| --- | --- | --- |
| `name` | yes | Display name, recorded on the run |
| `description` | no | Used as the lead's mission text when there are no goals or coverage target |
| `lead` | yes | Agent name of the lead. Must also appear in `pool.agents` |
| `engagement_profiles` | yes | Profile ids that type the run's evidence (`code`, `network`, `web`, `binary`, a composite like `pentest`, ...). See [Engagement profiles](coverage.md#engagement-profiles) |
| `goals` | one of `goals` / `coverage` | Measurable targets, below |
| `coverage` | one of `goals` / `coverage` | `{ reach, depth?, kinds? }`, below |
| `budget` | no | `{ usd?, tokens?, wall_clock?, rounds?, soft_at? }`, section 5 |
| `scope.authorized` | yes | Must be `true`. It is the operator's statement that the fleet may act on the targets unattended |
| `scope.roots` | no | Root assets of the engagement: `kind` (a root asset kind of an active profile, such as `code:repo`, `network:host`, `web:site`) plus that kind's coordinates |
| `scope.mode` | no | Accepted and stored, not acted on (section 10) |
| `pool.agents` | yes | The only agents the lead may `dispatch` |
| `pool.workflows` | no | A list of workflow names, or `all` for every workflow in the catalog at launch. The only workflows `run_workflow` may start |
| `round.lead_max_turns` | no | Turns the lead may take per round. Default `50`, minimum `1` |
| `round.ceiling` | no | `{ rounds?, wall_clock? }`: a hard end to the run |
| `trigger` | no | Accepted and stored, not acted on (section 10) |

Unknown keys are rejected.

### Goals

Each goal is a predicate over the run's own evidence. A target names either
findings or an asset, never both:

```yaml
# Findings: at least N findings, optionally of one classification
target: { findings: { classification: "CWE-79" }, count_gte: 3 }

# Asset: a specific asset reached a depth rung on its profile's ladder
target:
  asset: { kind: "network:host", locator: { host: "10.0.0.5" } }
  depth_at_least: tested
```

- `count_gte` is required on a findings target. `classification` matches any
  classification on the finding's report (a finding without a report matches
  nothing when a classification is given).
- `depth_at_least` is required on an asset target, and must be a rung of the
  owning profile's depth ladder. `locator` keys are limited to the string
  coordinates `host`, `url`, `path`, `symbol`, `sha256`, `commit` and `param`.
- `required` defaults to `true`. Only required goals can end the run as
  `goals_met`; a run with no required goal never stops that way.
- `verified`, `verify_with` and `verify_check` gate a findings goal on
  independent verification. See section 6.

### Coverage

`coverage` is an engagement-wide stop: the fraction of discovered assets (of the
kinds the profiles enumerate, or of `kinds` when given) that reached `depth` or
deeper. `reach` must be in `(0, 1]`. Every kind and the depth are checked
against the active profiles at launch, so a coverage stop that could never fire
is rejected instead of silently burning the budget.

### Validation

`rupu agentiflow run` validates the definition against the resolved profiles
before anything starts, and refuses to launch when, for example:
`scope.authorized` is not `true`; there are neither goals nor a coverage target;
the lead is not in `pool.agents`; a goal names a kind or depth no active profile
defines; a `verify_with` agent is not in `pool.agents`; a scope root is not a
root kind; or a budget value is out of range.

The command also warns (but still runs) when nothing except an operator stop can
end the run: no required goal, no coverage target, no enforceable budget cap and
no ceiling.

---

## 3. How a run proceeds

```bash
rupu agentiflow run review-sweep
```

The command runs from your current directory, which becomes the run's
**workspace**: the lead's file and shell tools act there, and units start
there. It resolves configuration the same way `rupu run` does (global, then
customer, then project layer), loads the lead agent, builds its provider, and
prints the run id to stderr.

### Rounds

The run is a loop of rounds. At the top of each round the envelope:

1. drains operator steering messages (section 7);
2. evaluates every goal, the coverage target and the budget over the run's
   evidence;
3. stops the run if a stop condition holds, checked in this order (first match
   wins):

   | Stop reason | When |
   | --- | --- |
   | `goals_met` | at least one goal is required, and every required goal is met |
   | `coverage_reached` | the coverage target is met |
   | `budget_exhausted:<dimension>` | a budget cap is reached (`usd`, `tokens`, `rounds` or `wall_clock`) |
   | `operator_stop` | a graceful stop was requested |
   | `ceiling` | `round.ceiling.rounds` or `round.ceiling.wall_clock` is reached |

4. otherwise hands the lead a digest (goal progress, coverage, budget stage,
   steering, warnings) and lets it run one round of up to `lead_max_turns` turns.

The lead's conversation carries over from round to round. A round that errors is
recorded and the loop continues; the budget and ceilings are what bound a lead
that keeps failing. Because the check comes before the lead's turn, a run whose
goals are already met runs zero rounds.

An evaluation that fails (an unreadable ledger, for instance) counts as **not
met**, never as met, and is passed to the lead as a warning. A broken evaluator
can't end a run as a success.

### What the lead can do

The lead gets the tools its own agent file lists in `tools:` (none of the
built-ins if `tools:` is absent), plus `report_finding` (and `asset_mark` under an
engagement profile), plus these always-on fleet tools:

| Group | Tools | Purpose |
| --- | --- | --- |
| Units | `dispatch { agent, prompt }` | Start a pool agent as a unit. Returns a handle at once |
| | `run_workflow { workflow, inputs? }` | Start a pool workflow as a unit |
| | `generate_workflow { description, inputs? }` | Write a new workflow and start it as a unit (only offered when generation is available, below) |
| | `join { handle, timeout_secs? }` | Wait for a unit's result. Default 300 s, at most 3600 s per call |
| Board | `board.claim`, `board.release` | Claim a piece of work (a one-hour lease) so two participants don't duplicate it |
| | `board.post`, `board.read` | Shared posts of kind `observation`, `question`, `answer`, `vote` or `note` |
| | `msg.send` | Message one participant's inbox, or `broadcast` |
| Steering | `board.directive` | Write a standing directive every agent unit sees on every turn. Returns an id |
| | `board.retract { id }` | Lift a directive. Retraction is appended to the log; the board is never rewritten |
| Status | `goal.status`, `coverage.status`, `budget.status` | Re-check goals, coverage and spend mid-round with the same evaluators and meter the envelope uses |
| Roster | `agents.list`, `agents.get`, `workflows.list`, `workflows.get`, `catalog.search` | Browse the agents and workflows it can draw on |

Each lead turn also includes the lead's inbox, the standing directives, and a
short index of the pool, so it knows what it can start without spending a turn
on `agents.list`.

### Units

A unit is a separate process:

- an **agent unit** is `rupu run <agent> --mode bypass ...`;
- a **workflow unit** is `rupu workflow run <name> --mode bypass --plain ...`
  (or `--file <path>` for a generated workflow).

Each unit gets a unique participant id (`<name>#<n>`) and the run's engagement
profiles. Agent units also get the board, mailbox and directive tools; workflow
units are findings-only (their steps get no board tools). Its findings and assets
are pooled with the lead's under one coverage scope, which is what the goals are
evaluated against. Units don't get `dispatch`, `run_workflow` or
`generate_workflow`, so they can't start units of their own (an agent's
in-process `dispatch_agent` sub-agents still work).

`dispatch` refuses an agent that is not in `pool.agents`. `run_workflow` checks,
before anything starts, that the workflow is in the pool, parses, has no
approval gate and no `host:`/`distribute:` placement, dispatches only pool
agents, and that its inputs resolve. A refusal comes back to the lead as a tool
error it can react to.

`generate_workflow` writes a new workflow, applies the same checks, saves it as
`<run dir>/generated/<name>-<ulid>.yaml`, and runs it as a unit. The generating
provider is the first authenticated one in rupu's default generation order,
falling back to the lead's own. When that provider has no credential, or
authenticates with Anthropic OAuth (which can't be built where the tool runs),
the tool isn't offered and a warning says why. An API key enables it.

When the run stops for any reason, every unit still running is sent SIGTERM.

---

## 4. Engagement profiles

`engagement_profiles` is what makes goals and coverage meaningful outside source
code. The profiles declare the asset kinds (`network:host`, `web:route`,
`code:file`, ...), their coordinates, and the depth ladder (`discovered →
enumerated → tested → exploited` for `network`; `unreviewed → reviewed` for
`code`). The definition is validated against them, the lead and every unit run
under them, and `asset_mark` records how deep each asset was examined.

Profiles resolve the same way as everywhere else: built-ins, then
`~/.rupu/profiles/`, then `.rupu/profiles/` in the directory you launch from. A
profile only types and validates evidence. It is not a sandbox and does not
limit what the agents' tools can reach. See
[coverage.md](coverage.md#engagement-profiles).

---

## 5. Budgets and metering

| Key | Meaning |
| --- | --- |
| `usd` | Spend cap in US dollars, priced through the layered `[pricing]` config |
| `tokens` | Cap on billable (input + output) tokens |
| `wall_clock` | Duration since launch: `Ns`, `Nm`, `Nh` or `Nd` |
| `rounds` | Cap on lead rounds |
| `soft_at` | Fraction (0 to 1, default `0.8`) of any cap at which the budget turns **soft** |

Every set dimension is enforced; unset ones never stop the run. A cap of `0` is
exhausted immediately. When any dimension passes `soft_at`, the lead's digest
says to converge: finish the highest-value work instead of opening new lines.

Spend is metered once per round from the lead's ledger (`<run dir>/usage.jsonl`)
plus each unit's own run ledger. Things to know:

- `budget.usd` is enforced only when the **lead's** model has a price. If it
  doesn't, the cap is dropped with a warning at launch (it would read `$0`
  forever), and `tokens`, `wall_clock` and `rounds` still apply. Add a
  `[pricing.<provider>."<model>"]` entry to enforce it.
- A unit on an unpriced model adds `$0` (with a warning the first time) but its
  tokens still count.
- Sub-agents a unit starts with `dispatch_agent` are undercounted, so a cap can
  trip later than the true spend.

The running spend is written to the run record after every round, so
`rupu agentiflow status` and the control plane show it live.

---

## 6. Verification (opt-in)

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

Verdicts are written by the `finding.verify` tool. It is never granted
automatically, not even to a `concerns:` agent. Write a verifier agent that lists
it, and put that agent in the pool:

```markdown
---
name: finding-verifier
description: Re-checks a finding another run filed and records a verdict.
tools: [read_file, grep, glob, finding.verify]
---
Reproduce or refute the finding you are given. Record `confirmed` only if
you established it yourself.
```

The lead dispatches the verifier as a unit; its separate run id is the
independence. A finding can only be verified if it has a full report, so the
agents that file findings should use the `full` findings profile
(`findingsProfile: full`; see [coverage.md](coverage.md#finding-reports)).

---

## 7. Operating a run

| Command | What it does |
| --- | --- |
| `rupu agentiflow run <name>` | Run in the foreground until the envelope stops it, then print the outcome |
| `rupu agentiflow run <name> --detach` | Start the run in its own background process group and return once it has started, printing `agentiflow <name>: run <id> (detached)` (with `--format json`, an object with `id`, `name` and `run_dir`) |
| `rupu agentiflow list` | All runs, newest first: id, name, status, stop reason, rounds, goals met, start time |
| `rupu agentiflow status <id>` | One run: status, stop reason, rounds, goals, budget and spend |
| `rupu agentiflow attach <id>` | Follow the run's event log and the lead's current-round transcript until it ends |
| `rupu agentiflow attach <id> --no-follow` | Print what has been recorded so far and return (alias `--once`) |
| `rupu agentiflow send <id> "<message>"` | Queue a steering message for the lead's next round |
| `rupu agentiflow send <id> "<message>" --now` | Interrupt: cut the lead's current round short, then deliver the message |
| `rupu agentiflow stop <id>` | Graceful stop at the next round boundary (units are stopped with it) |
| `rupu agentiflow stop <id> --now` | Hard stop: signal the coordinator and its units immediately |
| `rupu agentiflow serve` | Run the orphan reaper in the foreground (section 8) |

`<id>` can be the full `af_...` id, the short form `list` prints, or any unique
prefix or suffix. `run` and `status` take `--format json`; `list` takes
`--format json` or `csv`.

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
fails before it starts (no credential, an invalid definition, ...), the error is
printed by the launching command and no run directory is left behind. If it is
still starting after 30 seconds, the command says so and returns; the run keeps
going in the background.

**Steering.** A message is queued under the run's `steering/` directory and
delivered to the lead as an operator instruction at the next round boundary.
With `--now`, a watcher sees the message within about 200 ms and cuts the
lead's round short at a safe point (an in-flight model call is abandoned, a
running tool finishes); the next round starts at once with the message
delivered. A run that has finished takes no messages.

**Stopping.** A plain `stop` queues a stop request: the envelope ends the run at
the next round boundary as `operator_stop`. `stop --now` sends SIGTERM to the
coordinator (and, for a detached run, its whole process group) and to every unit
still running, waits up to 12 seconds, sends SIGKILL to whatever is left, and
records the run as `failed` with stop reason `operator_stop:now`.

---

## 8. The orphan reaper

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

## 9. Where state lives

Each run gets a directory `~/.rupu/agentiflows/af_<ULID>/`:

| Path | Contents |
| --- | --- |
| `agentiflow.json` | The run record: status (`running`, `completed`, `failed`), stop reason, rounds, goal status, spend, coordinator pid. Rewritten atomically after each round and at the end |
| `agentiflow.yaml` | The definition the run started from, so the run stays readable if the file is later edited |
| `events.jsonl` | Append-only events: `run_started`, one `round` per finished round (budget stage, goals met, steering count, outcome, spend), `run_stopped` |
| `usage.jsonl` | The lead's usage ledger |
| `lead/transcript.r<N>.jsonl` | The lead's transcript for round N |
| `steering/` | Queued operator messages |
| `board/` | `posts.jsonl`, `directives.jsonl` and `claims/` |
| `mailboxes/` | Per-participant inboxes and the broadcast log |
| `units/<run id>/unit.json` | One record per unit: kind, process group and last known status |
| `generated/` | Workflows written by `generate_workflow` |
| `detach.log` | stderr of a `--detach` run |

A run that completes normally has status `completed` whatever the reason; the
reason is in `stop_reason`. `failed` means the run couldn't start, crashed, was
hard-stopped or was reaped.

Units' own runs are ordinary rupu runs: transcripts under the project's
`.rupu/transcripts/` (or `~/.rupu/transcripts/`), workflow units under
`~/.rupu/runs/<id>/`. Pooled evidence (findings, assets, coverage) lives in the
workspace under `.rupu/coverage/<target>/`, scoped by the run id, so two runs
never share a scope.

---

## 10. In the control plane

`rupu cp serve` shows agentiflow runs under **Runs → Agentiflows** in the
sidebar: a table of every run (status, stop reason, rounds, goals, spend,
started). Opening a run shows a header with goals and budget usage, and six
tabs:

| Tab | Shows |
| --- | --- |
| Flow | The run as a graph: the lead's rounds as a spine with each round's units branching off it, plus a Fleet list of every unit and its status |
| Assets | Engagement assets (hosts, services, sites, routes, files) with their depth rung |
| Findings | Findings filed by the lead and its units, in the shared findings table |
| Messages | The board (posts and directives), with a box to send the lead a steering message, queued for the round boundary or sent mid-round |
| Transcript | The lead's transcript for the latest round, tailing live |
| Events | `events.jsonl`, newest first |

A running run refreshes every 5 seconds.

The API behind it, all on the local host:

| Endpoint | Purpose |
| --- | --- |
| `GET /api/agentiflows` | Run list, newest first |
| `GET /api/agentiflows/:id` | Record, definition snapshot, events, units, lead transcript paths |
| `GET /api/agentiflows/:id/messages` | Board posts and directives |
| `POST /api/agentiflows/:id/steer` | `{ message, now?, stop? }`: queue a steering message (409 when the run isn't running) |
| `GET /api/findings?run_id=<id>` | The run's findings (the lead's and every unit's) |
| `GET /api/assets` | Asset inventory (`?ws_id=` / `?target=` to scope it) |

---

## 11. Limitations

What is not built, or only partly built, today:

- **No launching from the control plane.** Runs start only with
  `rupu agentiflow run`. The CP observes and steers.
- **Local only.** The CP reads runs on its own host. There is no remote-host
  listing, and units always run locally: workflows with `host:` or `distribute:`
  can't be run as units.
- **`trigger:` is not acted on.** It is parsed and stored, but nothing schedules
  an agentiflow. Use cron to call `rupu agentiflow run --detach` if you need one.
- **`scope.mode` is not acted on**, and `scope.roots` is only validated: neither
  is passed to the lead. Put targets and constraints in the goal objectives or
  `description` so the lead sees them.
- **No sub-leads.** Only the lead can start units.
- **Workflows with approval gates can't run as units.** A unit has no terminal to
  answer the prompt.
- **Verification is findings-only.** Asset goals can't require verification.
- **The CP has no stop button.** The steer API accepts `stop: true`, but the web
  page only sends messages; use `rupu agentiflow stop`.
- **The CP Assets tab is not scoped to the run.** It currently lists assets from
  every registered workspace, like the Security → Assets page.
- **Codenames are derived.** Agentiflow records don't store a codename yet, so
  the CP derives one from the run id.
- **No definition listing or dry-run validation.** There is no
  `rupu agentiflow` subcommand that lists or checks definitions; `run` validates
  at launch.
- **Metering undercounts nested sub-agents** (section 5).

---

## See also

- [configuration.md](configuration.md#agentiflow): `[agentiflow]` settings; `[pricing]` for `budget.usd`
- [coverage.md](coverage.md): findings, finding reports, engagement profiles and assets
- [agent-format.md](agent-format.md): agent files, `tools:` and `findingsProfile`
- [workflow-format.md](workflow-format.md): workflows a lead can run as units
