# rupu agentiflows — design

- **Status:** draft (umbrella architecture spec)
- **Date:** 2026-09-30
- **Author:** matt + Claude
- **Depends on:** the engagement-profiles + asset-model work
  (`docs/superpowers/specs/2026-09-30-rupu-engagement-profiles-asset-model-design.md`).
  Agentiflows sit *on top* of that model: scope, pool, goals, and coverage are all
  expressed in its asset / finding / coverage vocabulary. See §7.
- **Relates to / reuses:** the autoflow controller pattern
  (`issue-supervisor-dispatch.yaml`), the non-linear orchestration arc
  (`docs/superpowers/specs/2026-07-2*-rupu-nonlinear-*`), the session worker,
  `dispatch_agent`, `generate.rs`, the finding store, and the usage ledger.

## 1. Summary

An **agentiflow** is a long-running, goal-directed orchestration: an operator
hands a **lead agent** one or more **engagement profiles**, one or more
measurable **goals**, a **budget**, a **pool** of agents and workflows, and an
authorization **scope**. The lead then *dynamically* decides how to reach those
goals — reusing existing workflows, generating new ones, and dispatching agents —
coordinating a live fleet through a shared board and direct messaging, until a
goal set is met, coverage is reached, a budget is burned, the operator stops it,
or a hard ceiling trips.

The design is deliberately **C-model**: a deterministic **envelope** owns the
loop, the budget, the stop conditions, and the operator mailbox; inside each
round the lead runs **as an agent** (the existing agent runtime) with a large
per-round turn budget, so the core is effectively fully agentic while code —
never the model — holds the purse and the kill switch.

Everything the agentiflow reasons about — the scope it operates within, the
assets it discovers, the findings that count toward a goal, and what "covered"
means — comes from the active **engagement profile(s)** (§7). Agentiflows add no
parallel scope/coverage/asset vocabulary; they orchestrate over the one the
profile model defines.

Two capabilities in this spec are foundational and valuable on their own,
independent of agentiflows:

1. A **pre-turn collector pipeline** — a runtime port that injects context into
   an agent's turn without spending an agentic turn to fetch it (mailbox
   messages, board directives, ambient status, ambient command output). Okesu's
   "collectors", generalized.
2. A **fleet comms substrate** — a file-backed shared **board** (claims, posts,
   directives, findings) plus per-participant **mailboxes**, with delivery at
   agent-loop iteration boundaries.

## 2. Motivation

rupu is used to build code and to review code, and is meant to run
Chimera-style security assessments. All three are goal-directed and open-ended:
"deliver this issue", "find N verified RCEs", "obtain root on host X". Today the
only way to express open-ended, multi-step work is to hand-author a workflow, or
to chain workflows with an autoflow controller that *picks* a pre-written
workflow each cycle. Neither can *compose* new structure in response to what it
finds, coordinate a live fleet, or stop itself on a measurable goal or a budget.

Agentiflows close that gap: the operator states the profile, the goal, and the
constraints; the lead builds and runs whatever structure the goal needs, from the
existing catalog, and stops when the goal is provably met or the budget is spent.

## 3. Goals (of this design)

- Give an operator a way to launch a goal-directed, budget-bounded, long-running
  fleet from a single definition file and steer it in real time.
- Reuse the workflow language and runtime wholesale — including the loops,
  panels, splits, and `for_each` constructs that exist but are under-used.
- Reuse the engagement-profile / asset model wholesale for scope, assets,
  findings, and coverage — add no parallel domain vocabulary.
- Make "done" objective and code-checked, not a subjective self-report.
- Make agent-to-agent and operator-to-lead communication first-class, fast, and
  safe (attributed data, never authority).
- Build the collector pipeline and comms substrate as clean, reusable ports so we
  can add new injectors and coordination primitives later without reworking the
  agent loop.

## 4. Non-goals / explicitly deferred

- **No code-level scope enforcement in v1 (conscious choice, and consistent with
  the profiles spec).** The engagement-profiles spec itself makes live-scope
  enforcement "design-level for the pilot; enforced in the network fast-follow."
  Agentiflows match that: the `scope` (the profile's root asset(s)) is fully and
  deterministically defined and injected to the fleet as authoritative operator
  intent; agents are trusted to understand it and keep themselves in scope — the
  same instruction-trust model rupu already runs on. A running agentiflow in
  `bypass` can do whatever its granted tools and host can reach; in v1 the only
  boundary is the agents' instructions and the operator's trust. Recorded here as
  an accepted risk, not hidden.
- **Scope is deterministic *so the sandbox can be built against it.*** Because
  scope is the profile's typed root asset(s) (host/port/url/cidrs — not free
  text), it is a precise, machine-consumable structure. v1 does not enforce it,
  but it is authored in exactly the shape the future sandbox and warden will read
  as input. Defining it deterministically now is the whole point of doing it now.
- **Warden (code enforcement) is a named follow-on.** A `WardenDecider` at the
  existing `PermissionDecider` chokepoint, with per-tool structured-target
  extraction against the scope assets, fail-closed defaults, and `scope_audit`
  events, is a separate spec. It turns the v1 scope contract into a
  precisely-enforced boundary for structured tools. This is the same guardrail the
  profiles spec calls the "live-touch tool checked fail-closed against the
  in-scope set."
- **Custom egress sandbox is a separate feature, built after this.** Bounding
  arbitrary shell egress needs an OS/network boundary (netns + egress allowlist,
  or a locked container); rupu has none today. It consumes the deterministic scope
  as input. Its own spec, after agentiflows.
- **No dynamic agent minting in v1.** The lead composes *workflows* from a *fixed*
  pool; it does not invent new fleet-member agents mid-run.
- **No new blocking waits between agents in v1.** Messaging is fire-and-forget.
- **No combined multi-profile engagement at runtime in v1 beyond what the profile
  pilot supports.** The model allows several profiles; v1 agentiflows run a single
  engagement profile (the profiles pilot runs one too). The definition permits a
  list for forward-compatibility.

## 5. Terminology

- **Agentiflow** — a goal-directed orchestration run defined by an agentiflow
  definition file and executed by the envelope.
- **Envelope** — the deterministic supervisor loop that owns rounds, budget, stop
  conditions, the fleet, the board, and the operator mailbox. A specialized
  session worker.
- **Lead** — the orchestrator agent, run as a persistent session.
- **Round (tick)** — one envelope iteration.
- **Unit** — a piece of work the lead spawns (a workflow run or a standalone
  dispatched agent). Runs as its own process.
- **Participant** — anything with a mailbox: the operator, the lead, every running
  agent.
- **Board** — the file-backed shared blackboard for a single agentiflow.
- **Collector** — a pre-turn injector implementing the `TurnCollector` port.
- **Engagement profile / asset / coverage ladder** — as defined by the
  engagement-profiles spec; summarized in §7.

## 6. Architecture overview

```
 operator ──(session turn-queue)──► ENVELOPE (deterministic supervisor)
                                      │  owns: rounds, budget, stops, scope,
                                      │        the board, the operator mailbox
                                      ▼
                                    LEAD (agent, run as a session)
                                      │  tools: generate_workflow, run_workflow,
                                      │         dispatch, join, board.*, msg.send,
                                      │         goal/budget/coverage.status,
                                      │         agents/workflows.list/get
                                      ▼
        ┌───────────── spawns (non-blocking, process-isolated) ─────────────┐
        ▼                                ▼                                   ▼
   workflow run (unit)            dispatched agent (unit)            dispatched agent (unit)
        │   (each launched with the active engagement profile)              │
        └──────────── all participants read/write ──────────────────────────┘
                                         │
                                  BOARD + MAILBOXES  (file-backed, in the run dir)
                                         ▲
                        pre-turn COLLECTOR PIPELINE drains into each agent's turn
```

**Layering (bottom to top):**

1. **Engagement-profile / asset model** (`rupu-coverage`, per its own spec) — the
   asset graph, findings, coverage ladders, and scope-as-root-asset that
   agentiflows reason over.
2. **Collector pipeline** (`rupu-agent`) — the agent loop runs registered
   collectors before each model call.
3. **Fleet comms substrate** (`rupu-fleet`, new) — file-backed board + mailboxes
   + participant registry.
4. **Envelope + lead + goals/budget/coverage** (`rupu-agentiflow`, new).
5. **Surfaces** — `rupu-tools`, `rupu-cli` (`rupu agentiflow …`), `rupu-cp`.

## 7. Engagement profile integration

The engagement-profiles spec introduces an **engagement profile** axis
(`code`/`binary`/`network`/`web`/…), orthogonal to the existing `FindingProfile`
(`full`/`summary`). A profile is a data package that declares asset kinds and
their locator coordinates, evidence blocks, classification systems, a
completeness checklist, a coverage depth ladder, and a default **bundle**
(agents/tools/workflows). Crucially: **scope is the root of the asset graph** —
the root asset(s) of an engagement are its scope, and coverage is the asset tree
discovered under the root plus each node's examination depth.

Agentiflows adopt this directly:

- **An agentiflow names its engagement profile(s).** A required `profile:` field.
  This is resolved and propagated to every unit the lead spawns, so every
  workflow run and dispatched agent records findings and coverage under the right
  profile. Resolution and fail-closed rules are the profile spec's (§"Selection,
  resolution & enforcement"): unknown profile id → hard error at load.
- **Scope = the profile's root asset(s).** The agentiflow's `scope:` block
  *instantiates* the root asset kind(s) the profile declares (`network`'s `scope`
  kind with `cidrs`/`out_of_scope`/`window`; `web`'s `target` with
  `in_scope_hosts`/`auth`; `code`'s repo root). This is typed and deterministic —
  the exact structure the future warden/sandbox reads. Agentiflows define no
  separate scope schema.
- **Pool defaults from the profile `bundle`.** `profile: network` defaults the
  pool to that profile's `bundle.agents` (`recon`, `service-analyst`,
  `exploit-verifier`) and `bundle.workflows` (`network-assessment`); the
  agentiflow may narrow or extend. (Per the profile spec, `bundle.tools` is
  launcher prefill only, never a grant — agentiflow tool access still comes from
  agent frontmatter + tiers, §13.)
- **Goals are predicates over profile-typed assets/findings/coverage** (§14): a
  finding's classification uses the profile's taxonomy, a host capability is an
  asset at a coverage-ladder depth, "covered" is the profile's depth ladder.
- **Coverage stop = the profile's asset-tree depth ladder** (§15).

Dependency/sequencing: the agentiflow *machinery* (envelope, collectors, comms,
goals-over-findings) is profile-agnostic and can land against `code`/`binary`
first. Agentiflow security scenarios that need `host`/`port`/`url` assets and
scope-as-root depend on the profiles spec's **network** fast-follow. This is
stated in §23.

## 8. The collector pipeline (foundational)

### 8.1 Purpose

Inject context into an agent's turn **without spending an agentic turn** to fetch
it. Mailbox delivery is one instance; ambient command output and ambient status
are others. Built abstract so new injectors drop in without touching the loop.

### 8.2 Port

Lives in `rupu-agent`. The loop knows only the trait; implementations are
registered per run via `AgentRunOpts`.

```rust
#[async_trait]
pub trait TurnCollector: Send + Sync {
    fn name(&self) -> &str;
    /// Called once immediately before each model call.
    async fn collect(&self, ctx: &TurnContext) -> Vec<Injection>;
}

pub struct Injection {
    pub source: String,        // "mailbox:cobalt-harbor/heron#412", "collector:cmd:nmap", ...
    pub kind: InjectionKind,   // Message | Directive | Observation | Status
    pub cadence: Cadence,      // EveryTurn | Once
    pub priority: u8,          // higher = kept first under the budget
    pub content: String,       // rendered, delimited, attributed body
}

pub enum Cadence {
    EveryTurn, // recomputed each turn; transient — never accumulated in the transcript
    Once,      // delivered once, marked consumed, and persisted into the transcript
}
```

`TurnContext` carries the run id / codename, participant identity, turn index, and
the remaining per-turn injection token budget.

### 8.3 Pipeline semantics

- **Ordering:** collectors run in registration order; injections are concatenated.
- **Cadence decides persistence:** `EveryTurn` is transient (folded in for this
  turn, replaced next, not written to history — ambient command output does not
  bloat the transcript). `Once` is delivered exactly once, marked consumed in its
  source store, and **persisted** into the transcript.
- **Bounded + prioritized:** a per-turn injection token budget; on overflow,
  lower-priority `EveryTurn` injections truncate first; `Once` items are never
  silently dropped — an undeliverable `Once` dead-letters to the board.
- **Data, never authority (hard pipeline invariant):** every injection is wrapped
  and attributed, presented as observed data, and can never override the
  recipient's system prompt or permission grant. Enforced at the pipeline. This
  matters most for ambient command output (attacker-influenceable).

### 8.4 Integration point

The agent loop (`rupu-agent/src/runner.rs`) gains a pre-model-call phase beside
the existing cooperative checks (the `pause` token, `maxTurns`) and the
`on_usage` hook: run the pipeline, fold injections into the outgoing messages, and
append `Once` items to the transcript.

### 8.5 Collectors shipped first

- `MailboxCollector` (`Once`) — drains this participant's inbox.
- `DirectiveCollector` (`Once`/`EveryTurn`) — board directives to this
  participant/role.
- `StatusCollector` (`EveryTurn`) — goal tallies, budget remaining, coverage (for
  the lead; trimmed for workers).
- `RosterCollector` (`EveryTurn`) — the compact agent + workflow index (§12).
- `CommandCollector` (`EveryTurn`) — runs a configured read-only command
  out-of-band and injects fresh output as ambient observation.

## 9. Fleet comms substrate (`rupu-fleet`, new crate)

A leaf-ish store crate: pure on-disk records + lease logic, no agent/provider
deps. Reuses the lease pattern from `rupu-workspace`'s autoflow claim store.

### 9.1 The board

One board per agentiflow run, file-backed under the run dir. Entry types:

- **Claim** — `claim(work_unit, ttl) -> Granted | Denied{holder}`. Atomic
  claim-or-skip on a namespaced key. The deduplication primitive: two branches
  asking for the same unit, one wins. Lease-backed; `release(work_unit)` frees it.
  Keys naturally align with asset ids (`host:1.1.2.2`, `service:1.1.2.2:443`) so
  claims map onto the profile's asset graph.
- **Post** — append-only `{author, ts, kind, body, addressed_to?}`, `kind` ∈
  {observation, question, answer, vote, note}. Read via `board.read(filter)`.
- **Directive** — a lead→fleet standing instruction (`board.directive`), read by
  the `DirectiveCollector`. How operator steering propagates.
- **Finding** — reuses the profile-typed finding store and `record_finding`; the
  board indexes findings so goal evaluation and the coverage ledger read them.

Inspectable in full by the operator and the CP (the coverage map of what has been
claimed / covered / found).

### 9.2 Mailboxes

Every participant has an addressable inbox, file-backed. `msg.send(to, body)`,
`to` ∈ {codename/run-id, role, `parent`, `lead`, `broadcast`}. Delivery via the
`MailboxCollector` at the recipient's next iteration boundary.

- Fire-and-forget; no blocking wait in v1.
- Bounded (capped inbox) and rate-limited (per-agent sends per round); the
  envelope budget is the backstop.
- Dead letters (recipient finished) fall back to the board / the lead.
- Attributed and injected as data (pipeline invariant).

### 9.3 Emergent patterns (built from primitives, not baked in)

Votes, broadcast questions, and knowledge-sharing are compositions of
`msg.send(broadcast, …)` + `board.post` + `board.read`; the lead orchestrates
them. No consensus/voting protocol ships in the substrate.

## 10. Dispatch model — non-blocking, process-isolated

### 10.1 Non-blocking, agentiflow-scoped

Inside an agentiflow, both work-creation paths are **non-blocking**:

- `run_workflow(name | inline_yaml, inputs) -> handle`
- `dispatch(agent, prompt) -> handle`

Each returns a handle immediately; the lead continues and observes progress via
the collectors. `join(handle)` is an explicit opt-in for "I need this result
before I plan further".

**Legacy is untouched.** The existing blocking `dispatch_agent` keeps its exact
fork-join behavior for plain workflows, preserving byte-for-byte legacy workflow
equality. Non-blocking spawn is a new, agentiflow-scoped default — not a global
flip.

### 10.2 Process isolation

- The **lead/envelope is a persistent daemon** (the session worker).
- Each **unit** runs as its own **detached subprocess** with a tracked pid — the
  `cp serve` detached-run model. Crash isolation, per-unit SIGTERM cancel, reuse
  of the orphan reaper (dead `runner_pid` → `Failed`).
- **Within** a unit, existing in-process concurrency (split/parallel/for_each) is
  unchanged — the process boundary is at the lead→unit seam only.
- The board and mailboxes are **file-backed**, so cross-process coordination is
  natural; the collectors poll files.
- Each unit is launched carrying the active engagement profile.

### 10.3 Fan-out governance

Only Tier-2 holders (the lead + capability-gated sub-leads) can spawn, so fan-out
and depth stay bounded. The existing `MAX_DEPTH = 5` dispatch limit applies.

## 11. The envelope and the round lifecycle

The envelope is a specialized session worker. The lead's context persists across
rounds as one session transcript; operator steering is another
`SessionTurnRequest` on that session.

**One round:**

1. **Evaluate stop conditions** (§12). Any met → terminate (or, at the soft budget
   threshold, inject the converge directive and continue).
2. **Assemble the round turn:** drain the operator steering mailbox + a progress
   digest (goal tallies, budget left, board deltas, fleet status); enqueue it as
   the lead's next session turn.
3. **Run the lead agentically** up to its per-round turn budget (large — the core
   is effectively fully agentic). It reads board/status, decides, lays out work
   (non-blocking).
4. **Lead yields** — a `wait` tool ("work laid out; wake me on progress or a
   deadline") — or hits its per-round turn budget.
5. **Envelope waits** on in-flight units / a timer / an inbound steering message,
   then loops to 1.

Resume/pause/cancel reuse the run machinery: pause via the cooperative token /
`.pause` marker; cancel SIGTERMs the fleet pids; resume re-enters at the recorded
round from the persisted session + board state.

## 12. Termination — a disjunction of stop conditions

The agentiflow ends when **any** fires:

1. **Goals met** — all `required` goals satisfy their predicates (§14). Evaluated
   as code by the envelope over the board's verified, profile-typed evidence — the
   lead cannot make the tally lie.
2. **Coverage reached** — the profile's coverage ladder hits the declared target
   (§15). Deterministic.
3. **Budget tripped** — any budget dimension exhausted (§16). Two-stage:
   soft-converge, then hard-kill.
4. **Operator stop** — the operator sends a stop; the envelope drives a graceful
   wind-down (bank findings, summarize) then stops.
5. **Hard ceiling** — max rounds / max wall-clock safety net.

On any terminal condition the envelope runs a **wind-down**: a bounded final
"produce your summary and bank evidence" turn, finalize the run record, and emit a
truthful completion summary (which goals passed/failed, why it stopped).

## 13. Tool taxonomy — always-on vs optional

Three tiers. The envelope constructs each unit's tool registry at spawn.

- **Tier 1 — always-on, platform-granted.** Every participant, regardless of
  frontmatter; `actions:` narrowing cannot strip them (platform, like builtins).
  - `board.claim`, `board.release`, `board.post`, `board.read`, `msg.send`
  - `record_finding` (reuse the existing, now profile-typed, tool)
  - read-only introspection: `goal.status`, `budget.status`, `coverage.status`,
    `agents.list/get`, `workflows.list/get`, `catalog.search`
- **Tier 2 — orchestration, platform-granted but role-gated.** The lead always
  holds these; a pool agent holds them only if its frontmatter `capabilities:`
  includes `orchestrator`.
  - `dispatch`, `join`, `generate_workflow`, `run_workflow`, `board.directive`
- **Tier 3 — optional, agent-declared in frontmatter `tools:` (unchanged).**
  Governed by the run mode and `actions:` narrowing.
  - builtins (`bash`, `read_file`, `grep`), the MCP connector catalog, and future
    domain tools (including the profile's `bundle.tools`, which are prefill, not a
    grant — an agent still declares what it actually uses).

Frontmatter carries two opt-in knobs: `tools:` (Tier-3) and `capabilities:`
(Tier-2). Tier 1 is unconditional.

## 14. Goals and measurability

A goal is an **objective predicate** — a count or boolean over **verified**,
profile-typed evidence on the board. No subjective goals.

```yaml
goals:
  - id: rce
    objective: "Find 10 verified RCE issues in the codebase."
    target: { findings: { classification: "CWE-94", verified: true }, count_gte: 10 }
    required: true
  - id: root-1122
    objective: "Obtain root on 1.1.2.2."
    # an asset in the network profile's graph reaching the 'exploited' depth,
    # backed by a verified finding
    target: { asset: { kind: host, locator: { host: "1.1.2.2" } }, depth_at_least: exploited, verified: true }
    required: true
    verify_with: exploit-verifier   # optional independent corroboration
```

- **The check is code.** The envelope evaluates `target` deterministically over
  the profile's asset graph + finding store + coverage ledger. The lead drives the
  fleet to *make* the predicate true; it does not judge it.
- **Expressed in the profile's vocabulary.** `classification` uses the profile's
  taxonomy (CWE/CVE/OWASP/ATT&CK); `asset.kind`/`locator` use its asset kinds and
  coordinates; `depth_at_least` uses its coverage ladder. Agentiflows invent no
  parallel finding/asset schema.
- **`verified` is load-bearing.** A finding counts only when genuinely verified —
  the profile's completeness checklist + finding reports' `verification_status` /
  `has_poc` / per-claim `evidence_status`. For a host capability the evidence is a
  concrete artifact (command output, the profile's `scan_output`/`http_exchange`
  blocks).
- **`verify_with` (optional) raises the bar** — a finding counts only after an
  independent verifier agent corroborates it.
- **Predicate shape:** a small structured predicate evaluated in code
  (`findings`/`asset` selectors + `count_gte`/`exists`/`depth_at_least`). An
  `expr:` escape hatch (minijinja over board state, reusing the `until:`
  evaluator) exists for advanced cases.

## 15. Coverage as a stop condition

Binds to the profile's **asset-tree coverage ladder** (per the profiles spec:
asset nodes + a per-node depth state from a profile-declared ladder).

```yaml
coverage:
  reach: 0.9                 # 90% of enumerated assets at/above the target depth
  depth: tested              # optional; defaults to the ladder's terminal state
  kinds: [host, service]     # optional; defaults to the profile's `enumerates`
```

The board's claims/findings feed the coverage ledger; the envelope checks the
ledger against `reach`/`depth` for the profile's enumerated asset kinds. No new
metric invented.

## 16. Budget

```yaml
budget:
  usd: 50            # primary — every call is already priced
  tokens: 20000000
  wall_clock: "6h"
  rounds: 40
  soft_at: 0.8       # fraction at which the converge directive fires
```

- **Four dimensions, any subset, first-to-trip wins.** USD primary (usage ledger),
  wall-clock + rounds envelope-owned, tokens from the ledger.
- **Two-stage.** At `soft_at` (default 0.8) the envelope injects a
  **converge-and-bank** directive into the lead's mailbox. At 100% the **hard cap**
  trips the cancel/pause token and SIGTERMs the fleet. The soft stage prevents
  losing work and unbanked findings.
- **Enforcement is code in the envelope** reading the usage ledger + wall-clock +
  round count.

## 17. Operator steering

The operator talks to the lead like a session: `rupu agentiflow send <id>
"<message>"` enqueues a `SessionTurnRequest` on the lead's session.

- **Default: round-boundary delivery** (folded into the round digest assembled in
  §11, step 2).
- **Priority/interrupt (opt-in):** a flagged steering turn trips the pause token to
  cut the current round short and re-enter with the message injected. Default is
  round-boundary (cancelling an in-flight generation wastes spent tokens).
- **Examples:** "focus on the auth module", "expand to the staging subnet", "here
  is more evidence: …", "wrap up and hand me what you have". Scope changes
  requested via steering are honored by the lead (trust model); v1 has no code
  re-check.

## 18. The agentiflow definition file

Lives at `.rupu/agentiflows/<name>.yaml` (project), then `~/.rupu/agentiflows/`
(global), mirroring workflow/agent discovery.

```yaml
name: rce-hunt
description: Find and verify remote code execution across the in-scope services.
lead: orchestrator-lead            # an agent in the pool; auto-holds Tier 1+2

profile: network                   # engagement profile(s) — required; a list is allowed.
                                   #   Resolved + propagated to every spawned unit.

goals:                             # objective predicates over profile-typed evidence
  - id: rce
    objective: "Find 10 verified RCE issues across the in-scope services."
    target: { findings: { classification: "CWE-94", verified: true }, count_gte: 10 }
    required: true

coverage:                          # optional deterministic stop (profile asset tree)
  reach: 0.9
  depth: tested

budget:                            # any subset; first to trip wins
  usd: 50
  wall_clock: "6h"
  rounds: 40
  soft_at: 0.8

scope:                             # instantiates the PROFILE'S root asset(s).
  authorized: true                 #   Typed + deterministic (future sandbox/warden input).
  # fields below are the network profile's `scope` root-asset shape:
  cidrs: ["10.0.0.0/24"]
  hosts: ["1.1.2.2"]
  out_of_scope: ["10.0.0.9"]
  window: "2026-10-01/2026-10-05"
  mode: bypass                     # run permission mode for the fleet
                                   #   (code-enforcement of scope is deferred, §4)

pool:                              # defaults from the profile's `bundle`; may narrow/extend
  agents: [recon, service-analyst, exploit-verifier, finding-fixer]
  workflows: [network-assessment, investigate-then-fix]   # or: all

workspace: { strategy: worktree, branch: "agentiflow/rce-hunt" }

round:
  lead_max_turns: 200              # large — the round core is effectively agentic
  ceiling: { rounds: 40, wall_clock: "8h" }

trigger: manual                    # manual | cron | event (reuse existing)
```

- **Required:** `name`, `lead`, `profile`, at least one of `goals` / `coverage`,
  and `scope`.
- **`scope` is required and complete** — it instantiates the profile's root
  asset(s). Its *shape* is the profile's (a `code` agentiflow's scope is a repo
  root; a `web` agentiflow's is a `target` origin). Its enforcement is deferred
  (§4), not its definition. Authored deterministically so the sandbox/warden reads
  it directly.
- **`pool`** defaults to the profile's `bundle.agents`/`bundle.workflows`;
  explicit values narrow or extend it.
- Parsing/validation: a new `AgentiflowDef::parse(&str)` (in-memory,
  filesystem-free), analogous to `Workflow::parse`. Validates: `profile`
  resolvable (fail-closed), goals predicates well-formed **against the resolved
  profile's taxonomy/asset-kinds/ladder**, `scope` matches the profile's
  root-asset shape, pool agents/workflows resolvable where cheap, budget
  dimensions ≥ 0, `scope.authorized == true`, `lead` ∈ pool.

## 19. Persistence and run model

- **New trigger source / kind:** `RunTriggerSource::Agentiflow`; represent an
  agentiflow run distinctly (parent run with child unit runs; `RunRecord` already
  has `parent_run_id`).
- **Run dir:** `<global>/agentiflows/<id>/`
  - `agentiflow.json` — record (profile(s), goals + status, budget state, coverage
    state, round count, codename, stop reason).
  - `agentiflow.yaml` — definition snapshot.
  - `lead/` — the lead's session transcript + session record.
  - `board/` — claims, posts, directives.
  - `mailboxes/<participant>/` — inboxes.
  - `assets/` + `findings/` — reuse the profile-typed asset graph + finding store,
    indexed by the board.
  - `units/<unit_id>/` — each spawned unit's run dir.
  - `events.jsonl` — agentiflow event stream (round started/ended, unit
    spawned/completed, goal progress, budget stage, stop).
  - `usage.jsonl` — reuse the usage ledger, folded across all units.
- **Status:** reuse `RunStatus` plus agentiflow-specific sub-state in
  `agentiflow.json` (per-goal pass/fail, stop reason).

## 20. CP surfaces

- **New Activity tab "Agentiflows"** (like autoflows), listing agentiflow runs.
- **Run detail:** goals panel (predicate + live tally + pass/fail), budget panel
  (per-dimension spend vs cap + soft/hard state), coverage panel (the profile's
  asset tree + depth ladder), the **board** (claims map + posts + directives +
  findings), the **fleet** (live units with status, codenames, per-unit spend),
  and the lead's session transcript. Steering send box (reuse session send).
- **DTOs / fixtures:** hand-map the agentiflow DTOs; regenerate macOS fixtures if
  the app consumes them (app deprecated for new features — CP web is the target).

## 21. CLI surface (thin, per architecture rule #2)

`rupu-cli/src/cmd/agentiflow.rs` — arg parsing + delegation only.

- `rupu agentiflow run <def> [--input k=v …] [--detach]`
- `rupu agentiflow serve` — run agentiflows as a supervised background service
  (alongside the cron tick + autoflow loops in `cp serve`).
- `rupu agentiflow attach <id>` / `send <id> "<msg>"` — reuse session attach/send.
- `rupu agentiflow status <id>` — goals/budget/coverage/fleet snapshot.
- `rupu agentiflow stop <id>` — graceful operator stop; `--now` for hard stop.
- `rupu agentiflow list`.

## 22. Crate and module placement (hexagonal, rule #1)

- **`rupu-agent`** — the `TurnCollector` port + pipeline in the loop; register
  collectors via `AgentRunOpts`; add the optional `capabilities:` frontmatter
  field.
- **`rupu-fleet`** (new) — file-backed board + mailboxes + participant registry +
  lease logic. Leaf store crate; reuses the claim-store lease pattern from
  `rupu-workspace`. No agent/provider deps.
- **`rupu-agentiflow`** (new) — `AgentiflowDef` (parse/validate against the
  resolved profile), the envelope (round loop, stop conditions, budget enforcer,
  goal evaluator, coverage check, wind-down), the fleet supervisor
  (spawn/watch/reap/kill units). Depends on `rupu-orchestrator` (run_workflow,
  generate, usage ledger), `rupu-agent` (the lead run + collectors), `rupu-fleet`,
  `rupu-coverage` (the asset/profile/coverage model).
- **`rupu-tools`** — the Tier-1/Tier-2 tools; the `AgentDispatcher` trait gains a
  non-blocking spawn method.
- **`rupu-cli`** — the thin `agentiflow` subcommand; **and a refactor:** lift the
  session-worker core out of `cmd/session.rs` into a reusable lib (either
  `rupu-runtime` or a small `rupu-session` crate) so both `rupu session` and the
  envelope share it.
- **`rupu-cp`** — the Agentiflows tab, run-detail panels, DTOs, endpoints.

## 23. Implementation plan decomposition (stacked)

Sequenced so the profile-agnostic machinery lands first and can be exercised
against `code`/`binary`, with the security scenarios following the profiles spec's
network fast-follow.

1. **Plan 1 — Collector pipeline + comms substrate.** The `TurnCollector` port +
   pipeline in `rupu-agent`; `rupu-fleet` board + mailboxes; the Mailbox /
   Directive / Status / Roster / Command collectors. Valuable standalone (any
   `rupu run`/session can register collectors). Ships with the
   data-never-authority, cadence, and bounded-delivery invariants tested. No
   profile dependency.
2. **Plan 2 — Session-worker lift + the envelope skeleton.** Lift the session
   worker to a lib; build the envelope round loop, stop-condition disjunction,
   budget enforcer, goal evaluator, coverage check, wind-down; `AgentiflowDef`
   parse/validate. Goal/coverage evaluation reads the profile model (depends on
   the engagement-profiles pilot landing far enough to expose the asset/finding/
   coverage read APIs; against `code`/`binary` first).
3. **Plan 3 — The lead toolset + non-blocking dispatch + process isolation.**
   Tier-1/2 tools; non-blocking spawn + `join`; process-isolated units + reaper;
   `generate_workflow` taught the full format (loops/panels/splits) and made to
   stamp the active engagement profile on generated workflows; pool validation
   (incl. pool ⊇ the workflow's dispatched agents); `capabilities:` + roster;
   profile propagation to every unit.
4. **Plan 4 — Surfaces.** `rupu agentiflow …` CLI; CP Agentiflows tab + run
   detail (goals/budget/coverage/board/fleet + steering); `cp serve` supervision;
   docs.

(Each plan gets its own `docs/superpowers/plans/…` doc via the writing-plans
skill.)

## 24. Testing strategy

- **Collector pipeline:** cadence (EveryTurn transient vs Once persisted +
  consumed), budget truncation order, Once-never-dropped/dead-letter,
  data-never-authority wrapping. `rupu-agent`.
- **Board/mailboxes:** atomic claim-or-skip under concurrency (two claimers, one
  wins), lease expiry, dead-letter, bounded inbox + rate limit. `rupu-fleet`.
- **Envelope:** stop-condition disjunction (each fires and terminates), goal
  predicate evaluation over a seeded profile-typed board (count/exists/depth,
  verified gating, `verify_with`), budget soft→hard staging, ceiling, wind-down
  truthful summary, resume/cancel. `rupu-agentiflow` with a mock provider + mock
  units.
- **Dispatch/isolation:** non-blocking spawn returns immediately; a panicking unit
  doesn't kill the fleet; per-unit SIGTERM; orphan reap; legacy blocking
  `dispatch_agent` unchanged.
- **Profile integration:** an agentiflow with `profile: X` propagates X to every
  spawned unit; a goal predicate referencing a taxonomy/asset-kind the profile
  doesn't declare is rejected at parse (fail-closed); `scope` mismatching the
  profile's root-asset shape is rejected.
- **Pool safety:** running a catalogued workflow whose agents escape the pool is
  rejected fail-closed.
- **End-to-end (operator gate):** matt runs a real agentiflow with a measurable
  goal (a coverage or count target on a benign local target), steers it mid-run,
  watches the board and fleet in the CP, and confirms it stops on the goal and on a
  budget.

## 25. Open questions

- **Session-worker lift target:** `rupu-runtime` vs a new `rupu-session` crate.
  Decide in Plan 2.
- **Profile read-API surface:** exactly which asset/finding/coverage query
  functions the envelope needs from `rupu-coverage`, and how much of the profiles
  pilot must land before Plan 2. Coordinate with that spec.
- **Goal predicate richness:** how far the structured predicate goes before the
  `expr:` escape hatch. Start minimal (`count_gte`/`exists`/`depth_at_least`).
- **`CommandCollector` cadence controls:** run-every-turn vs interval vs on-change,
  plus its own timeout/rate budget. Decide in Plan 1.
- **Multi-profile agentiflows:** the definition permits a `profile:` list; v1 runs
  a single profile (matching the profiles pilot). When to light up multi.
- **Sub-lead depth:** whether `orchestrator`-capable agents run full rounds
  (recursive envelopes) or only dispatch. v1: dispatch only.

## 26. Follow-on specs (named, not designed here)

- **Warden (code enforcement):** `WardenDecider` at the `PermissionDecider`
  chokepoint; per-tool structured-target extraction **against the scope assets**;
  fail-closed; `scope_audit` events; propagation to every unit. This is the
  profiles spec's "live-touch tool checked fail-closed against the in-scope set,"
  realized for the whole fleet. Turns the v1 scope contract into a
  precisely-enforced boundary for structured tools.
- **Custom egress sandbox:** the OS/network boundary (netns + egress allowlist, or
  a locked container) that bounds arbitrary shell egress — the real boundary for
  offensive work, consuming the deterministic scope as input. Built after
  agentiflows.
