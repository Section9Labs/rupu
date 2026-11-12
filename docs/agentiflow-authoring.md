# Authoring agentiflows

How to write the two things an agentiflow run turns on: the **lead** agent that
plans the work, and the **definition** that says when it is done. This is the
practical companion to the [reference](agentiflows.md); for a start-to-finish run
see the [tutorial](agentiflow-tutorial.md).

---

## Writing a lead prompt

The lead is an ordinary agent file ([agent-format.md](agent-format.md)) that
happens to be handed orchestration tools. Three things make a good lead.

### What the lead sees each round

At the top of every round the envelope hands the lead a digest, rendered as a
plain message (section 4 of [agentiflows.md](agentiflows.md#the-round-digest-what-the-lead-sees)
has the full shape). Round 0 adds the mission framing, the mission objective, and
the authorized scope. Every round then shows:

- each goal as `[MET]/[UNMET] <id>: <current>/<target> (<detail>) - <objective>`;
- the coverage fraction, if a coverage target is set;
- the budget stage (`ok` / `soft cap reached (converge)` / `hard`), with a
  converge instruction when the budget is soft;
- a recap of operator messages since the last round;
- any evaluator warnings, quoted as data.

Between rounds the lead's conversation carries over, so it remembers what it
dispatched. Write the prompt so the lead reads the goal lines and the budget
stage and acts on them: dispatch toward the unmet goals, and converge when told.

### The lead's tools

The lead gets exactly the tools its `tools:` lists, plus the fleet tools
(`dispatch`, `join`, `workflows.generate`, the board, mailbox, status and roster
tools) and the engagement's `findings.report` / `assets.mark`. **`tools:` is the
whole of its own grant** — an absent `tools:` grants no built-ins, so give the
lead the read tools it needs to orient (`read_file`, `grep`, `glob`, maybe
`bash`). Keep it lean: the lead orchestrates, the units do the work.

Tell the lead in the prompt which fleet tools it has and what they are for.
A lead that doesn't know it can `dispatch` will try to do everything itself.

### The round shape: dispatch, then join

The core loop is dispatch-then-join:

1. **Fan out.** `dispatch` the specialists at tightly-scoped, independent pieces
   of work, in parallel. Each `dispatch` returns a handle at once.
2. **Collect.** `join` the handles and read what came back. `join { until: any }`
   returns as soon as one finishes if you want to react early; the default waits
   for all. Each `join` waits up to `timeout_secs` (default 300, max 3600); call
   it again to keep waiting on a unit that is still running.
3. **Decide.** Check `goal.status` and `budget.status`, record what you verified,
   and dispatch the next batch.

A lead that dispatches everything in round 1 and never joins will hit its
`lead_max_turns` with units still running; they carry into the next round, but
the lead should join them deliberately rather than by accident.

### Claims: avoid duplicate work

When two units could pick up the same piece of work (two reviewers over
overlapping files, two testers over the same host), have the lead or the units
`board.claim` a work unit first. A claim is a one-hour lease; `board.release`
frees it. The claim journal (`board/claims.jsonl`) records who held what. Claims
persist across rounds, since the lead keeps one board for the whole run.

### Directives: standing scope rules

`board.directive` writes a standing instruction every agent unit sees on every
turn until it is lifted with `board.retract`. Use it for a rule that must hold
for the rest of the run, not a one-off instruction:

> `board.directive { body: "Stay inside 10.20.0.0/24. Never touch .1 (the gateway)." }`

Directives are the lead's alone (units cannot write them). Post sparingly and
keep each short: every unit re-reads every live directive each turn. Retract one
once it is obsolete; the id comes back from `board.directive` and is shown in
brackets on each directive the lead sees.

### Mid-round status checks

The lead doesn't have to wait for the next round's digest. It can call, any time:

- `goal.status` — every goal's `satisfied`, `current`, `target`, `detail`;
- `goal.coverage` (alias `coverage.status` inside a lead) — the coverage
  `fraction`, `satisfied`, and `per_kind`;
- `budget.status` — the `stage`, and each dimension's `cap`, `spent` /
  `elapsed`, `remaining` and `state`.

These use the exact evaluators and meter the envelope's next round-boundary check
will use, so "`goal.status` says met" and "the envelope stops the run" agree.
Have the lead check `goal.status` before deciding it is done, and `budget.status`
before opening a new line of work.

### End each round with a summary

Have the lead finish each round with a short summary of what it did that round.
That last assistant message becomes the `decision` recorded on the round's
event, and it is what the control plane's flow map shows on the round card. A
lead that ends a round mid-tool-call or with no reply leaves the round card
blank. The default prompt tail ("finish the round with a short summary of what
you did") asks for exactly this; keep it.

---

## Goal-target recipes

Every goal is a predicate over recorded evidence. Pick the target shape that
matches what "done" means.

### N findings of a classification

```yaml
- id: xss
  objective: "Find three reflected XSS issues."
  target: { findings: { classification: "CWE-79" }, count_gte: 3 }
  required: true
```

The `classification` must match a classification on the finding's **report**
(`CWE-79`, `OWASP-A03`, ...). A summary-profile finding has no report, so it
matches nothing when a classification is set. Omit `classification` to count any
finding.

### N findings, verified with a PoC, by a named verifier

```yaml
- id: confirmed-rce
  objective: "Confirm two RCEs with a proof of concept."
  target:
    findings: { classification: "CWE-78" }
    count_gte: 2
    verified: true
    verify_check: with_poc
  verify_with: exploit-verifier
  required: true
```

A finding counts only when a *different* run records a `confirmed` verdict, the
verifier is `exploit-verifier`, and the report lists at least one artifact
(`with_poc`). The reviewers that file must use `findingsProfile: full`. See
[verifier setup](#verifier-setup).

### A specific host (or path, or URL) reached a depth

```yaml
- id: host-tested
  objective: "Test 10.0.0.5 to the `tested` rung."
  target:
    asset: { kind: "network:host", locator: { host: "10.0.0.5" } }
    depth_at_least: tested
  required: true
```

An asset goal counts assets of `kind` matching the `locator` at or above
`depth_at_least` on that profile's ladder. `count_gte` defaults to 1. Locator
keys are the string coordinates `host`, `url`, `path`, `symbol`, `sha256`,
`commit`, `param`. Omit `locator` to match any asset of that kind, with
`count_gte` for "N of them".

### Every route enumerated (N of a kind)

```yaml
- id: routes
  objective: "Enumerate at least twenty web routes."
  target:
    asset: { kind: "web:route" }
    depth_at_least: enumerated
  count_gte: 20
  required: true
```

### 80% of files reviewed (coverage, not a goal)

Coverage is an engagement-wide stop, separate from goals:

```yaml
coverage:
  reach: 0.8          # 80% of...
  depth: reviewed     # ...files at `reviewed` or deeper
  kinds: [code:file]
```

Without `kinds`, the profiles' enumerated kinds are used; without `depth`, each
profile's last ladder rung. `0/0` is `0%`, so a coverage stop never fires before
assets exist.

### Gotchas

- **Classification must match the report, exactly.** `CWE-89` does not match a
  report classified only `CWE-564`. Agree on the classification the filers use.
- **Verifiers need the full profile.** A finding without a full report cannot be
  verified; a summary-profile finding never counts toward a verified goal.
- **A run with only optional goals never ends as `goals_met`.** At least one goal
  must be `required: true`, or the run is governed only by coverage, the budget,
  the operator and the ceilings.
- **`count_gte` is a floor, not a ceiling.** The run stops the first round the
  floor is met; tell the lead in its prompt to keep recording real findings past
  the floor if that is what you want, but know the envelope stops at the floor.

---

## Tuning budgets

Budgets bound a lead that would otherwise run forever. The subtlety is **caps are
checked only at round boundaries** (section 4 of
[agentiflows.md](agentiflows.md#caps-are-checked-only-at-round-boundaries)):
nothing enforces a cap mid-round.

### Worked example: the overshoot

Say `budget.usd: 20`, `lead_max_turns: 40`, and a round averages $3. The envelope
checks spend before each round:

- The run reaches round 7 at $18 (below $20): the check passes, round 7 runs.
- Round 7 spends another $4, reaching $22. The run is now over cap.
- The *next* check (top of round 8) sees $22, trips `hard`, and stops the run as
  `budget_exhausted:usd`.

So a `usd: 20` cap can land anywhere up to one round's spend over. If that
overshoot matters, lower the cap, lower `lead_max_turns` (so a round can spend
less before the next check), or cap the units' own `maxTurns`. A `join` inside a
round can wait up to 3600 seconds, so `wall_clock` overshoots the same way.

### `soft_at` and converging

`soft_at` (default `0.8`) is the fraction of any cap at which the budget turns
**soft**: the lead's digest tells it to converge — finish the highest-value work,
verify and record what it already has, and open no new lines. Lower `soft_at`
(`0.6`) to make the lead wind down earlier relative to the cap; the earlier it
converges, the smaller the final overshoot.

```yaml
budget:
  usd: 30
  tokens: 8000000
  wall_clock: "4h"
  soft_at: 0.7
```

### `budget.rounds` versus `ceiling.rounds`

Both cap the round count. `budget.rounds` produces `budget_exhausted:rounds`,
contributes to the soft stage (the lead is warned as it nears the cap), and is
checked before an operator stop. `ceiling.rounds` produces `ceiling`, has no soft
stage, and is checked after an operator stop. Use `budget.rounds` when you want
the lead to converge toward the end; use `ceiling.rounds` as a plain backstop.
The same holds for `budget.wall_clock` versus `ceiling.wall_clock`.

### `budget.usd` and pricing

`budget.usd` is enforced only when the **lead's** model has a price (in the
built-in table or `[pricing.<provider>."<model>"]`). Otherwise the cap is dropped
at launch, with a warning, since it would read `$0` forever; `tokens`,
`wall_clock` and `rounds` still apply. A **unit** on an unpriced model counts its
tokens but adds `$0` to the usd meter, so a `usd` cap under-counts when units run
unpriced models. `rupu agentiflow status` names the unpriced models when a usd
cap is set. The fix is a price entry; see
[configuration.md](configuration.md#pricing).

### A belt-and-braces budget

For an unattended run you want to really end:

```yaml
budget: { tokens: 10000000, wall_clock: "6h", soft_at: 0.75 }
round:
  lead_max_turns: 40
  ceiling: { rounds: 40, wall_clock: "8h" }
```

`tokens` and `wall_clock` always enforce (no pricing needed); the `ceiling` is a
hard backstop above the budget. The launch warns if *nothing* can end the run.

---

## Verifier setup

Verification counts only findings an independent run confirmed. To set it up:

1. **Write a verifier agent** that lists `findings.verify` (or its alias
   `finding.verify`) in `tools:` and uses `findingsProfile: full`:

   ```markdown
   ---
   name: finding-verifier
   description: Independently confirms or refutes a finding another run filed.
   permissionMode: bypass
   maxTurns: 80
   findingsProfile: full
   tools: [read_file, grep, glob, bash, findings.verify]
   ---
   Reproduce or refute the finding you are given. Record `confirmed` with
   `findings.verify` only if you established it yourself; `disputed` if it is
   wrong; `inconclusive` otherwise. Capture proof.
   ```

   `findings.verify` is never granted automatically, not even to a `concerns:`
   agent — the verifier must name it.

2. **Put the verifier in the pool** and name it on the goal:

   ```yaml
   goals:
     - id: confirmed
       target: { findings: {}, count_gte: 1, verified: true }
       verify_with: finding-verifier   # must be a pool agent
   pool:
     agents: [lead, reviewer, finding-verifier]
   ```

3. **Make the filers use the full profile.** A finding without a full report
   cannot be verified. The agents that call `findings.report` need
   `findingsProfile: full`.

4. **Have the lead dispatch the verifier as a unit.** Its separate run id is the
   independence: a run can never verify its own finding. In the lead prompt,
   make verification an explicit step ("dispatch `finding-verifier` to confirm
   each candidate before counting it").

`verify_check: with_poc` additionally requires an artifact on the report (a
proof of concept), not just the verdict.

---

## Pool design checklist

The `pool` is the lead's whole menu: it can dispatch nothing outside it, and
neither can the units.

- [ ] **The lead is in `pool.agents`.** Validation requires it.
- [ ] **Every `verify_with` agent is in the pool.** Otherwise the goal can never
      be met, and the launch is refused.
- [ ] **The filers use `findingsProfile: full`** if any goal is verified.
- [ ] **The specialists cover the work.** A code sweep needs a reviewer and a
      verifier; a network sweep needs recon, analysis and verification. The lead
      can only dispatch what you list.
- [ ] **Workflows the lead may run are in `pool.workflows`.** Omit the key for
      none, or `all` for every catalog workflow at launch. A workflow unit must
      have no approval gate and no `host:` / `distribute:` placement, and may run
      only pool agents.
- [ ] **Every pool agent is one you'd run unattended under `bypass`.** The whole
      fleet runs with no approval prompts in the workspace you launch from.
- [ ] **Agents' own `dispatchableAgents` don't fight the pool.** A lead or unit
      that also declares `dispatchableAgents` gets the *intersection* with the
      pool, which can leave it unable to dispatch a pool agent. Usually leave
      `dispatchableAgents` unset on pool agents and let the pool be the allowlist.
- [ ] **Concurrency fits the work.** At most `[dispatch].max_concurrent_children`
      (default 8) units run at once; more wait as `pending`. Raise it in config
      if the lead fans out wider.

---

## Steering a running flow

Five ways to influence a run, from gentlest to hardest:

| Action | Command | Effect |
| --- | --- | --- |
| Message the lead | `rupu agentiflow send <id> "<text>"` | The lead gets it before its next model call; the next round recaps it. Use for guidance ("skip `third_party/`", "focus on the auth module") |
| Message the lead, urgently | `rupu agentiflow send <id> "<text>" --now` | Same, but it cuts the current round short so the lead turns to it at once |
| A standing scope rule | the lead writes `board.directive` | A rule every unit sees every turn. You can't write one as the operator; ask the lead to, or bake it into the definition's goals and the lead prompt |
| Graceful stop | `rupu agentiflow stop <id>` | The envelope ends the run at the next round boundary as `operator_stop`; units are stopped with it |
| Hard stop | `rupu agentiflow stop <id> --now` | SIGTERM the coordinator and every unit now, SIGKILL after ~12s, record `failed` (`operator_stop:now`) |

`send` versus `send --now`: a plain `send` is polite — the lead picks it up at
its next model call, which may be several tool calls away. `--now` is for "stop
what you're doing and read this": it ends the round early (an in-flight model
call is dropped, a running tool finishes), and the next round starts with the
message. The message is framed to the lead as an operator instruction.

`stop` versus `stop --now`: a graceful `stop` lets the current round finish and
its work land, then stops cleanly as a `completed` run. `stop --now` kills
everything immediately and records a `failed` run, losing the current round's
unbanked work — use it only when you need the processes gone now.

The operator can reach a run only while it is running. A `send` or `stop` to a
finished run, or one whose coordinator died, fails and says why (the control
plane answers `409`). A `running` run whose coordinator is gone is closed out by
the orphan reaper, or by `stop --now`.

On the web, the **Messages** tab has a composer that reaches the lead, and the
header has **Stop** and **Stop now** buttons with the same semantics.

---

## See also

- [agentiflows.md](agentiflows.md): the full reference
- [agentiflow-tutorial.md](agentiflow-tutorial.md): an end-to-end walkthrough
- [agent-format.md](agent-format.md): the agent file format and `tools:`
- [coverage.md](coverage.md): finding reports and the full profile
- [engagement-profiles.md](engagement-profiles.md): asset kinds and depth ladders
- [messaging.md](messaging.md): the board, claims, directives and operator messages
- [configuration.md](configuration.md#pricing): `[pricing]` for `budget.usd`; `[dispatch]` for concurrency
